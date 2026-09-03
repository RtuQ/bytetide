<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, provide, ref } from 'vue'
import { emit, type UnlistenFn } from '@tauri-apps/api/event'
import { useSessionStore } from './stores/session'
import { useBridgeStore } from './stores/bridge'
import { setupEvents } from './composables/useTauriEvents'
import { useHighlighter, HIGHLIGHTER_KEY } from './composables/useHighlighter'
import { usePlotData, PLOT_DATA_KEY } from './composables/usePlotData'
import { useTheme } from './composables/useTheme'
import {
  usePanelState,
  loadCenterSplit,
  saveCenterSplit,
  SPLIT_MIN,
  SPLIT_MAX,
} from './composables/useLayoutPrefs'
import { DEFAULT_PLOT_CONFIG, DEFAULT_SEARCH } from './types'
import TitleBar from './components/TitleBar.vue'
import TabBar from './components/TabBar.vue'
import LogView from './components/LogView.vue'
import PlotView from './components/PlotView.vue'
import CompareView from './components/CompareView.vue'
import DockView from './components/DockView.vue'
import StatusBar from './components/StatusBar.vue'
import SearchPanel from './components/SearchPanel.vue'
import BookmarkPanel from './components/BookmarkPanel.vue'
import KeywordPanel from './components/KeywordPanel.vue'
import ParserPanel from './components/ParserPanel.vue'
import AutoReplyPanel from './components/AutoReplyPanel.vue'
import AlertPanel from './components/AlertPanel.vue'
import ConfigPresetsPanel from './components/ConfigPresetsPanel.vue'
import PlotConfigPanel from './components/PlotConfigPanel.vue'
import AiNotesPanel from './components/AiNotesPanel.vue'
import SendPanel from './components/SendPanel.vue'
import SplitView from './components/SplitView.vue'

const store = useSessionStore()
const bridge = useBridgeStore()
const unlistens = ref<UnlistenFn[]>([])

// 顶层共享高亮器：搜索负责“只看命中/次数/命中行列表”，
// 关键词为独立多色高亮（每个自带颜色）。子组件通过 inject 复用，避免重复全量扫描。
const highlighter = useHighlighter(
  () => store.active?.search ?? DEFAULT_SEARCH,
  () => store.active?.keywords ?? [],
  () => store.active?.lines ?? [],
  () => store.active?.lineCounter ?? 0,
)
provide(HIGHLIGHTER_KEY, highlighter)

// 顶层共享绘图解析器：仅当 plot.enabled 时解析，节流 300ms；
// PlotView 绘制与 PlotConfigPanel 展示统计共用，避免重复扫描全量行
const plotData = usePlotData(
  () => store.active?.plot ?? DEFAULT_PLOT_CONFIG,
  () => store.active?.lines ?? [],
  () => store.active?.lineCounter ?? 0,
)
provide(PLOT_DATA_KEY, plotData)

onMounted(async () => {
  useTheme()
  // 首帧就绪：通知 Rust 显示窗口（防白屏）。放 finally：启动链任何一步失败
  // 也要亮窗报错，不能把窗口藏死成「应用打不开」（浏览器冒烟无后端时静默）
  try {
    unlistens.value = await setupEvents()
    await store.refreshPorts()
    await bridge.load()
  } catch (e) {
    console.error('[startup] 启动链失败:', e)
  } finally {
    emit('app-ready').catch(() => {})
  }
})
onBeforeUnmount(() => {
  unlistens.value.forEach((f) => f())
  window.removeEventListener('mousemove', onSplitMove)
  window.removeEventListener('mouseup', onSplitEnd)
})

// ---- 视图四态（布局重构 V1）：viewbar 常驻于工具区之上，任何模式下都可切换/退出对比 ----
const activeView = computed(() => store.active?.centerView ?? 'log')
const compareOn = computed(() => store.compareMode)

const VIEW_ITEMS = [
  { key: 'log', label: '日志', title: '仅日志' },
  { key: 'split', label: '分屏', title: '日志与图表同屏，可拖分割条调整高度' },
  { key: 'plot', label: '图表', title: '仅图表' },
] as const

