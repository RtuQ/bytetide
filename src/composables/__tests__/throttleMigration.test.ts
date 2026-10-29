import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'
import { DEFAULT_PLOT_CONFIG } from '../../types'
import { useHighlighter } from '../useHighlighter'
import { usePlotData } from '../usePlotData'

afterEach(() => vi.useRealTimers())

describe('节流行为兼容', () => {
  it.each(['高亮', '图表'])('%s 在节流窗口内不会追加尾次扫描', async kind => {
    vi.useFakeTimers()
    vi.setSystemTime(10_000)
    const version = ref(0)
    const getLines = vi.fn(() => [])
    const scope = effectScope()
    scope.run(() => {
      if (kind === '高亮') {
        useHighlighter(() => ({ pattern: 'err', useRegex: false, caseSensitive: false, wholeWord: false }), () => [], getLines, () => version.value)
      } else {
        usePlotData(() => ({ ...DEFAULT_PLOT_CONFIG, enabled: true }), getLines, () => version.value)
      }
    })
    expect(getLines).toHaveBeenCalledTimes(1)
    version.value++
    await nextTick()
    await vi.advanceTimersByTimeAsync(300)
    expect(getLines).toHaveBeenCalledTimes(1)
    scope.stop()
  })
})
