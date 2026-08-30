<script setup lang="ts">
import { computed, inject, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useResizeObserver } from '@vueuse/core'
import { useSessionStore } from '../stores/session'
import { PLOT_DATA_KEY } from '../composables/usePlotData'
import { theme } from '../composables/useTheme'
import { KEYWORD_PALETTE, type PlotPoint } from '../types'

const props = defineProps<{ sessionId: string }>()
const store = useSessionStore()
const session = computed(() => store.sessions[props.sessionId] ?? null)

const { points } = inject(PLOT_DATA_KEY)!

const wrapEl = ref<HTMLDivElement | null>(null)
const canvasEl = ref<HTMLCanvasElement | null>(null)

// 视图变换：null 表示该轴自适应（X=全幅，Y=自适应量程）
const xLo = ref<number | null>(null)
const xHi = ref<number | null>(null)
const yLo = ref<number | null>(null)
const yHi = ref<number | null>(null)

// 测量游标
const measureMode = ref(false)
const cursorA = ref<PlotPoint | null>(null)
const cursorB = ref<PlotPoint | null>(null)

const hover = ref(-1)
const mousePos = ref({ x: 0, y: 0 })
const dragging = ref(false)
const dragState = { lastX: 0, lastY: 0, moved: false }

const PAD_L = 50
const PAD_R = 10
const PAD_T = 10
const PAD_B = 28

let ctx: CanvasRenderingContext2D | null = null
let cssW = 0
let cssH = 0
let dpr = 1
let rafId = 0
let onWheelBound: ((e: WheelEvent) => void) | null = null

interface Palette {
  grid: string
  axis: string
  text: string
  fontMono: string
  fontUi: string
  rx: string
  tx: string
  accent: string
}
let palette: Palette | null = null
function ensurePalette(): Palette {
  if (palette) return palette
  const s = getComputedStyle(document.documentElement)
  palette = {
    grid: s.getPropertyValue('--border').trim() || '#1e293b',
    axis: s.getPropertyValue('--text-dim').trim() || '#64748b',
    text: s.getPropertyValue('--text-muted').trim() || '#94a3b8',
    fontMono: s.getPropertyValue('--font-mono').trim() || 'monospace',
    fontUi: s.getPropertyValue('--font-ui').trim() || 'system-ui, sans-serif',
    rx: s.getPropertyValue('--rx').trim() || '#34d399',
    tx: s.getPropertyValue('--tx').trim() || '#fbbf24',
    accent: s.getPropertyValue('--accent').trim() || '#3b82f6',
  }
  return palette
}
function channelColor(i: number): string {
  const p = ensurePalette()
  if (i === 0) return p.rx
  if (i === 1) return p.tx
  return KEYWORD_PALETTE[(i - 2) % KEYWORD_PALETTE.length]
}

const numChannels = computed(() => {
  const pts = points.value
  return pts.length ? pts[0].values.length : 0
})

const legend = computed(() => {
  const pts = points.value
  const n = numChannels.value
  if (!pts.length || n === 0) return []
  const out: { color: string; cur: number; min: number; max: number }[] = []
  for (let ch = 0; ch < n; ch++) {
    let min = Infinity
    let max = -Infinity
    for (const p of pts) {
      const v = p.values[ch] ?? 0
      if (v < min) min = v
      if (v > max) max = v
    }
    out.push({ color: channelColor(ch), cur: pts[pts.length - 1].values[ch] ?? 0, min, max })
  }
  return out
})

const fullIdxRange = computed(() => {
  const pts = points.value
  if (!pts.length) return { lo: 0, hi: 1 }
  return { lo: pts[0].idx, hi: pts[pts.length - 1].idx }
})

const xView = computed(() => {
  const f = fullIdxRange.value
  const lo = xLo.value ?? f.lo
  const hi = xHi.value ?? f.hi
  return { lo: Math.min(lo, hi), hi: Math.max(lo, hi) }
})

const visiblePoints = computed(() => {
  const pts = points.value
  const v = xView.value
  return pts.filter((p) => p.idx >= v.lo && p.idx <= v.hi)
})

const autoYRange = computed(() => {
  const vp = visiblePoints.value
  let min = Infinity
  let max = -Infinity
  for (const p of vp) {
    for (const v of p.values) {
      if (v < min) min = v
      if (v > max) max = v
    }
  }
  if (!isFinite(min)) return { min: 0, max: 1 }
  if (min === max) {
    min -= 1
    max += 1
  }
  const pad = (max - min) * 0.1 || 1
  return { min: min - pad, max: max + pad }
})

