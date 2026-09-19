<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { theme, toggleTheme } from '../composables/useTheme'
import { locale, setLocale, t } from '../i18n'
import type { MessageKey } from '../i18n'
import { useNotificationPrefs } from '../composables/useNotificationPrefs'
import { useBridgeStore } from '../stores/bridge'
import LogSettingsPanel from './LogSettingsPanel.vue'
import BridgeSettings from './BridgeSettings.vue'
import PresetsPanel from './PresetsPanel.vue'
import type { PortConfig } from '../types'
import { POPOVER_EVENT, requestPopover } from '../composables/usePopoverBridge'

/** 设置弹层：公用功能按 日志/集成/视图 归类。二级内容在同一浮层内切换视图（无嵌套弹层，不遮挡）。 */
defineProps<{ cfg: PortConfig }>()
const emit = defineEmits<{ 'open-log': []; 'apply-preset': [config: PortConfig] }>()

const store = useSessionStore()
const bridge = useBridgeStore()
const notif = useNotificationPrefs()
const open = ref(false)
type View = 'menu' | 'log' | 'bridge' | 'presets'
const view = ref<View>('menu')

// 视图标题：值存 MessageKey，computed 内 t() 求值（切语言即时刷新）
const titles: Record<View, MessageKey> = {
  menu: 'set.title',
  log: 'set.logSettings',
  bridge: 'set.bridge',
  presets: 'set.presets',
}
const title = computed(() => t(titles[view.value]))

function toggle() {
  open.value = !open.value
  if (!open.value) view.value = 'menu'
  if (open.value) requestPopover('settings')
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
/** 通知总开关（.switch 的 checkbox → 偏好回写） */
function setNotif(ev: Event) {
  notif.setEnabled((ev.target as HTMLInputElement).checked)
}
/** 语言切换（zh ↔ en 循环，与主题按钮同形态） */
function toggleLocale() {
  setLocale(locale.value === 'zh-CN' ? 'en' : 'zh-CN')
}
function applyPreset(c: PortConfig) {
  emit('apply-preset', c)
}

function onPopover(event: Event) {
  const name = (event as CustomEvent<string>).detail
  if (name === 'settings') open.value = true
  else close()
}
function onDocumentPointerDown(event: PointerEvent) {
  const target = event.target as HTMLElement
  if (!target.closest('.sm-pop') && !target.closest('.titlebar-actions')) close()
}
onMounted(() => {
  window.addEventListener(POPOVER_EVENT, onPopover)
  document.addEventListener('pointerdown', onDocumentPointerDown)
})
onBeforeUnmount(() => {
  window.removeEventListener(POPOVER_EVENT, onPopover)
  document.removeEventListener('pointerdown', onDocumentPointerDown)
})
</script>

<template>
  <button
    class="btn btn-ghost btn-icon"
    type="button"
    :class="{ 'is-active': open }"
    :title="t('set.title')"
    :aria-label="t('set.title')"
    @click="toggle"
  >
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
  </button>

  <div v-if="open" class="portbar-pop right sm-pop" @click.stop>
    <div class="portbar-pop-head">
      <button v-if="view !== 'menu'" class="btn btn-ghost btn-icon btn-sm" type="button" :title="t('set.back')" :aria-label="t('set.back')" @click="back">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 18-6-6 6-6"/></svg>
      </button>
      <span>{{ title }}</span>
      <button class="btn btn-ghost btn-icon btn-sm" type="button" :title="t('app.common.close')" :aria-label="t('app.common.close')" @click="close">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
      </button>
    </div>

    <!-- 菜单视图 -->
    <div v-if="view === 'menu'">
      <div class="sm-sec">{{ t('set.secLog') }}</div>
      <button class="sm-item" type="button" @click="openLog">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
        <span>{{ t('set.openLog') }}<span class="sm-sub">{{ t('set.openLogSub') }}</span></span>
      </button>
      <button class="sm-item" type="button" @click="view = 'log'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/></svg>
        <span>{{ t('set.logSettings') }}<span class="sm-sub">{{ t('set.logSettingsSub') }}</span></span>
      </button>
      <div class="sm-sep"></div>
      <div class="sm-sec">{{ t('set.secIntegrations') }}</div>
      <button class="sm-item" type="button" @click="view = 'bridge'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>
        <span style="flex: 1">{{ t('set.bridge') }}<span class="sm-sub">{{ t('set.bridgeSub') }}</span></span>
        <span v-if="bridge.running" class="bridge-dot" :title="t('set.bridgeDot')"></span>
      </button>
      <button class="sm-item" type="button" @click="view = 'presets'">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
        <span>{{ t('set.presets') }}<span class="sm-sub">{{ t('set.presetsSub') }}</span></span>
      </button>
      <div class="sm-sep"></div>
      <div class="sm-sec">{{ t('set.secNotifications') }}</div>
      <div class="sm-switch-row">
        <span>{{ t('set.enableNotif') }}<span class="sm-sub">{{ t('set.enableNotifSub') }}</span></span>
        <label class="switch">
          <input type="checkbox" :checked="notif.prefs.enabled" @change="setNotif($event)" />
          <span class="track"></span>
          <span class="thumb"></span>
        </label>
      </div>
      <div class="sm-sep"></div>
      <div class="sm-sec">{{ t('set.secView') }}</div>
      <button class="sm-item" type="button" @click="toggleSplit">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="18" rx="1"/><rect x="14" y="3" width="7" height="18" rx="1"/></svg>
        <span style="flex: 1">{{ store.splitMode ? t('set.exitSplit') : t('set.split') }}<span class="sm-sub">{{ t('set.splitSub') }}</span></span>
      </button>
      <button class="sm-item" type="button" @click="toggleTheme()">
        <svg v-if="theme === 'dark'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>
        <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
        <span>{{ theme === 'dark' ? t('set.toLight') : t('set.toDark') }}</span>
      </button>
      <button class="sm-item" type="button" @click="toggleLocale">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/><path d="M2 5h12"/><path d="M7 2h1"/><path d="m22 22-5-10-5 10"/><path d="M14 18h6"/></svg>
        <span>{{ t('app.settings.langToggle') }}<span class="sm-sub">{{ t('app.settings.langSub') }}</span></span>
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
