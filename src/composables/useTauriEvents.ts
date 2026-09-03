import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore } from '../stores/session'
import { useAlertStore } from '../stores/alerts'
import { playAlertBeep } from './useAlertBeep'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import { recordBatch } from './usePerfWatch'
import { setupBridgeSync } from './useBridgeSync'
import type {
  AiAnnotation,
  AlertLevel,
  ErrorPayload,
  LogLine,
  PlotConfig,
  PortInfo,
  StatusPayload,
} from '../types'

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
/** 单次 drain 最多翻页数：8×5000=4 万行 > ring 容量 2 万，一轮必收敛 */
const PULL_MAX_PAGES = 8

interface PulledLine {
  no: number
  ts: string
  dir: LogLine['dir']
  text: string
  bytes: number[] | null
  epochMillis: number
}

/** 正在拉取的会话集合（防同会话并发 drain 导致游标回退覆盖） */
const draining = new Set<string>()

async function drainSession(sessionId: string): Promise<void> {
  if (draining.has(sessionId)) return
  draining.add(sessionId)
  try {
    const store = useSessionStore()
    for (let page = 0; page < PULL_MAX_PAGES; page++) {
      const s = store.sessions[sessionId]
      // 会话没了/已停止（后端句柄移除，invoke 会报"会话不存在"）就停
      if (!s || s.kind !== 'live') return
      if (s.status !== 'connected' && s.status !== 'connecting') return
      let pulled: PulledLine[]
      try {
        pulled = await invoke<PulledLine[]>('ring_lines_no_cmd', {
          sessionId,
          sinceNo: s.pullNo,
          max: PULL_PAGE_MAX,
        })
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
      // 性能哨兵：滞后=墙钟−最新行后端时间戳，批耗时=本处理段
      recordBatch(sessionId, fresh, performance.now() - t0)
      // 取证探针（seg/raf，仅 DEV 构建；release 由 Vite tree-shake 移除）
      const handlerMs = performance.now() - t0
      if (import.meta.env.DEV && handlerMs > 5) {
        const s2 = store.sessions[sessionId]
        void invoke('append_perf_diag_cmd', {
          kind: 'seg',
          sessionId,
          lagMs: Math.min(Date.now() - fresh[fresh.length - 1]!.epochMillis, 4_000_000),
          batchMs: Math.round(handlerMs * 10) / 10,
          lines: s2?.lines.length ?? 0,
          vis: `a=${fresh.length},pg=${page + 1}`,
        }).catch(() => {})
        requestAnimationFrame(() => {
          void invoke('append_perf_diag_cmd', {
            kind: 'raf',
            sessionId,
            lagMs: Math.round(performance.now() - t0),
            batchMs: 0,
            lines: s2?.lines.length ?? 0,
            vis: '',
          }).catch(() => {})
        })
      }
      if (pulled.length < PULL_PAGE_MAX) return // 拉空，已到最新
    }
  } finally {
    draining.delete(sessionId)
  }
}

/** 注册后端事件监听与拉取循环；返回取消函数列表 */
export async function setupEvents(): Promise<UnlistenFn[]> {
  const store = useSessionStore()
  const unlistens: UnlistenFn[] = []

  // 拉取循环：所有 live 会话按 PULL_INTERVAL_MS 拉自己的游标 delta。
  // 低频 IPC（每会话 5 次/秒、空转返回空），渲染进程调度不再被事件洪水挤占。
  const timer = window.setInterval(() => {
    for (const id of Object.keys(store.sessions)) void drainSession(id)
  }, PULL_INTERVAL_MS)
  unlistens.push(() => window.clearInterval(timer))

  // 会话停止时立即拉最后一波（disconnect 前后端已 flush 完 ring）
  unlistens.push(
    await listen<StatusPayload>('session-status', (e) => {
      store.setStatus(e.payload.sessionId, e.payload.status)
      if (e.payload.status === 'disconnected') void drainSession(e.payload.sessionId)
    }),
  )
  unlistens.push(
    await listen<ErrorPayload>('session-error', (e) => {
      store.setError(e.payload.sessionId, e.payload.error)
    }),
  )
  unlistens.push(
    await listen<PortInfo[]>('port-changed', (e) => {
      store.setPorts(e.payload)
    }),
  )
  // REST 桥写回绘图文法（POST /plot-config）：前端即时采纳，绘图面板与曲线同步刷新
  unlistens.push(
    await listen<{ sessionId: string; config: PlotConfig }>('bridge-plot-updated', (e) => {
      store.adoptBridgePlot(e.payload.sessionId, e.payload.config)
    }),
  )
  // AI 批注（POST/DELETE /annotations）：日志行标记与侧栏面板实时刷新
  unlistens.push(
    await listen<{ sessionId: string; annotations: AiAnnotation[] }>(
      'bridge-annotations-updated',
      (e) => {
        store.applyBridgeAnnotations(e.payload.sessionId, e.payload.annotations)
      },
    ),
  )
  // 书签/告警历史推送到后端桥镜像（REST /bookmarks、/alerts 只读）
  unlistens.push(...setupBridgeSync())

  // 告警命中（后端读线程评估，稀疏事件）：通知 + 提示音 + 历史入表
  const alertStore = useAlertStore()
  alertStore.load()
  unlistens.push(
    await listen<{ sessionId: string; hits: { ruleId: string; pattern: string; level: string; no: number; ts: string; text: string; at: number }[] }>(
      'alert-hit',
      (e) => {
        for (const h of e.payload.hits) {
          // ring no -> UI 行号（拉模型下两者不同；rn 由 appendPulled 携带）
          const s = store.sessions[e.payload.sessionId]
          const uiNo = s?.lines.find((l) => l.rn === h.no)?.no ?? null
          const title = `${ALERT_LEVEL_LABEL[h.level] ?? h.level} · ${s?.config.name ?? e.payload.sessionId}`
          const body = `[${h.pattern}] ${alertSnippet(h.text)}`
          void ensureNotify(title, body)
          if (alertStore.sound) playAlertBeep()
          alertStore.push({
            sessionId: e.payload.sessionId,
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
      },
    ),
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