function setView(v: 'log' | 'split' | 'plot') {
  if (store.compareMode) store.toggleCompareMode()
  if (!store.activeId) return
  store.setCenterView(store.activeId, v)
}

// ---- 日志↔图表 分屏高度比（拖拽手柄逻辑对齐侧栏手柄的 moved-while-down 模式） ----
const splitPct = ref(loadCenterSplit())
const logWrapStyle = computed(() =>
  activeView.value === 'split' ? { flex: `0 0 ${splitPct.value}%` } : undefined,
)

let splitDragging = false
let splitMoved = false
function onSplitStart(e: MouseEvent) {
  e.preventDefault()
  splitDragging = true
  splitMoved = false
  document.body.classList.add('app-dragging')
  window.addEventListener('mousemove', onSplitMove)
  window.addEventListener('mouseup', onSplitEnd)
}
function onSplitMove(e: MouseEvent) {
  if (!splitDragging) return
  splitMoved = true
  const el = document.getElementById('center-body')
  if (!el) return
  const r = el.getBoundingClientRect()
  const pct = ((e.clientY - r.top) / r.height) * 100
  splitPct.value = Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, Math.round(pct)))
}
function onSplitEnd() {
  splitDragging = false
  document.body.classList.remove('app-dragging')
  window.removeEventListener('mousemove', onSplitMove)
  window.removeEventListener('mouseup', onSplitEnd)
  if (splitMoved) saveCenterSplit(splitPct.value)
}

// ---- 侧栏：左缘手柄拖拽调宽；点击手柄上的按钮收起/展开；状态 localStorage 记忆 ----
const SIDEBAR_KEY = 'serialtool.sidebar'
const SIDEBAR_MIN = 240
const SIDEBAR_MAX = 560
const SIDEBAR_DEFAULT = 312

type SidebarState = { width: number; collapsed: boolean }

function loadSidebar(): SidebarState {
  try {
    const raw = localStorage.getItem(SIDEBAR_KEY)
    if (raw) {
      const v = JSON.parse(raw) as Partial<SidebarState>
      const w = Number(v.width)
      return {
        width: Number.isFinite(w) && w > 0 ? Math.min(w, SIDEBAR_MAX) : SIDEBAR_DEFAULT,
        collapsed: v.collapsed === true,
      }
    }
  } catch {
    /* ignore */
  }
  return { width: SIDEBAR_DEFAULT, collapsed: false } // 默认展开
}

const sidebarWidth = ref<number>(loadSidebar().width)
const sidebarCollapsed = ref<boolean>(loadSidebar().collapsed)

function saveSidebar() {
  try {
    localStorage.setItem(
      SIDEBAR_KEY,
      JSON.stringify({ width: sidebarWidth.value, collapsed: sidebarCollapsed.value }),
    )
  } catch {
    /* ignore */
  }
}

function toggleSidebar() {
  sidebarCollapsed.value = !sidebarCollapsed.value
  saveSidebar()
}

let resizing = false
let movedWhileDown = false
function onResizeStart(e: MouseEvent) {
  // 按钮点击不触发拖拽
  if ((e.target as HTMLElement).closest('.sidebar-toggle')) return
  e.preventDefault()
  resizing = true
  movedWhileDown = false
  document.body.classList.add('sidebar-resizing')
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', onResizeEnd)
}
function onResizeMove(e: MouseEvent) {
  if (!resizing) return
  movedWhileDown = true
  // 手柄贴侧栏左缘：向左拖增大、向右拖减小，最小保留 SIDEBAR_MIN
  sidebarWidth.value = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, window.innerWidth - e.clientX))
}
function onResizeEnd() {
  resizing = false
  document.body.classList.remove('sidebar-resizing')
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', onResizeEnd)
  if (movedWhileDown) saveSidebar()
}
const sidebarStyle = computed(() =>
  sidebarCollapsed.value
    ? { width: '0px', flex: '0 0 0px' }
    : {
        width: `${sidebarWidth.value}px`,
        flex: `0 0 ${sidebarWidth.value}px`,
      },
)

// ---- 面板开合记忆：默认全收起，用户展开/收起即持久化（布局重构 V1） ----
const panel = usePanelState()
function onPanelToggle(e: Event, id: string) {
  // toggle 事件在 open 态变更后触发，读 DOM 当前值即用户意图
  panel.setOpen(id, (e.target as HTMLDetailsElement).open)
}
</script>

