import { computed, ref, watch, type InjectionKey } from 'vue'
import { useThrottleFn } from '@vueuse/core'
import { parseFrames } from './usePlotParser'
import type { LogLine, PlotConfig, PlotPoint } from '../types'

export interface PlotData {
  points: import('vue').Ref<PlotPoint[]>
  frameCount: import('vue').Ref<number>
  lastError: import('vue').Ref<string>
}

/**
 * 绘图数据 composable（在 App 顶层创建并 provide，供 PlotView 绘制与 PlotConfigPanel 展示统计共享，
 * 避免重复解析全量行）。
 * - 配置或行数变化时经 300ms 节流后台重解析
 * - 仅返回最近 maxPoints 个点用于绘制
 */
export function usePlotData(
  getConfig: () => PlotConfig,
  getLines: () => LogLine[],
  getVersion: () => number,
): PlotData {
  const configSig = computed(() => JSON.stringify(getConfig()))
  const points = ref<PlotPoint[]>([])
  const frameCount = ref(0)
  const lastError = ref('')

  const recompute = useThrottleFn(() => {
    const cfg = getConfig()
    if (!cfg.enabled) {
      points.value = []
      frameCount.value = 0
      lastError.value = ''
      return
    }
    const r = parseFrames(cfg, getLines())
    points.value = r.points
    frameCount.value = r.frameCount
    lastError.value = r.lastError
  }, 300)

  watch([configSig, getVersion], () => recompute(), { immediate: true })

  return { points, frameCount, lastError }
}

export const PLOT_DATA_KEY: InjectionKey<PlotData> = Symbol('plotData')
