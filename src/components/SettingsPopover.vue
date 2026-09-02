<script setup lang="ts">
import { computed, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { theme, toggleTheme } from '../composables/useTheme'
import { useBridgeStore } from '../stores/bridge'
import LogSettingsPanel from './LogSettingsPanel.vue'
import BridgeSettings from './BridgeSettings.vue'
import PresetsPanel from './PresetsPanel.vue'
import type { PortConfig } from '../types'

/** 设置弹层：公用功能按 日志/集成/视图 归类。二级内容在同一浮层内切换视图（无嵌套弹层，不遮挡）。 */
defineProps<{ cfg: PortConfig }>()
const emit = defineEmits<{ 'open-log': []; 'apply-preset': [config: PortConfig] }>()

const store = useSessionStore()
const bridge = useBridgeStore()
const open = ref(false)
type View = 'menu' | 'log' | 'bridge' | 'presets'
const view = ref<View>('menu')

const titles: Record<View, string> = {
  menu: '设置',
  log: '日志设置',
  bridge: 'REST 桥接',
  presets: '连接配置预设',
}
const title = computed(() => titles[view.value])

function toggle() {
  open.value = !open.value
  if (!open.value) view.value = 'menu'
}
function close() {
  open.value = false
  view.value = 'menu'
}
function back() {
  view.value = 'menu'
}
function openLog() {
  emit('open-log')
  close()
}
function toggleSplit() {
  store.splitMode ? store.exitSplit() : store.enterSplit()
}
function applyPreset(c: PortConfig) {
  emit('apply-preset', c)
}
</script>

<template>
  <button
    class="btn btn-ghost btn-icon"
    type="button"
    :class="{ 'is-active': open }"
    title="设置"
    aria-label="设置"
    @click="toggle"
  >
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
  </button>

  <div v-if="open" class="portbar-pop right sm-pop" @click.stop>
    <div class="portbar-pop-head">
      <button v-if="view !== 'menu'" class="btn btn-ghost btn-icon btn-sm" type="button" title="返回" aria-label="返回" @click="back">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>
      </button>
      <span>{{ title }}</span>
      <button class="btn btn-ghost btn-icon btn-sm" type="button" title="关闭" aria-label="关闭" @click="close">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
      </button>
    </div>

    <!-- 菜单视图 -->
    <div v-if="view === 'menu'">
      <div class="sm-sec">日志</div>
      <button class="sm-item" type="button" @click="openLog">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
        <span>打开日志文件<span class="sm-sub">离线加载 .log 进行分析</span></span>
      </button>
      <button class="sm-item" type="button" @click="view = 'log'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/></svg>
        <span>日志设置<span class="sm-sub">路径模板 · 行时间戳格式</span></span>
      </button>
      <div class="sm-sep"></div>
      <div class="sm-sec">集成</div>
      <button class="sm-item" type="button" @click="view = 'bridge'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>
        <span style="flex: 1">REST 桥接<span class="sm-sub">外部 AI 经 HTTP 读取/分析日志</span></span>
        <span v-if="bridge.running" class="bridge-dot" title="桥服务运行中"></span>
      </button>
      <button class="sm-item" type="button" @click="view = 'presets'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
        <span>连接配置预设<span class="sm-sub">端口参数快捷切换</span></span>
      </button>
      <div class="sm-sep"></div>
      <div class="sm-sec">视图</div>
      <button class="sm-item" type="button" @click="toggleSplit">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="18" rx="1"/><rect x="14" y="3" width="7" height="18" rx="1"/></svg>
        <span style="flex: 1">{{ store.splitMode ? '退出分屏，回到单列' : '分屏' }}<span class="sm-sub">多端口并列查看</span></span>
      </button>
      <button class="sm-item" type="button" @click="toggleTheme()">
        <svg v-if="theme === 'dark'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>
        <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        <span>{{ theme === 'dark' ? '切换到亮色主题' : '切换到深色主题' }}</span>
      </button>
    </div>

    <!-- 二级视图：同浮层切换，无嵌套遮挡 -->
    <template v-else-if="view === 'log'">
      <LogSettingsPanel />
    </template>
    <template v-else-if="view === 'bridge'">
      <BridgeSettings />
    </template>
    <template v-else-if="view === 'presets'">
      <PresetsPanel :current="cfg" @apply="applyPreset" />
    </template>
  </div>
</template>