const yView = computed(() => {
  const a = autoYRange.value
  return { lo: yLo.value ?? a.min, hi: yHi.value ?? a.max }
})

// 自适应 Y 开关：勾选=清除手动 Y 量程；取消=冻结当前自适应量程
const autoY = computed<boolean>({
  get: () => yLo.value === null,
  set: (v) => {
    if (v) {
      yLo.value = null
      yHi.value = null
    } else {
      const a = autoYRange.value
      yLo.value = a.min
      yHi.value = a.max
    }
  },
})

const measure = computed(() => {
  const a = cursorA.value
  const b = cursorB.value
  if (!a || !b) return null
  const dPts = b.idx - a.idx
  const dMs = b.epochMillis - a.epochMillis
  const len = Math.max(a.values.length, b.values.length)
  const dVals: number[] = []
  for (let i = 0; i < len; i++) dVals.push((b.values[i] ?? 0) - (a.values[i] ?? 0))
  return { a, b, dPts, dMs, dVals }
})

const hoverPoint = computed<PlotPoint | null>(() => {
  const h = hover.value
  if (h < 0) return null
  return points.value[h] ?? null
})

const tooltipStyle = computed(() => {
  const mp = mousePos.value
  const w = 210
  const h = 96
  let left = mp.x + 14
  if (left + w > cssW) left = mp.x - w - 14
  if (left < 4) left = 4
  let top = mp.y + 14
  if (top + h > cssH) top = mp.y - h - 14
  if (top < 4) top = 4
  return { left: left + 'px', top: top + 'px' }
})

const canvasCursor = computed(() => (measureMode.value ? 'crosshair' : dragging.value ? 'grabbing' : 'grab'))

function plotRect() {
  return { x: PAD_L, y: PAD_T, w: cssW - PAD_L - PAD_R, h: cssH - PAD_T - PAD_B }
}

function resizeCanvas() {
  const c = canvasEl.value
  const wrap = wrapEl.value
  if (!c || !wrap) return
  dpr = window.devicePixelRatio || 1
  cssW = wrap.clientWidth
  cssH = wrap.clientHeight
  c.width = Math.max(1, Math.floor(cssW * dpr))
  c.height = Math.max(1, Math.floor(cssH * dpr))
  c.style.width = cssW + 'px'
  c.style.height = cssH + 'px'
  ctx = c.getContext('2d')
  if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
  scheduleDraw()
}

useResizeObserver(wrapEl, resizeCanvas)

function scheduleDraw() {
  if (rafId) return
  rafId = requestAnimationFrame(() => {
    rafId = 0
    draw()
  })
}

function draw() {
  const c = ctx
  if (!c) return
  const pal = ensurePalette()
  c.clearRect(0, 0, cssW, cssH)
  const plotW = cssW - PAD_L - PAD_R
  const plotH = cssH - PAD_T - PAD_B
  if (plotW <= 0 || plotH <= 0) return

  const pts = points.value
  const n = pts.length
  const padL = PAD_L
  const padT = PAD_T

  const xv = xView.value
  const yv = yView.value
  const xSpan = Math.max(1e-6, xv.hi - xv.lo)
  const ySpan = Math.max(1e-6, yv.hi - yv.lo)
  const xOf = (idx: number) => padL + ((idx - xv.lo) / xSpan) * plotW
  const yOf = (v: number) => padT + (1 - (v - yv.lo) / ySpan) * plotH

  // 网格 + Y 刻度（十进制点值）
  c.font = '11px ' + pal.fontMono
  c.textBaseline = 'middle'
  c.textAlign = 'right'
  const yTicks = 6
  for (let i = 0; i <= yTicks; i++) {
    const t = yv.lo + (i / yTicks) * ySpan
    const y = yOf(t)
    c.strokeStyle = pal.grid
    c.lineWidth = 1
    c.beginPath()
    c.moveTo(padL, y)
    c.lineTo(padL + plotW, y)
    c.stroke()
    c.fillStyle = pal.text
    c.fillText(String(Math.round(t)), padL - 6, y)
  }

  // X 刻度（点序号）
  c.textAlign = 'center'
  c.textBaseline = 'top'
  const xTicks = 8
  for (let i = 0; i <= xTicks; i++) {
    const idx = Math.round(xv.lo + (i / xTicks) * xSpan)
    const x = xOf(idx)
    c.strokeStyle = pal.grid
    c.lineWidth = 1
    c.beginPath()
    c.moveTo(x, padT)
    c.lineTo(x, padT + plotH)
    c.stroke()
    c.fillStyle = pal.text
    c.fillText(String(idx), x, padT + plotH + 6)
  }

  // 轴线
  c.strokeStyle = pal.axis
  c.lineWidth = 1
  c.beginPath()
  c.moveTo(padL, padT)
  c.lineTo(padL, padT + plotH)
  c.lineTo(padL + plotW, padT + plotH)
  c.stroke()

  // 多通道折线（裁剪到绘图区，避免溢出轴线）
  const nch = numChannels.value
  if (n && nch) {
    c.save()
    c.beginPath()
    c.rect(padL, padT, plotW, plotH)
    c.clip()
    for (let ch = 0; ch < nch; ch++) {
      c.strokeStyle = channelColor(ch)
      c.lineWidth = 1.5
      c.lineJoin = 'round'
      c.beginPath()
      let started = false
      for (let i = 0; i < n; i++) {
        const p = pts[i]
        if (p.idx < xv.lo || p.idx > xv.hi) continue
        const x = xOf(p.idx)
        const y = yOf(p.values[ch] ?? 0)
        if (!started) {
          c.moveTo(x, y)
          started = true
        } else {
          c.lineTo(x, y)
        }
      }
      c.stroke()
    }
    c.restore()
  }

  // 悬停十字线
  const h = hover.value
  if (h >= 0 && h < n) {
    const hp = pts[h]
    const x = xOf(hp.idx)
    if (x >= padL && x <= padL + plotW) {
      c.strokeStyle = pal.axis
      c.lineWidth = 1
      c.setLineDash([3, 3])
      c.beginPath()
      c.moveTo(x, padT)
      c.lineTo(x, padT + plotH)
      c.stroke()
      c.setLineDash([])
    }
  }

  // 测量游标 A / B
  drawCursor(cursorA.value, pal.accent, 'A')
  drawCursor(cursorB.value, pal.tx, 'B')
}

