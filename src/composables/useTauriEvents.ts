import { useSessionStore } from '../stores/session'
import { isPullSession } from '../stores/session/model'
import { useAlertStore } from '../stores/alerts'
import { playAlertBeep } from './useAlertBeep'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import { recordBatch } from './usePerfWatch'
import { setupBridgeSync } from './useBridgeSync'
import { feedParser } from './useParserEngine'
import { connectionErrorHint, dismissByTag, toast } from './useToast'
import { useNotificationPrefs } from './useNotificationPrefs'
import { consumePortDiff, describePort } from './usePortNotifications'
import { commands } from '../ipc/commands'
import { PULL_CEIL_MS, PULL_FLOOR_MS, initialPacing, nextPullInterval, type PullOutcome } from './pullPacing'
import {
  onAlertHit,
  onBridgeAnnotationsUpdated,
  onBridgePlotUpdated,
  onCaptureActive,
  onCaptureSaved,
  onPortChanged,
  onReplayState,
  onSessionError,
  onSessionStatus,
} from '../ipc/events'
import type { Unlisten } from '../ipc/client'
import type { PulledLine } from '../ipc/types'
import type { AlertLevel } from '../types'

/**
 * 拉模型视图通道：后端 ring 是唯一真相（`no` 游标单调递增、清屏不回退），
 * 前端按会话自适应节奏（pullPacing.ts 控制律：活跃贴 25ms 地板、空闲回
 * 200ms 天花板）拉 delta 入表。渲染进程不再需要"跟上"任何事件流——
 * 被节流/被调度饥饿时，醒来一次拉齐即收敛，滞后上限=一个拉取周期；
 * 节流期 setTimeout 自动晚触发即被动降频，零浪费，回前台首个数据拍贴地板。
 *
 * 历史：曾用 40ms 推事件流，WebView2 渲染进程被高频小事件挤占调度后，
 * 消费速率跌破生产速率形成死亡螺旋（实测积压 15 分钟、tick 饿到 48s），
 * 故整体倒转为拉（取证数据见 perf-frontend.log seg/tick 探针）。拉的方向
 * 自带背压：IPC 频率自限于渲染进程真实处理能力（忙则定时器晚触发、一次
 * 拉齐），每次拉取是替换工作而非累积队列，不会重演积压形态。
 */
const PULL_PAGE_MAX = 5000
/** 单次 drain 最多翻页数：24×5000=12 万行 ≥ ring 容量 10 万，一轮必收敛 */
const PULL_MAX_PAGES = 24
/** 上滑回补单页行数：比正向拉取小，保证滚动响应即时（可连续触发多页） */
const BACKFILL_PAGE_MAX = 2000

/** 在途拉取表（会话 id → 完成 promise）：防同会话并发 drain 导致游标回退覆盖，
 *  且值可等待——最终补拉以此做所有权交接，不用固定等待时间猜在途何时结束。
 *  resolve 值为 PullOutcome，供自适应节奏控制律消费 */
const draining = new Map<string, Promise<PullOutcome>>()
/** 最终补拉挂起中的会话：常规拉取见之让路，保证 stopSession 的 release 只会
 *  发生在最终拉空真正完成之后 */
const tailPending = new Set<string>()
/** 正在往前翻页回补的会话集合（防同会话并发回补重复插入同一批行） */
const backfilling = new Set<string>()

/**
 * 翻页补旧行（方案 B）：用户上滑到视图缓冲头时，把仍在后端 ring 窗口内的
 * 被裁旧行按原行号回补到头部。与正向拉取完全独立——不碰 pullNo/ringDropped，
 * beforeNo 取视图头行的 rn；ring 翻空即置 backfillExhausted 不再白发请求。
 * 由 LogView onScroll 触发（scrollTop < 阈值且未跟随尾部时）。
 */
