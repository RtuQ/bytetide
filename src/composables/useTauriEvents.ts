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
 * 前端按固定节奏拉 delta 入表。渲染进程不再需要"跟上"任何事件流——
 * 被节流/被调度饥饿时，醒来一次拉齐即收敛，滞后上限=一个拉取周期。
 *
 * 历史：曾用 40ms 推事件流，WebView2 渲染进程被高频小事件挤占调度后，
 * 消费速率跌破生产速率形成死亡螺旋（实测积压 15 分钟、tick 饿到 48s），
 * 故整体倒转为拉（取证数据见 perf-frontend.log seg/tick 探针）。
 */
const PULL_INTERVAL_MS = 200
const PULL_PAGE_MAX = 5000
/** 单次 drain 最多翻页数：24×5000=12 万行 ≥ ring 容量 10 万，一轮必收敛 */
const PULL_MAX_PAGES = 24
/** 上滑回补单页行数：比正向拉取小，保证滚动响应即时（可连续触发多页） */
const BACKFILL_PAGE_MAX = 2000

/** 在途拉取表（会话 id → 完成 promise）：防同会话并发 drain 导致游标回退覆盖，
 *  且值可等待——最终补拉以此做所有权交接，不用固定等待时间猜在途何时结束 */
const draining = new Map<string, Promise<void>>()
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

async function pullUntilEmpty(sessionId: string, ignoreStatus: boolean): Promise<void> {
  const store = useSessionStore()
  for (let page = 0; page < PULL_MAX_PAGES; page++) {
    const s = store.sessions[sessionId]
    // 会话没了/非拉模型会话（offline 初始装载后静态）就停。
    // live：断开后端句柄已移除，停在 connected/connecting 之外；
    // replay：后端会话常驻（Finished/Stopped 后仍可查询），只有控制面定格
    // stopped（用户停止/断开）或会话移除才停——EOF 事件后的最后一波仍要拉齐。
    // ignoreStatus（最终补拉）跳过状态守卫：停止/断开正是要拉这最后一批
    if (!s || !isPullSession(s)) return
    if (
      !ignoreStatus &&
      ((s.kind === 'live' && s.status !== 'connected' && s.status !== 'connecting') ||
        (s.kind === 'replay' && s.replay?.state === 'stopped'))
    )
      return
    let pulled: PulledLine[]
    try {
      pulled = await commands.ringLinesAfter(sessionId, s.pullNo, PULL_PAGE_MAX)
    } catch {
      return // 无后端（浏览器冒烟）或会话已断开，静默
    }
    if (pulled.length === 0) return
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
    if (fresh.length === 0) return // 游标已到最新
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
    if (pulled.length < PULL_PAGE_MAX) return // 拉空，已到最新
  }
}

/** 独占启动一次拉取：check+set 之间无 await（单线程 JS 原子），完成 promise
 *  登记在 draining 供最终补拉等待/交接 */
function beginDrain(sessionId: string, ignoreStatus: boolean): Promise<void> | null {
  if (draining.has(sessionId)) return null
  const p = pullUntilEmpty(sessionId, ignoreStatus)
  const tracked = p.finally(() => {
    if (draining.get(sessionId) === tracked) draining.delete(sessionId)
  })
  draining.set(sessionId, tracked)
  return tracked
}

/** 常规拉取（200ms tick）：在途或最终补拉挂起时合流跳过。
 *  具名导出仅供测试构造「在途拉取」场景——生产入口只有拉取循环 */
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
    let last: Promise<void> | undefined
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

/** 注册后端事件监听与拉取循环；返回取消函数列表 */
export async function setupEvents(): Promise<Unlisten[]> {
  const store = useSessionStore()
  const unlistens: Unlisten[] = []

  // 拉取循环：所有 live 会话按 PULL_INTERVAL_MS 拉自己的游标 delta。
  // 低频 IPC（每会话 5 次/秒、空转返回空），渲染进程调度不再被事件洪水挤占。
  const timer = window.setInterval(() => {
    for (const id of Object.keys(store.sessions)) void drainSession(id)
  }, PULL_INTERVAL_MS)
  unlistens.push(() => window.clearInterval(timer))

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
