<script setup lang="ts">
import { ref, watch } from 'vue'
import { useResizeObserver } from '@vueuse/core'

/**
 * 极简 canvas 折线面积图（监控迷你图用）。颜色传 CSS 变量名（如 'var(--accent)'），
 * 每次绘制时解析，主题切换后的下一个数据刷新周期自然换色。
 */
const props = withDefaults(
  defineProps<{
    values: number[]
    color?: string
    height?: number
  }>(),
  { color: 'var(--accent)', height: 34 },
)

const canvas = ref<HTMLCanvasElement | null>(null)

function draw() {
  const el = canvas.value
  if (!el) return
  const w = el.clientWidth
  const h = el.clientHeight
  if (w === 0 || h === 0) return
  const dpr = window.devicePixelRatio || 1
  el.width = Math.round(w * dpr)
  el.height = Math.round(h * dpr)
  const ctx = el.getContext('2d')
  if (!ctx) return
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  ctx.clearRect(0, 0, w, h)

  // 中线参考
  ctx.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue('--border-strong') || '#334155'
  ctx.lineWidth = 1
  ctx.beginPath()
  ctx.moveTo(0, h - 0.5)
  ctx.lineTo(w, h - 0.5)
  ctx.stroke()

  const vals = props.values
  if (vals.length === 0) return
  let max = 0
  for (const v of vals) if (v > max) max = v
  if (max === 0) return

  const color = getComputedStyle(document.documentElement).getPropertyValue(
    props.color.replace(/^var\((.+)\)$/, '$1').trim(),
  ) || props.color

  const x = (i: number) => (vals.length === 1 ? w : (i / (vals.length - 1)) * w)
  const y = (v: number) => h - 2 - (v / max) * (h - 4)

  ctx.beginPath()
  ctx.moveTo(x(0), y(vals[0]!))
  for (let i = 1; i < vals.length; i++) ctx.lineTo(x(i), y(vals[i]!))
  ctx.strokeStyle = color.trim()
  ctx.lineWidth = 1.5
  ctx.stroke()

  ctx.lineTo(x(vals.length - 1), h)
  ctx.lineTo(x(0), h)
  ctx.closePath()
  ctx.globalAlpha = 0.18
  ctx.fillStyle = color.trim()
  ctx.fill()
  ctx.globalAlpha = 1
}

watch([() => props.values, () => props.color], draw)
useResizeObserver(canvas, draw)
</script>

<template>
  <canvas ref="canvas" class="mini-chart" :style="{ height: height + 'px' }"></canvas>
</template>