export async function requestBackfill(sessionId: string): Promise<void> {
  if (backfilling.has(sessionId)) return
    const store = useSessionStore()
    const s = store.sessions[sessionId]
    // live 与 indexed 离线（Task 8 分页）会话均可回补——离线的"ring"是整个源文件；
    // replay 的 ring 命令照常路由（同 RING_CAP 窗口），无需额外守卫
    if (!s || s.backfillExhausted) return
  const head = s.lines[0]
  // 无可补视图（空/清屏后）、视图头无 rn（本地/旧全量链路行）或属旧 ring 纪元
  // （重连迁移行，旧 ring 已销毁；离线会话 reconnectNo 恒 0 不受影响）
  if (!head || head.rn === undefined || head.no <= s.reconnectNo) return
  // ring 最早行不早于视图头：没有更旧的行可补（省一次 invoke）
  try {
    const bounds = await commands.ringBounds(sessionId)
    if (bounds.firstNo >= head.rn) {
      s.backfillExhausted = true
      return
    }
  } catch {
    return // 无后端（浏览器冒烟）或会话已移除，静默
  }
  backfilling.add(sessionId)
  try {
    let pulled: PulledLine[]
    try {
      pulled = await commands.ringLinesBefore(sessionId, head.rn, BACKFILL_PAGE_MAX)
    } catch {
      return // 会话已断开/移除，静默
    }
    const s2 = store.sessions[sessionId]
    if (!s2) return
    if (pulled.length === 0) {
      s2.backfillExhausted = true // ring 内更早的行已全部回补完
      return
    }
    store.prependBackfill(
      sessionId,
      pulled.map((l) => ({
        ts: l.ts,
        dir: l.dir,
        text: l.text,
        bytes: l.bytes,
        epochMillis: l.epochMillis,
        ringNo: l.no,
      })),
    )
    // 注意：不给解析引擎喂补行——framer 是 (sessionId,dir) 有序状态机，
    // 乱序喂历史行会破坏切帧；解码回看由「导入时回溯 2000 行」既有机制覆盖
  } finally {
    backfilling.delete(sessionId)
  }
}

async function pullUntilEmpty(sessionId: string, ignoreStatus: boolean): Promise<PullOutcome> {
  const store = useSessionStore()
  let got = 0
  for (let page = 0; page < PULL_MAX_PAGES; page++) {
    const s = store.sessions[sessionId]
    // 会话没了/非拉模型会话（offline 初始装载后静态）就停。
    // live：断开后端句柄已移除，停在 connected/connecting 之外；
    // replay：后端会话常驻（Finished/Stopped 后仍可查询），只有控制面定格
    // stopped（用户停止/断开）或会话移除才停——EOF 事件后的最后一波仍要拉齐。
    // ignoreStatus（最终补拉）跳过状态守卫：停止/断开正是要拉这最后一批
    if (!s || !isPullSession(s)) return { got: 0, capped: false }
    if (
      !ignoreStatus &&
      ((s.kind === 'live' && s.status !== 'connected' && s.status !== 'connecting') ||
        (s.kind === 'replay' && s.replay?.state === 'stopped'))
    )
      return { got, capped: false }
    let pulled: PulledLine[]
    try {
      pulled = await commands.ringLinesAfter(sessionId, s.pullNo, PULL_PAGE_MAX)
    } catch {
      return { got, capped: false } // 无后端（浏览器冒烟）或会话已断开，静默
    }
    if (pulled.length === 0) return { got, capped: false }
    const t0 = performance.now()
    const fresh = store.appendPulled(
      sessionId,
      pulled.map((l) => ({
        ts: l.ts,
        dir: l.dir,
        text: l.text,
        bytes: l.bytes,
        epochMillis: l.epochMillis,
        ringNo: l.no,
      })),
    )
    if (fresh.length === 0) return { got, capped: false } // 游标已到最新
    got += fresh.length
    store.tallyBytes(sessionId, fresh)
    // 性能哨兵：滞后=墙钟−最新行后端时间戳，批耗时=本处理段。
    // replay 跳过——行时间戳是源文件历史时刻，滞后恒为巨值（假阳性）；
    // 取证探针同理（seg 探针的 lagMs 同口径）
    if (s.kind !== 'replay') {
      recordBatch(sessionId, fresh, performance.now() - t0)
    }
    // 解析引擎 feed（未启用脚本时 no-op）：切帧在主线程线性批处理
    feedParser(sessionId, fresh)
    // 取证探针（seg/raf，仅 DEV 构建；release 由 Vite tree-shake 移除）
    const handlerMs = performance.now() - t0
    if (import.meta.env.DEV && handlerMs > 5 && s.kind !== 'replay') {
      const s2 = store.sessions[sessionId]
      void commands
        .appendPerfDiagnostic({
          kind: 'seg',
          sessionId,
          lagMs: Math.min(Date.now() - fresh[fresh.length - 1]!.epochMillis, 4_000_000),
          batchMs: Math.round(handlerMs * 10) / 10,
          lines: s2?.lines.length ?? 0,
          vis: `a=${fresh.length},pg=${page + 1}`,
        })
        .catch(() => {})
      requestAnimationFrame(() => {
        void commands
          .appendPerfDiagnostic({
            kind: 'raf',
            sessionId,
            lagMs: Math.round(performance.now() - t0),
            batchMs: 0,
            lines: s2?.lines.length ?? 0,
            vis: '',
          })
          .catch(() => {})
      })
    }
    if (pulled.length < PULL_PAGE_MAX) return { got, capped: false } // 拉空，已到最新
  }
  // 翻满 PULL_MAX_PAGES 仍整页返回：明确落后于 ring 产能，控制律据此维持地板
  return { got, capped: true }
}

