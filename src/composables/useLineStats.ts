import { onScopeDispose, ref, watch } from 'vue'
import { useThrottleFn } from '@vueuse/core'
import type { LogLine } from '../types'

/** 一个 1 秒桶内的行数 */
export interface RateBucket {
  /** 桶起始 epoch 毫秒 */
  t: number
  lines: number
}

/** RX 相邻行 Δ间隔摘要（口径对齐 REST 桥 /timing：仅 RX、p95） */
export interface GapStats {
  count: number
  min: number
  avg: number
  p95: number
  max: number
}

const BUCKET_COUNT = 60
const GAP_SAMPLE_MAX = 200
/** Δ间隔统计只取最近 N 行：全量扫描+排序在 5 万行缓冲下每 300ms 一次会吃掉
 *  持续流式吞吐的一半以上（实测积压线性增长），而间隔分布看尾部已足够。 */
const GAP_LINES_MAX = 5000

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0
  const idx = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1)
  return sorted[idx]!
}

/** 日志时间轴上最近 60 个 1 秒桶的行速率（离线日志 epochMillis 为合成时间轴，同样适用） */
function computeLineHist(lines: LogLine[]): RateBucket[] {
  if (lines.length === 0) return []
  const last = lines[lines.length - 1]!.epochMillis
  const start = last - BUCKET_COUNT * 1000
  const map = new Map<number, number>()
  for (const l of lines) {
    const e = l.epochMillis
    if (e <= start || e > last) continue
    const k = Math.floor((e - start) / 1000)
    map.set(k, (map.get(k) ?? 0) + 1)
  }
  const out: RateBucket[] = []
  for (let i = 0; i < BUCKET_COUNT; i++) out.push({ t: start + i * 1000, lines: map.get(i) ?? 0 })
  return out
}

/** RX 相邻行间隔摘要 + 最近若干原始间隔样本（供迷你图画趋势）。
 *  仅扫尾部 GAP_LINES_MAX 行（对齐迷你图“尾部趋势”口径）。 */
export function computeGaps(lines: LogLine[]): { stats: GapStats; samples: number[] } {
  const from = Math.max(0, lines.length - GAP_LINES_MAX)
  const ds: number[] = []
  let prev = -1
  for (let i = from; i < lines.length; i++) {
    const l = lines[i]!
    if (l.dir !== 'rx') continue
    if (prev >= 0 && l.epochMillis >= prev) ds.push(l.epochMillis - prev)
    prev = l.epochMillis
  }
  let stats: GapStats = { count: 0, min: 0, avg: 0, p95: 0, max: 0 }
  if (ds.length > 0) {
    const sorted = [...ds].sort((a, b) => a - b)
    let sum = 0
    for (const d of ds) sum += d
    stats = {
      count: ds.length,
      min: sorted[0]!,
      avg: Math.round(sum / ds.length),
      p95: percentile(sorted, 95),
      max: sorted[sorted.length - 1]!,
    }
  }
  const samples = ds.length > GAP_SAMPLE_MAX ? ds.slice(ds.length - GAP_SAMPLE_MAX) : ds
  return { stats, samples }
}

/**
 * 会话日志的实时监控统计。仿 useHighlighter：以 getVersion()（lineCounter）
 * 为版本信号、节流 300ms 重算；字节速率曲线独立 1s 采样（与会话切换解耦，
 * 切到累计值更小的会话时自动归零基线）。
 */
export function useLineStats(
  getLines: () => LogLine[],
  getTotalBytes: () => number,
  getVersion: () => number,
) {
  const lineHist = ref<RateBucket[]>([])
  const gapStats = ref<GapStats>({ count: 0, min: 0, avg: 0, p95: 0, max: 0 })
  const gapSamples = ref<number[]>([])
  const byteHist = ref<number[]>([])

  const recompute = useThrottleFn(
    () => {
      const lines = getLines()
      lineHist.value = computeLineHist(lines)
      const g = computeGaps(lines)
      gapStats.value = g.stats
      gapSamples.value = g.samples
    },
    300,
    true,
  )
  watch(getVersion, recompute, { immediate: true })

  let prevTotal = getTotalBytes()
  const timer = window.setInterval(() => {
    const t = getTotalBytes()
    let d = 0
    if (t >= prevTotal) d = t - prevTotal
    prevTotal = t
    byteHist.value = [...byteHist.value.slice(-(BUCKET_COUNT - 1)), d]
  }, 1000)
  onScopeDispose(() => window.clearInterval(timer))

  return { lineHist, gapStats, gapSamples, byteHist }
}