function drawCursor(p: PlotPoint | null, color: string, label: string) {
  const c = ctx
  if (!c || !p) return
  const pal = ensurePalette()
  const r = plotRect()
  if (r.w <= 0) return
  const xv = xView.value
  const x = r.x + ((p.idx - xv.lo) / Math.max(1e-6, xv.hi - xv.lo)) * r.w
  if (x < r.x - 1 || x > r.x + r.w + 1) return
  c.strokeStyle = color
  c.lineWidth = 1
  c.setLineDash([5, 3])
  c.beginPath()
  c.moveTo(x, r.y)
  c.lineTo(x, r.y + r.h)
  c.stroke()
  c.setLineDash([])
  c.fillStyle = color
  c.font = '10px ' + pal.fontMono
  c.textAlign = 'left'
  c.textBaseline = 'top'
  c.fillText(label + ' #' + p.idx, x + 3, r.y + 2)
}

function nearestPointByX(targetIdx: number): number {
  const pts = points.value
  if (!pts.length) return -1
  let lo = 0
  let hi = pts.length - 1
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    if (pts[mid].idx < targetIdx) lo = mid + 1
    else hi = mid
  }
  let best = lo
  if (lo > 0 && Math.abs(pts[lo - 1].idx - targetIdx) <= Math.abs(pts[best].idx - targetIdx)) best = lo - 1
  return best
}

function onMove(e: MouseEvent) {
  const c = canvasEl.value
  if (!c) return
  const rect = c.getBoundingClientRect()
  const mx = e.clientX - rect.left
  const my = e.clientY - rect.top
  mousePos.value = { x: mx, y: my }
  if (dragging.value) {
    const dx = e.clientX - dragState.lastX
    const dy = e.clientY - dragState.lastY
    if (!dragState.moved && Math.abs(dx) + Math.abs(dy) > 3) dragState.moved = true
    if (dragState.moved && !measureMode.value) {
      applyPan(dx, dy)
      dragState.lastX = e.clientX
      dragState.lastY = e.clientY
    }
    return
  }
  const r = plotRect()
  if (mx < r.x || mx > r.x + r.w) {
    hover.value = -1
    return
  }
  const xv = xView.value
  const target = xv.lo + ((mx - r.x) / Math.max(1, r.w)) * (xv.hi - xv.lo)
  hover.value = nearestPointByX(target)
}