/** 独占启动一次拉取：check+set 之间无 await（单线程 JS 原子），完成 promise
 *  登记在 draining 供最终补拉等待/交接。在途时返回 null（调用方合流跳过） */
function beginDrain(sessionId: string, ignoreStatus: boolean): Promise<PullOutcome> | null {
  if (draining.has(sessionId)) return null
  const p = pullUntilEmpty(sessionId, ignoreStatus)
  const tracked = p.finally(() => {
    if (draining.get(sessionId) === tracked) draining.delete(sessionId)
  })
  draining.set(sessionId, tracked)
  return tracked
}

/** 常规拉取（拉取循环 tick 触发）：在途或最终补拉挂起时合流跳过。
 *  具名导出仅供测试构造「在途拉取」场景——生产入口是 startPullLoop 的调度器 */
export function drainSession(sessionId: string): void {
  if (tailPending.has(sessionId)) return
  beginDrain(sessionId, false)
}

/** 测试探针（勿在生产代码消费）：会话的拉取互斥状态快照 */
export function drainStateForTest(sessionId: string): {
  draining: boolean
  tailPending: boolean
} {
  return { draining: draining.has(sessionId), tailPending: tailPending.has(sessionId) }
}

/**
 * 最终补拉（评审 P1-2「停止丢尾批」修复）：停止/断开瞬间，最近一个拉取周期
 * （渲染进程被系统节流时远不止 200ms）内已进入后端 ring、尚未入表的行由这里
 * 收尾——无视连接状态守卫拉空游标。live 停止后端留有只读墓碑 ring（两阶段
 * 关闭第 1 阶段），设备断连时 ring 本就在；调用方随后 releaseSession 释放
 * （第 2 阶段）。
 *
 * 所有权交接（复审 R-P1-2）：先置 tailPending 让常规拉取让路，再 **await 在途
 * 拉取的真实完成 promise**（不用固定等待时间猜），等干净后经同步 check+set
 * 独占执行最终拉空。stopSession 必须等本函数 resolve 后才 release——释放只会
 * 发生在最终拉空真正完成之后。并发重入（stopSession 与事件侧双路）经
 * tailPending 合流。
 */
export async function drainSessionTail(sessionId: string): Promise<void> {
  if (tailPending.has(sessionId)) return
  tailPending.add(sessionId)
  try {
    // 依次等待在途拉取真正结束（tailPending 已就位，不会再有新的常规拉取）。
    // last 哨兵防微任务时序下对同一 promise 重复等待
    let last: Promise<PullOutcome> | undefined
    for (;;) {
      const inflight = draining.get(sessionId)
      if (!inflight || inflight === last) break
      last = inflight
      await inflight.catch(() => {})
    }
    await beginDrain(sessionId, true)
  } finally {
    tailPending.delete(sessionId)
  }
}

// ── 自适应拉取调度器 ─────────────────────────────────────────────────────
// 单链 setTimeout + 每会话 nextDue：唤醒只是 Map 扫描（零 IPC），各会话按
// 自己的节奏到期才拉——活跃会话贴 25ms 地板不会拖着空闲会话陪跑，控制律
// 见 pullPacing.ts。不设会话级定时器，无需感知会话增删（惰性建账+顺手剪除）。

/** 每会话节奏条目：控制律状态 + 下次应拉时刻（performance.now 时基） */
interface PacingEntry {
  intervalMs: number
  idleStreak: number
  nextDue: number
}

