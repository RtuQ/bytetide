<script setup lang="ts">
import { computed, ref } from 'vue'
import { useAlertStore } from '../stores/alerts'
import {
  clampDockHeight,
  loadDockPrefs,
  saveDockPrefs,
  type DockTab,
} from '../composables/useLayoutPrefs'
import DockDecode from './DockDecode.vue'
import DockAlerts from './DockAlerts.vue'
import DockMonitor from './DockMonitor.vue'

/** 底部 dock 容器（docs/plan-layout-v1.md §2-③）：
 *  页签（解码 / 告警历史 / 监控）+ 上缘拖高 + 收起；状态持久化到 serialtool.dock。
 *  无 props / emits，由布局集成层挂载。 */
const alerts = useAlertStore()

const initial = loadDockPrefs(window.innerHeight)
const height = ref(initial.height)
const collapsed = ref(initial.collapsed)
const tab = ref<DockTab>(initial.tab)

const TABS: { key: DockTab; label: string }[] = [
  { key: 'decode', label: '解码' },
  { key: 'alerts', label: '告警历史' },
  { key: 'monitor', label: '监控' },
]

function persist() {
  saveDockPrefs({ height: height.value, collapsed: collapsed.value, tab: tab.value })
}

/** 告警页签角标：与列表口径一致的全量历史条数（0 不显示） */
const alertBadge = computed(() => alerts.hits.length)

/** 展开时高度走内联样式；收起时不绑定，由 .dock.collapsed 类收成页签条一行 */
const dockStyle = computed(() =>
  collapsed.value ? undefined : { height: `${height.value}px` },
)

function switchTab(key: DockTab) {
  if (tab.value === key) return
  tab.value = key
  persist()
}

function toggleCollapsed() {
  collapsed.value = !collapsed.value
  persist()
}

// 上缘拖高：moved-while-down 模式（同 App.vue 侧栏手柄），真正拖动过才持久化
const rootEl = ref<HTMLElement | null>(null)
let resizing = false
let movedWhileDown = false
let dragBottom = 0
function onResizeStart(e: MouseEvent) {
  e.preventDefault()
  resizing = true
  movedWhileDown = false
  // 以拖拽开始时 dock 底边为锚：拖动中 dock 上缘跟随鼠标、底边不动
  dragBottom = rootEl.value ? rootEl.value.getBoundingClientRect().bottom : window.innerHeight
  document.body.classList.add('dock-resizing')
  window.addEventListener('mousemove', onResizeMove)
  window.addEventListener('mouseup', onResizeEnd)
}
function onResizeMove(e: MouseEvent) {
  if (!resizing) return
  movedWhileDown = true
  height.value = clampDockHeight(dragBottom - e.clientY, window.innerHeight)
}
function onResizeEnd() {
  resizing = false
  document.body.classList.remove('dock-resizing')
  window.removeEventListener('mousemove', onResizeMove)
  window.removeEventListener('mouseup', onResizeEnd)
  if (movedWhileDown) persist()
}
</script>

<template>
  <section ref="rootEl" class="dock" :class="{ collapsed }" :style="dockStyle">
    <div
      class="dock-resize"
      title="拖动调整高度"
      role="separator"
      aria-label="调整底部面板高度"
      aria-orientation="horizontal"
      @mousedown="onResizeStart"
    ></div>
    <div class="dock-tabs">
      <button
        v-for="t in TABS"
        :key="t.key"
        class="dock-tab"
        :class="{ active: tab === t.key }"
        :title="t.label"
        :aria-pressed="tab === t.key"
        @click="switchTab(t.key)"
      >
        {{ t.label }}
        <span v-if="t.key === 'alerts' && alertBadge > 0" class="dock-tab-badge">{{
          alertBadge
        }}</span>
      </button>
      <span class="dock-tabs-spacer"></span>
      <button
        class="dock-toggle"
        :title="collapsed ? '展开 dock' : '收起 dock'"
        :aria-label="collapsed ? '展开底部面板' : '收起底部面板'"
        :aria-expanded="!collapsed"
        @click="toggleCollapsed"
      >
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2.5"
          stroke-linecap="round"
          stroke-linejoin="round"
        >
          <!-- 箭头指向内容边缘移动方向：展开时点击向上收起；收起后旋转 180° 指向下（展开） -->
          <polyline points="6 15 12 9 18 15" />
        </svg>
      </button>
    </div>
    <!-- 收起 = 不渲染 pane 内容（折叠早退红线，非 display:none 挂载态） -->
    <div v-if="!collapsed" class="dock-body">
      <DockDecode v-if="tab === 'decode'" />
      <DockAlerts v-else-if="tab === 'alerts'" />
      <DockMonitor v-else />
    </div>
  </section>
</template>