function applyPan(dx: number, dy: number) {
  const pts = points.value
  if (!pts.length) return
  const r = plotRect()
  const f = fullIdxRange.value
  // 进入手动视图
  if (xLo.value === null) {
    xLo.value = f.lo
    xHi.value = f.hi
  }
  if (yLo.value === null) {
    const a = autoYRange.value
    yLo.value = a.min
    yHi.value = a.max
  }
  const xspan = Math.max(1e-6, (xHi.value ?? 0) - (xLo.value ?? 0))
  const yspan = Math.max(1e-6, (yHi.value ?? 0) - (yLo.value ?? 0))
  const dataPerPxX = r.w > 0 ? xspan / r.w : 0
  const dataPerPxY = r.h > 0 ? yspan / r.h : 0
  // 抓取语义：拖动方向 = 内容跟随方向
  let nLo = (xLo.value ?? 0) - dx * dataPerPxX
  let nHi = (xHi.value ?? 0) - dx * dataPerPxX
  const spanX = nHi - nLo
  if (nLo < f.lo) {
    nLo = f.lo
    nHi = f.lo + spanX
  }
  if (nHi > f.hi) {
    nHi = f.hi
    nLo = f.hi - spanX
  }
  xLo.value = nLo
  xHi.value = nHi
  yLo.value = (yLo.value ?? 0) + dy * dataPerPxY
  yHi.value = (yHi.value ?? 0) + dy * dataPerPxY
}

function onWheel(e: WheelEvent) {
  const pts = points.value
  if (!pts.length) return
  e.preventDefault()
  const c = canvasEl.value
  if (!c) return
  const rect = c.getBoundingClientRect()
  const mx = e.clientX - rect.left
  const my = e.clientY - rect.top
  const r = plotRect()
  if (r.w <= 0 || r.h <= 0) return
  const factor = e.deltaY < 0 ? 0.8 : 1.25
  const f = fullIdxRange.value
  if (e.ctrlKey || e.metaKey) {
    // Y 缩放（围绕光标）
    const a = autoYRange.value
    const ylo = yLo.value ?? a.min
    const yhi = yHi.value ?? a.max
    const yspan = Math.max(1e-6, yhi - ylo)
    const fy = (my - r.y) / r.h
    const v = ylo + (1 - fy) * yspan
    const newSpan = yspan * factor
    if (newSpan >= (a.max - a.min) * 6) {
      yLo.value = null
      yHi.value = null
    } else {
      yLo.value = v - (1 - fy) * newSpan
      yHi.value = yLo.value + newSpan
    }
  } else {
    // X 缩放（围绕光标）
    const xv = xView.value
    const xspan = Math.max(1e-6, xv.hi - xv.lo)
    const fx = (mx - r.x) / r.w
    const D = xv.lo + fx * xspan
    const newSpan = xspan * factor
    const fullSpan = Math.max(1, f.hi - f.lo)
    if (newSpan >= fullSpan) {
      xLo.value = null
      xHi.value = null
    } else {
      let nLo = D - fx * newSpan
      let nHi = nLo + newSpan
      if (nLo < f.lo) {
        nLo = f.lo
        nHi = nLo + newSpan
      }
      if (nHi > f.hi) {
        nHi = f.hi
        nLo = nHi - newSpan
      }
      xLo.value = nLo
      xHi.value = nHi
    }
  }
  scheduleDraw()
}

function onDown(e: MouseEvent) {
  if (e.button !== 0) return
  dragging.value = true
  dragState.lastX = e.clientX
  dragState.lastY = e.clientY
  dragState.moved = false
}

function onUp() {
  if (!dragging.value) return
  const moved = dragState.moved
  dragging.value = false
  if (!moved && measureMode.value) pinCursor()
}

function onLeave() {
  hover.value = -1
  dragging.value = false
}

function onDblClick() {
  resetView()
}

function pinCursor() {
  const pts = points.value
  if (!pts.length) return
  const mp = mousePos.value
  const r = plotRect()
  if (mp.x < r.x || mp.x > r.x + r.w) return
  const xv = xView.value
  const target = xv.lo + ((mp.x - r.x) / Math.max(1, r.w)) * (xv.hi - xv.lo)
  const best = nearestPointByX(target)
  if (best < 0) return
  const p = pts[best]
  if (!cursorA.value) cursorA.value = p
  else if (!cursorB.value) cursorB.value = p
  else {
    cursorA.value = p
    cursorB.value = null
  }
}

function resetView() {
  xLo.value = null
  xHi.value = null
  yLo.value = null
  yHi.value = null
  cursorA.value = null
  cursorB.value = null
}

function onMeasureToggle(e: Event) {
  measureMode.value = (e.target as HTMLInputElement).checked
  if (!measureMode.value) {
    cursorA.value = null
    cursorB.value = null
  }
}

