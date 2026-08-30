import { onBeforeUnmount, ref, type Ref } from 'vue'

/** 人类可读字节数：B / KB / MB（1KB=1024）；非有限值按 0 处理 */
export function humanizeBytes(n: number): string {
  if (!Number.isFinite(n)) n = 0
  if (n < 1024) return `${n}B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}KB`
  return `${(n / 1024 / 1024).toFixed(1)}MB`
}

/** 人类可读毫秒差：<1s 显 ms，<60s 显 x.xs，否则 x.xm；非有限值显 '-' */
export function humanizeMs(ms: number): string {
  if (!Number.isFinite(ms)) return '-'
  if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`
  const s = ms / 1000
  if (s < 60) return `${s.toFixed(1)}s`
  return `${(s / 60).toFixed(1)}m`
}

/**
 * 滚动速率采样：每 intervalMs 采样一次 getValue()，bps = 本周期增量。
 * 用于“最近 1 秒字节数”即 bytes/s。组件卸载时自动清 interval。
 */
export function useRate(getValue: () => number, intervalMs = 1000): Ref<number> {
  const bps = ref(0)
  const start = getValue()
  let prev = Number.isFinite(start) ? start : 0
  let timer: number | null = window.setInterval(() => {
    const cur = getValue()
    const d = cur - prev
    bps.value = Number.isFinite(d) && d > 0 ? d : 0
    if (Number.isFinite(cur)) prev = cur
  }, intervalMs)
  onBeforeUnmount(() => {
    if (timer !== null) {
      clearInterval(timer)
      timer = null
    }
  })
  return bps
}