const pacing = new Map<string, PacingEntry>()
// ReturnType 而非 number：项目同时带 DOM/Node 类型，裸 setTimeout 命中 Node 重载；
// 且调度器要在 node 环境的测试里真跑（window.setInterval 旧写法在 node 下不存在）
let pullTimer: ReturnType<typeof setTimeout> | null = null

/** 到期会话各拉一次，再按最近的 nextDue 排下一跳 */
function loopPulls(): void {
  pullTimer = null
  const store = useSessionStore()
  const now = performance.now()
  for (const id of Object.keys(store.sessions)) {
    const s = store.sessions[id]
    if (!s || !isPullSession(s)) continue
    let entry = pacing.get(id)
    if (!entry) {
      entry = { ...initialPacing(), nextDue: now } // 新账首拉立即（重连新 id 亦然）
      pacing.set(id, entry)
    }
    if (entry.nextDue <= now) void tickSession(id, now)
  }
  // 顺手剪除已移除/非拉模型（offline、closeTab 后）会话的节奏条目
  for (const id of pacing.keys()) {
    const s = store.sessions[id]
    if (!s || !isPullSession(s)) pacing.delete(id)
  }
  let wait = PULL_CEIL_MS
  for (const e of pacing.values()) {
    wait = Math.min(wait, Math.max(0, e.nextDue - now))
  }
  pullTimer = setTimeout(loopPulls, wait)
}

async function tickSession(id: string, now: number): Promise<void> {
  const entry = pacing.get(id)
  if (!entry) return
  if (tailPending.has(id)) {
    // 最终补拉交接中：常规拉取让路（语义见 drainSessionTail），慢拍复查即可
    entry.nextDue = now + PULL_CEIL_MS
    return
  }
  // 暂定复查点：拉取在途时最快 25ms 后再看；outcome 到账后按控制律改写
  entry.nextDue = now + PULL_FLOOR_MS
  const outcome = await beginDrain(id, false)
  const e = pacing.get(id)
  if (!outcome || !e) return // 与在途拉取合流 / 条目已随会话剪除（暂定点兜底）
  nextPullInterval(e, outcome)
  e.nextDue = performance.now() + e.intervalMs
}

/** 启动自适应拉取循环（生产入口 setupEvents；具名导出供测试直接驱动） */
export function startPullLoop(): void {
  stopPullLoop()
  loopPulls()
}

/** 停止循环并清空节奏账目（setupEvents 卸载 / 测试收尾用） */
export function stopPullLoop(): void {
  if (pullTimer !== null) {
    clearTimeout(pullTimer)
    pullTimer = null
  }
  pacing.clear()
}

