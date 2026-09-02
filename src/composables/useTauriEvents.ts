import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore } from '../stores/session'
import { recordBatch } from './usePerfWatch'
import { setupBridgeSync } from './useBridgeSync'
import type {
  AiAnnotation,
  ErrorPayload,
  LogLine,
  LogPayload,
  PlotConfig,
  PortInfo,
  StatusPayload,
} from '../types'

/** 自愈补拉：事件派发被深度节流推迟时（后端正常、前端滞后），直接从后端
 *  ring 拉缺失行补齐，不等事件补投。仅真饿死（滞后>30s）才拉--普通赤字
 *  （GC/瞬时负载/搜索扫描吃掉部分吞吐）事件队列会自行排空，提前拉只会
 *  引入迟到重复行（虽有水位去重兜底）；带 5s 冷却。失败静默。 */
const PULL_LAG_MS = 30_000
const PULL_COOLDOWN_MS = 5000
let lastPullAt = 0

async function selfHealPull(sessionId: string, lastEpoch: number) {
  const store = useSessionStore()
  try {
    const pulled = await invoke<{ epochMillis: number; dir: LogLine['dir']; ts: string; text: string; bytes?: number[] | null }[]>(
      'ring_lines_cmd',
      { sessionId, sinceEpoch: lastEpoch, max: 2000 },
    )
    if (pulled.length === 0) return
    const inserted = store.appendMissing(
      sessionId,
      pulled.map((l) => ({ ts: l.ts, dir: l.dir, text: l.text, bytes: l.bytes ?? null, epochMillis: l.epochMillis })),
    )
    if (inserted.length > 0) {
      store.tallyBytes(
        sessionId,
        inserted.map((l) => ({ ts: l.ts, dir: l.dir, text: l.text, bytes: l.bytes, epochMillis: l.epochMillis })),
      )
      console.info(`[perf] 自愈补拉 ${inserted.length} 行 (session ${sessionId})`)
    }
  } catch {
    /* 浏览器冒烟环境无 Tauri 后端，静默 */
  }
}

/** 注册后端事件监听并分发到 store；返回取消监听函数列表 */
export async function setupEvents(): Promise<UnlistenFn[]> {
  const store = useSessionStore()
  const unlistens: UnlistenFn[] = []

  unlistens.push(
    await listen<LogPayload>('log', (e) => {
      const t0 = performance.now()
      const fresh = store.appendLines(e.payload.sessionId, e.payload.lines)
      const tAppend = performance.now()
      store.tallyBytes(e.payload.sessionId, e.payload.lines)
      const tTally = performance.now()
      store.processAutoReply(e.payload.sessionId, e.payload.lines)
      const tReply = performance.now()
      // 告警用带行号的新行：历史条目可跳转回日志
      store.processAlerts(e.payload.sessionId, fresh)
      const tHandler = performance.now()
      // 性能哨兵：滞后=墙钟−最新行后端时间戳（覆盖事件队列等待），批耗时=本处理段
      recordBatch(e.payload.sessionId, fresh, tHandler - t0)
      // 深度节流自愈：本批滞后>1s 时从后端 ring 拉齐（事件循环活着才走得到这里）
      const s = store.sessions[e.payload.sessionId]
      const last = s?.lines[s.lines.length - 1]
      // 取证探针：分段耗时落盘（seg），附会话总行数做「成本 ∝ 行数」相关轴；
      // 渲染 flush 用 rAF 估测（raf），延迟>50ms 才记。确认热点后移除。
      const lag = last ? Date.now() - last.epochMillis : 0
      const handlerMs = tHandler - t0
      if (handlerMs > 5 || lag > 2000) {
        const seg = `a=${(tAppend - t0).toFixed(1)},t=${(tTally - tAppend).toFixed(1)},r=${(tReply - tTally).toFixed(1)},al=${(tHandler - tReply).toFixed(1)}`
        void invoke('append_perf_diag_cmd', {
          kind: 'seg',
          sessionId: e.payload.sessionId,
          lagMs: Math.min(lag, 4_000_000),
          batchMs: Math.round(handlerMs * 10) / 10,
          lines: s?.lines.length ?? 0,
          vis: seg,
        }).catch(() => {})
        requestAnimationFrame(() => {
          void invoke('append_perf_diag_cmd', {
            kind: 'raf',
            sessionId: e.payload.sessionId,
            lagMs: Math.round(performance.now() - tHandler),
            batchMs: 0,
            lines: s?.lines.length ?? 0,
            vis: '',
          }).catch(() => {})
        })
      }
      if (last) {
        if (lag > PULL_LAG_MS && Date.now() - lastPullAt > PULL_COOLDOWN_MS) {
          lastPullAt = Date.now()
          void selfHealPull(e.payload.sessionId, last.epochMillis)
        }
      }
    }),
  )
  unlistens.push(
    await listen<StatusPayload>('session-status', (e) => {
      store.setStatus(e.payload.sessionId, e.payload.status)
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

  return unlistens
}
