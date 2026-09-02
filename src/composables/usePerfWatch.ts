import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

/**
 * 前端性能哨兵（模块级单例，与 useTheme 同风格）。
 * 解决"长时间监控后不实时"这类问题的取证：把积压深度与批耗时变成数字。
 *
 * 核心指标：
 * - lagMs  = 当前墙钟 − 最新行后端时间戳。即事件从后端发出到被前端处理的积压深度。
 * - batchCostMs = 单批次处理耗时 EMA。
 *
 * 越限写诊断环形日志（cap 500，够覆盖数小时心跳）；静默期不评估——
 * 长静默后首批的"滞后"其实是静默时长，不是积压（假阳性）。
 */

export interface DiagEntry {
  at: number
  kind: 'lag' | 'recover' | 'slowbatch'
  sessionId: string
  lagMs: number
  batchMs: number
  lines: number
  /** 记录该批次时窗口可见性。'visible' 时仍滞后=真积压；'hidden' 时滞后=后台/锁屏被限流。 */
  vis: 'visible' | 'hidden' | 'unknown'
}

const MAX_ENTRIES = 500
const LAG_WARN_MS = 2000
const LAG_RECOVER_MS = 500
const BATCH_WARN_MS = 50
const IDLE_RESET_MS = 3000
const HEARTBEAT_MS = 60_000

const entries = ref<DiagEntry[]>([])
const lagMs = ref(0)
const batchCostMs = ref(0)

let inLag = false
let lastWarnAt = 0
let lastArrivalAt = 0

function push(e: DiagEntry) {
  entries.value = [e, ...entries.value].slice(0, MAX_ENTRIES)
  // 旁路落盘：不 await、失败静默。前端卡顿但事件循环仍活时这里能写出去；
  // 与后端 perf-heartbeat.log 互补（前端死透时靠后者证明后端视角正常）。
  void invoke('append_perf_diag_cmd', {
    kind: e.kind,
    sessionId: e.sessionId,
    lagMs: e.lagMs,
    batchMs: e.batchMs,
    lines: e.lines,
    vis: e.vis,
  }).catch(() => {})
}

export function usePerfWatch() {
  return { entries, lagMs, batchCostMs, recordBatch }
}

export function recordBatch(
  sessionId: string,
  lines: { epochMillis: number }[],
  tookMs: number,
) {
  const now = Date.now()
  const idleGap = now - lastArrivalAt
  const firstAfterIdle = lastArrivalAt === 0 || idleGap > IDLE_RESET_MS
  lastArrivalAt = now
  batchCostMs.value = Math.round(batchCostMs.value * 0.8 + tookMs * 0.2)
  if (firstAfterIdle || lines.length === 0) return

  const newest = lines[lines.length - 1]!.epochMillis
  const lag = Math.max(0, now - newest)
  lagMs.value = lag
  // 捕获当前窗口可见性：hidden 时滞后多半来自系统限流而非真积压
  const vis: DiagEntry['vis'] =
    typeof document !== 'undefined' && document.visibilityState
      ? document.visibilityState === 'visible'
        ? 'visible'
        : 'hidden'
      : 'unknown'

  if (lag > LAG_WARN_MS) {
    // 进入滞后打一条，之后每 60s 心跳一条，避免刷屏
    if (!inLag || now - lastWarnAt >= HEARTBEAT_MS) {
      inLag = true
      lastWarnAt = now
      push({
        at: now,
        kind: 'lag',
        sessionId,
        lagMs: lag,
        batchMs: Math.round(tookMs),
        lines: lines.length,
        vis,
      })
      console.warn(`[perf] 处理滞后 ${lag}ms (session ${sessionId}, vis=${vis})`)
    }
  } else if (inLag && lag < LAG_RECOVER_MS) {
    inLag = false
    push({
      at: now,
      kind: 'recover',
      sessionId,
      lagMs: lag,
      batchMs: Math.round(tookMs),
      lines: lines.length,
      vis,
    })
    console.info(`[perf] 滞后恢复，lag=${lag}ms`)
  }

  if (tookMs > BATCH_WARN_MS && now - lastWarnAt >= HEARTBEAT_MS) {
    lastWarnAt = now
    push({
      at: now,
      kind: 'slowbatch',
      sessionId,
      lagMs: lag,
      batchMs: Math.round(tookMs),
      lines: lines.length,
      vis,
    })
  }
}

// ===== 临时取证探针（验证 WebView2 唤醒节流，确认后移除） =====
// tick：1s 心跳。正常节拍 gap≈1000ms；若被深度节流，gap 会呈 ~60s 级跳变。
// gap>1.5s 才落盘（kind=tick，lagMs 字段记 gap），正常时零噪音。
// consume：滞后(>2s)批次逐批记录（不受 60s 心跳门控），还原唤醒后的排空模式。
let lastTickAt = 0
setInterval(() => {
  const now = Date.now()
  const gap = lastTickAt ? now - lastTickAt : 1000
  lastTickAt = now
  console.log('tick', now, `gap=${gap}ms`)
  if (gap > 1500) {
    const vis: DiagEntry['vis'] =
      typeof document !== 'undefined' && document.visibilityState === 'visible'
        ? 'visible'
        : typeof document === 'undefined'
          ? 'unknown'
          : 'hidden'
    void invoke('append_perf_diag_cmd', {
      kind: 'tick',
      sessionId: '-',
      lagMs: gap,
      batchMs: 0,
      lines: 0,
      vis,
    }).catch(() => {})
  }
}, 1000)