/** 注册后端事件监听与拉取循环；返回取消函数列表 */
export async function setupEvents(): Promise<Unlisten[]> {
  const store = useSessionStore()
  const unlistens: Unlisten[] = []

  // 拉取循环：自适应节奏（活跃贴 25ms 地板、空闲回 200ms 天花板），控制律在
  // pullPacing.ts。低频 IPC（空拉为后端一次读锁+二分、空转返回空），渲染进程
  // 调度不再被事件洪水挤占。
  startPullLoop()
  unlistens.push(stopPullLoop)

  // 会话状态：live 的连接/断开 toast 提示（通知重设计 v2：断开升级 warning + 6s +
  // 'disconnect' tag，重连成功时按 tag 收掉未过期的断开提示）；replay 的状态事件不打
  // 连接 toast（起跑 connected / EOF·停止 disconnected 对回放语义是「回放中/已播完」，
  // 用 ReplayControls 的状态标签表达），断开时最终补拉收尾（丢尾批修复，不受开关门控）
  const notif = useNotificationPrefs().prefs
  unlistens.push(
    await onSessionStatus((p) => {
      store.setStatus(p.sessionId, p.status)
      const session = store.sessions[p.sessionId]
      if (session?.kind === 'replay') {
        if (p.status === 'disconnected') void drainSessionTail(p.sessionId)
        return
      }
      if (p.status === 'connected' && notif.enabled) {
        dismissByTag('disconnect')
        toast('连接成功', 'success', 2600, session?.config.name)
      }
      if (p.status === 'disconnected') {
        if (notif.enabled) toast('连接已断开', 'warning', 6000, session?.config.name, 'disconnect')
        void drainSessionTail(p.sessionId)
      }
    }),
  )
  unlistens.push(
    await onSessionError((p) => {
      store.setError(p.sessionId, p.error)
      // 正常断连（eof「已断开」文案）hint 返回 null：由 disconnected 状态提示负责，不报 error
      const hint = connectionErrorHint(p.error)
      if (hint) toast(hint.title, 'error', 4500, hint.action)
    }),
  )
  unlistens.push(
    await onPortChanged((ports) => {
      store.setPorts(ports)
      // 热插拔通知（通知重设计 v2）：后端轮询 diff 不带方向，前端对前后列表求差；
      // 首帧只建基线（启动时已插着的端口不刷「已接入」），开关在设置弹层「通知」分组
      const diff = consumePortDiff(ports)
      if (!diff) return
      if (!notif.enabled) return
      for (const p of diff.arrived) toast(`串口已接入 · ${p.name}`, 'success', 3200, describePort(p))
      for (const p of diff.removed) toast(`串口已移除 · ${p.name}`, 'warning', 4200, describePort(p))
    }),
  )
  // REST 桥写回绘图文法（POST /plot-config）：前端即时采纳，绘图面板与曲线同步刷新
  unlistens.push(
    await onBridgePlotUpdated((p) => {
      store.adoptBridgePlot(p.sessionId, p.config)
    }),
  )
  // AI 批注（POST/DELETE /annotations）：日志行标记与侧栏面板实时刷新
  unlistens.push(
    await onBridgeAnnotationsUpdated((p) => {
      store.applyBridgeAnnotations(p.sessionId, p.annotations)
    }),
  )
  // 书签/告警历史推送到后端桥镜像（REST /bookmarks、/alerts 只读）
  unlistens.push(...setupBridgeSync())

  // 告警命中（后端读线程评估，稀疏事件）：通知 + 提示音 + 历史入表
  const alertStore = useAlertStore()
  alertStore.load()
  unlistens.push(
    await onAlertHit((p) => {
      for (const h of p.hits) {
        // ring no -> UI 行号（拉模型下两者不同；rn 由 appendPulled 携带）
        const s = store.sessions[p.sessionId]
        const uiNo = s?.lines.find((l) => l.rn === h.no)?.no ?? null
        const title = `${ALERT_LEVEL_LABEL[h.level] ?? h.level} · ${s?.config.name ?? p.sessionId}`
        const body = `[${h.pattern}] ${alertSnippet(h.text)}`
        void ensureNotify(title, body)
        toast(title, 'warning', 4200, body)
        if (alertStore.sound) playAlertBeep()
        alertStore.push({
          sessionId: p.sessionId,
          sessionName: s?.config.name ?? '',
          ruleId: h.ruleId,
          pattern: h.pattern,
          level: h.level as AlertLevel,
          no: uiNo ?? 0,
          ts: h.ts,
          text: alertSnippet(h.text),
          at: h.at,
        })
      }
    }),
  )

  // 现场捕获事件（极稀疏）：armed 置「捕获中」呼吸指示，档案落成刷新列表并解除指示
  unlistens.push(
    await onCaptureActive((p) => {
      store.setCaptureActive(p.sessionId, p.rule)
    }),
  )
  unlistens.push(
    await onCaptureSaved((p) => {
      store.setCaptureActive(p.sessionId, null)
      toast('现场捕获已保存', 'success', 3200)
      void store.loadCaptures()
    }),
  )

  // 回放控制面（Stage 3 Task 7）：control 命令执行后命令层 emit 一次；EOF/Error
  // 等无控制命令的状态变化由 ReplayControls 的 replayStatus 轮询兜底
  unlistens.push(
    await onReplayState((p) => {
      store.setReplayView(p.sessionId, p)
    }),
  )

  return unlistens
}

const ALERT_LEVEL_LABEL: Record<string, string> = {
  info: '提示',
  warn: '警告',
  err: '错误',
}

function alertSnippet(text: string): string {
  const t = text.replace(/\s+/g, ' ').trim()
  return t.length > 100 ? `${t.slice(0, 100)}…` : t || '(空行)'
}

async function ensureNotify(title: string, body: string) {
  try {
    let granted = await isPermissionGranted()
    if (!granted) granted = (await requestPermission()) === 'granted'
    if (granted) sendNotification({ title, body })
  } catch {
    /* 通知不可用时静默 */
  }
}