<template>
  <div class="app">
    <TitleBar />
    <TabBar />
    <div class="app-main">
      <SplitView v-if="store.splitMode" />
      <template v-else>
        <div class="app-center">
          <!-- 视图切换条：常驻（对比模式下也能一键切回，兼作对比退出） -->
          <div v-if="store.active" class="viewbar">
            <div class="seg" role="group" aria-label="中心视图模式">
              <button
                v-for="v in VIEW_ITEMS"
                :key="v.key"
                class="seg-item"
                :class="{ active: !compareOn && activeView === v.key }"
                :title="v.title"
                type="button"
                @click="setView(v.key)"
              >{{ v.label }}</button>
              <button
                class="seg-item"
                :class="{ active: compareOn }"
                title="双会话时间对齐对比（占中心区）"
                type="button"
                @click="store.toggleCompareMode()"
              >对比</button>
            </div>
          </div>
          <div id="center-body" class="center-body">
            <div class="log-wrap" :style="logWrapStyle">
              <LogView :session-id="store.activeId ?? ''" />
            </div>
            <div
              v-if="activeView === 'split' && !compareOn"
              class="hsplit"
              role="separator"
              aria-orientation="horizontal"
              aria-label="调整日志与图表高度"
              title="拖动调整日志/图表高度"
              @mousedown="onSplitStart"
            ></div>
            <div v-if="activeView !== 'log' && !compareOn" class="plot-wrap">
              <PlotView :session-id="store.activeId ?? ''" />
            </div>
            <CompareView v-if="compareOn" />
          </div>
          <DockView />
          <SendPanel />
        </div>
      </template>
      <!-- 侧栏与收放手柄在单列/分屏两种模式下都渲染 -->
      <div
        class="sidebar-handle"
        :class="{ collapsed: sidebarCollapsed }"
        title="拖动调整侧栏宽度"
        role="separator"
        aria-label="调整侧栏宽度"
        aria-orientation="vertical"
        @mousedown="onResizeStart"
      >
        <button
          class="sidebar-toggle"
          :class="{ collapsed: sidebarCollapsed }"
          :title="sidebarCollapsed ? '展开侧栏' : '收起侧栏'"
          :aria-label="sidebarCollapsed ? '展开侧栏' : '收起侧栏'"
          @click.stop="toggleSidebar"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <!-- 箭头指向侧栏移动方向：展开时点击向右收起，收起时点击向左展开 -->
            <polyline v-if="sidebarCollapsed" points="15 18 9 12 15 6" />
            <polyline v-else points="9 18 15 12 9 6" />
          </svg>
        </button>
      </div>
      <aside class="app-sidebar" :class="{ collapsed: sidebarCollapsed }" :style="sidebarStyle">
        <div class="group-head">查找</div>
        <SearchPanel :open="panel.isOpen('search')" @toggle="onPanelToggle($event, 'search')" />
        <BookmarkPanel :open="panel.isOpen('bookmarks')" @toggle="onPanelToggle($event, 'bookmarks')" />

        <div class="group-head">规则</div>
        <KeywordPanel :open="panel.isOpen('keywords')" @toggle="onPanelToggle($event, 'keywords')" />
        <ParserPanel :open="panel.isOpen('parser')" @toggle="onPanelToggle($event, 'parser')" />
        <AutoReplyPanel :open="panel.isOpen('autoreply')" @toggle="onPanelToggle($event, 'autoreply')" />
        <AlertPanel :open="panel.isOpen('alerts')" @toggle="onPanelToggle($event, 'alerts')" />

        <div class="group-head">数据</div>
        <PlotConfigPanel :open="panel.isOpen('plot')" @toggle="onPanelToggle($event, 'plot')" />

        <div class="group-head">库</div>
        <ConfigPresetsPanel :open="panel.isOpen('presets')" @toggle="onPanelToggle($event, 'presets')" />
        <AiNotesPanel :open="panel.isOpen('ainotes')" @toggle="onPanelToggle($event, 'ainotes')" />
      </aside>
    </div>
    <StatusBar />
  </div>
</template>
