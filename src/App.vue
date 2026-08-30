<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, provide, ref } from 'vue'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { useSessionStore } from './stores/session'
import { useBridgeStore } from './stores/bridge'
import { setupEvents } from './composables/useTauriEvents'
import { useHighlighter, HIGHLIGHTER_KEY } from './composables/useHighlighter'
import { usePlotData, PLOT_DATA_KEY } from './composables/usePlotData'
import { useTheme } from './composables/useTheme'
import { DEFAULT_PLOT_CONFIG, DEFAULT_SEARCH } from './types'
import PortBar from './components/PortBar.vue'
import TitleBar from './components/TitleBar.vue'
import TabBar from './components/TabBar.vue'
import LogView from './components/LogView.vue'
import PlotView from './components/PlotView.vue'
import SearchPanel from './components/SearchPanel.vue'
import BookmarkPanel from './components/BookmarkPanel.vue'
import KeywordPanel from './components/KeywordPanel.vue'
import AutoReplyPanel from './components/AutoReplyPanel.vue'
import AlertPanel from './components/AlertPanel.vue'
import ConfigPresetsPanel from './components/ConfigPresetsPanel.vue'
import ComparePanel from './components/ComparePanel.vue'
import PlotConfigPanel from './components/PlotConfigPanel.vue'
import BridgePanel from './components/BridgePanel.vue'
import MatchStats from './components/MatchStats.vue'
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
  unlistens.value = await setupEvents()
  await store.refreshPorts()
  await bridge.load()
})
onBeforeUnmount(() => {
  unlistens.value.forEach((f) => f())
})

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
</script>

<template>
  <div class="app">
    <TitleBar />
    <PortBar />
    <TabBar />
    <div class="app-main">
      <SplitView v-if="store.splitMode" />
      <template v-else>
        <div class="app-center">
          <PlotView v-if="store.active?.plot?.enabled" :session-id="store.activeId ?? ''" />
          <LogView v-else :session-id="store.activeId ?? ''" />
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
        <SearchPanel />
        <BookmarkPanel />
        <KeywordPanel />
        <AutoReplyPanel />
        <AlertPanel />
        <ConfigPresetsPanel />
        <ComparePanel />
        <PlotConfigPanel />
        <BridgePanel />
        <MatchStats />
      </aside>
    </div>
  </div>
</template>