function formatMs(ms: number): string {
  const sign = ms >= 0 ? '+' : '-'
  const a = Math.abs(ms)
  if (a >= 1000) return sign + (a / 1000).toFixed(3) + 's'
  return sign + a + 'ms'
}

watch([points, xLo, xHi, yLo, yHi, hover, cursorA, cursorB], scheduleDraw)

// 切换主题时清除 token 缓存（从 <html> 重新读取）并重绘
watch(theme, () => {
  palette = null
  scheduleDraw()
})

onMounted(() => {
  resizeCanvas()
  const c = canvasEl.value
  if (c) {
    onWheelBound = onWheel
    c.addEventListener('wheel', onWheelBound, { passive: false })
  }
})
onBeforeUnmount(() => {
  if (rafId) cancelAnimationFrame(rafId)
  if (onWheelBound && canvasEl.value) canvasEl.value.removeEventListener('wheel', onWheelBound)
})
</script>

<template>
  <div class="plot-view" v-if="session">
    <div class="plot-toolbar">
      <div class="plot-legend">
        <div v-for="(lg, i) in legend" :key="i" class="plot-chip">
          <span class="plot-swatch" :style="{ background: lg.color }"></span>
          <span class="plot-ch-name">Ch{{ i }}</span>
          <span class="plot-ch-cur">{{ lg.cur }}</span>
          <span class="plot-ch-mm">{{ lg.min }} ~ {{ lg.max }}</span>
        </div>
      </div>
      <div class="bar-spacer"></div>
      <div class="bar-group">
        <label class="check" title="Y 轴自适应量程（关闭后可手动缩放/平移）">
          <input type="checkbox" :checked="autoY" @change="autoY = ($event.target as HTMLInputElement).checked" />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>自适应</span>
        </label>
        <label class="check" title="测量模式：点击放置双游标，显示 Δ 值与时间差">
          <input type="checkbox" :checked="measureMode" @change="onMeasureToggle" />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>测量</span>
        </label>
        <button class="btn btn-ghost btn-sm" title="重置缩放/平移并清除游标（双击画布亦可）" @click="resetView">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v5h-5"/></svg>
          <span>重置</span>
        </button>
        <button class="btn btn-ghost btn-sm" title="清屏（同时清空日志与绘图数据）" @click="store.clearLog(props.sessionId)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21"/><path d="M22 21H7"/><path d="m5 11 9 9"/></svg>
          <span>清空</span>
        </button>
        <button class="btn btn-ghost btn-sm" title="关闭绘图，返回日志视图" @click="store.setPlotEnabled(props.sessionId, false)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="m19 9-5 5-4-4-3 3"/></svg>
          <span>返回日志</span>
        </button>
      </div>
    </div>

    <div class="plot-canvas-wrap" ref="wrapEl">
      <canvas
        ref="canvasEl"
        :style="{ cursor: canvasCursor }"
        @mousemove="onMove"
        @mousedown.prevent="onDown"
        @mouseup="onUp"
        @mouseleave="onLeave"
        @dblclick="onDblClick"
      ></canvas>
      <div v-if="hoverPoint" class="plot-tooltip" :style="tooltipStyle">
        <div class="tt-row tt-head">点 #{{ hoverPoint.idx }} · {{ hoverPoint.ts }}</div>
        <div class="tt-row tt-mono">原始 {{ hoverPoint.rawHex }}</div>
        <div v-for="(v, i) in hoverPoint.values" :key="i" class="tt-row">
          <span class="tt-dot" :style="{ background: channelColor(i) }"></span>
          <span>Ch{{ i }}: {{ v }}</span>
        </div>
      </div>
      <div v-if="measure" class="plot-measure">
        <div class="pm-head">测量 A#{{ measure.a.idx }} → B#{{ measure.b.idx }}</div>
        <div class="pm-row">Δ点 <b>{{ measure.dPts }}</b></div>
        <div class="pm-row">Δ时间 <b>{{ formatMs(measure.dMs) }}</b></div>
        <div v-for="(dv, i) in measure.dVals" :key="i" class="pm-row">
          <span class="tt-dot" :style="{ background: channelColor(i) }"></span>
          ΔCh{{ i }} <b>{{ dv >= 0 ? '+' : '' }}{{ dv }}</b>
        </div>
      </div>
      <div v-if="!points.length" class="plot-empty">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="m19 9-5 5-4-4-3 3"/></svg>
        <span>等待帧数据…</span>
      </div>
    </div>
  </div>
  <div v-else class="plot-view">
    <div class="plot-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="m19 9-5 5-4-4-3 3"/></svg>
      <span>打开一个串口并开启绘图开始</span>
    </div>
  </div>
</template>
