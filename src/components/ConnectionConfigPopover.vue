<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, useId, watch } from 'vue'
import type { CSSProperties } from 'vue'
import type { PortConfig } from '../types'
import { t } from '../i18n'

const props = defineProps<{
  config: PortConfig
  sessionId: string
  connected: boolean
  busy?: boolean
  error?: string
}>()
const emit = defineEmits<{ apply: [config: PortConfig]; opened: [] }>()

const anchor = ref<HTMLElement | null>(null)
const trigger = ref<HTMLButtonElement | null>(null)
const panel = ref<HTMLFormElement | null>(null)
const panelPosition = ref<CSSProperties>({ left: '12px', top: '12px' })
const instanceId = useId()
const panelId = computed(() => {
  const sessionPart = props.sessionId.replace(/[^a-zA-Z0-9_-]/g, '-')
  return `connection-config-panel-${sessionPart}-${instanceId}`
})
const baudInput = ref<HTMLInputElement | null>(null)
const open = ref(false)
const draft = reactive({
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
})

function syncDraft() {
  Object.assign(draft, {
    baudRate: props.config.baudRate,
    dataBits: props.config.dataBits,
    parity: props.config.parity,
    stopBits: props.config.stopBits,
    flowControl: props.config.flowControl,
  })
}
function close(restoreFocus = true) {
  open.value = false
  if (restoreFocus) void nextTick(() => trigger.value?.focus())
}
function toggle() {
  if (open.value) close()
  else open.value = true
}
function updatePosition() {
  const anchorRect = anchor.value?.getBoundingClientRect()
  const panelEl = panel.value
  if (!anchorRect || !panelEl) return

  const margin = 12
  const gap = 6
  const viewportWidth = window.innerWidth
  const viewportHeight = window.innerHeight
  const availableWidth = Math.max(1, viewportWidth - margin * 2)
  const availableHeight = Math.max(1, viewportHeight - margin * 2)
  const rect = panelEl.getBoundingClientRect()
  const width = Math.min(rect.width || panelEl.offsetWidth || 330, availableWidth)
  const height = Math.min(
    rect.height || panelEl.offsetHeight || panelEl.scrollHeight || 420,
    availableHeight,
  )

  const left = Math.min(
    Math.max(margin, anchorRect.right - width),
    viewportWidth - margin - width,
  )
  let top = anchorRect.bottom + gap
  if (top + height > viewportHeight - margin) top = anchorRect.top - height - gap
  top = Math.min(Math.max(margin, top), viewportHeight - margin - height)
  panelPosition.value = {
    left: `${left}px`,
    top: `${top}px`,
    maxHeight: 'calc(100vh - 24px)',
    overflowY: 'auto',
  }
}
function onOutsidePointer(event: PointerEvent) {
  const target = event.target as Node
  if (open.value && !anchor.value?.contains(target) && !panel.value?.contains(target)) close(false)
}
function onKeydown(event: KeyboardEvent) {
  if (open.value && event.key === 'Escape') close()
}
let panelResizeObserver: ResizeObserver | null = null
function stopPositionTracking() {
  window.removeEventListener('resize', updatePosition)
  window.removeEventListener('scroll', updatePosition, true)
  panelResizeObserver?.disconnect()
  panelResizeObserver = null
}
watch(() => props.error, () => {
  if (open.value) updatePosition()
}, { flush: 'post' })
watch(() => props.sessionId, () => close())
watch(open, async (value) => {
  if (!value) {
    stopPositionTracking()
    return
  }
  syncDraft()
  emit('opened')
  await nextTick()
  baudInput.value?.focus()
  updatePosition()
  window.addEventListener('resize', updatePosition)
  window.addEventListener('scroll', updatePosition, true)
  if (typeof ResizeObserver !== 'undefined' && panel.value) {
    panelResizeObserver = new ResizeObserver(updatePosition)
    panelResizeObserver.observe(panel.value)
  }
})
onMounted(() => {
  document.addEventListener('pointerdown', onOutsidePointer)
  document.addEventListener('keydown', onKeydown)
})
onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', onOutsidePointer)
  document.removeEventListener('keydown', onKeydown)
  stopPositionTracking()
})
function submit() {
  emit('apply', {
    ...props.config,
    baudRate: Number(draft.baudRate),
    dataBits: Number(draft.dataBits),
    parity: draft.parity,
    stopBits: draft.stopBits,
    flowControl: draft.flowControl,
  })
}
defineExpose({ close })
</script>

<template>
  <div ref="anchor" class="connection-config-anchor">
    <button
      ref="trigger"
      class="btn btn-ghost btn-sm connection-config-trigger"
      type="button"
      :title="t('lv.connectionConfig.triggerTitle')"
      :aria-label="t('lv.connectionConfig.title')"
      :aria-expanded="open"
      :aria-controls="panelId"
      @click="toggle"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><line x1="21" y1="4" x2="14" y2="4"/><line x1="10" y1="4" x2="3" y2="4"/><line x1="21" y1="12" x2="12" y2="12"/><line x1="8" y1="12" x2="3" y2="12"/><line x1="21" y1="20" x2="16" y2="20"/><line x1="12" y1="20" x2="3" y2="20"/><line x1="14" y1="2" x2="14" y2="6"/><line x1="8" y1="10" x2="8" y2="14"/><line x1="16" y1="18" x2="16" y2="22"/></svg>
      <span>{{ t('lv.connectionConfig.title') }}</span>
    </button>
    <Teleport to="body">
      <form
        v-if="open"
        :id="panelId"
        ref="panel"
        class="portbar-pop connection-config-pop"
        :style="panelPosition"
        @submit.prevent="submit"
      >
      <div class="portbar-pop-head">
        <span>{{ t('lv.connectionConfig.title') }}</span>
        <button class="btn btn-ghost btn-sm btn-icon" type="button" :title="t('lv.connectionConfig.close')" :aria-label="t('lv.connectionConfig.close')" @click="close()">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="m18 6-12 12M6 6l12 12"/></svg>
        </button>
      </div>
      <p class="connection-config-copy">{{ t(props.connected ? 'lv.connectionConfig.connectedHint' : 'lv.connectionConfig.stoppedHint') }}</p>
      <div class="connection-config-grid">
        <label class="field connection-config-port">
          <span class="field-label">{{ t('lv.connectionConfig.port') }}</span>
          <input class="input input-mono" :value="config.name" readonly aria-readonly="true">
        </label>
        <label class="field">
          <span class="field-label">{{ t('conn.baud') }}</span>
          <input ref="baudInput" v-model.number="draft.baudRate" class="input input-mono" type="number" min="1" max="12000000" required>
        </label>
        <label class="field">
          <span class="field-label">{{ t('conn.dataBits') }}</span>
          <select v-model.number="draft.dataBits" class="select"><option :value="8">8</option><option :value="7">7</option><option :value="6">6</option><option :value="5">5</option></select>
        </label>
        <label class="field">
          <span class="field-label">{{ t('conn.parity') }}</span>
          <select v-model="draft.parity" class="select"><option value="none">N</option><option value="odd">O</option><option value="even">E</option></select>
        </label>
        <label class="field">
          <span class="field-label">{{ t('conn.stopBits') }}</span>
          <select v-model="draft.stopBits" class="select"><option value="1">1</option><option value="2">2</option></select>
        </label>
        <label class="field connection-config-flow">
          <span class="field-label">{{ t('conn.flowControl') }}</span>
          <select v-model="draft.flowControl" class="select"><option value="none">None</option><option value="software">Xon/Xoff</option><option value="hardware">RTS/CTS</option></select>
        </label>
      </div>
      <p class="connection-config-hint">{{ t('lv.connectionConfig.preserveHint') }}</p>
      <p v-if="error" class="connection-config-error" role="alert">{{ error }}</p>
      <div class="pop-foot">
        <button class="btn btn-ghost btn-sm" type="button" :disabled="busy" @click="close()">{{ t('lv.connectionConfig.cancel') }}</button>
        <button class="btn btn-primary btn-sm" type="submit" :disabled="busy">
          <span v-if="busy" class="connection-config-spinner" aria-hidden="true"></span>
          {{ busy ? t('lv.connectionConfig.working') : t(props.connected ? 'lv.connectionConfig.applyReconnect' : 'lv.connectionConfig.applyConnect') }}
        </button>
      </div>
      </form>
    </Teleport>
  </div>
</template>

<style scoped>
.connection-config-anchor { position: relative; display: inline-flex; }
.connection-config-trigger { white-space: nowrap; }
.connection-config-trigger svg { width: 14px; height: 14px; flex: none; }
.connection-config-pop {
  position: fixed;
  z-index: 1000;
  right: auto;
  width: 330px;
  max-width: min(420px, calc(100vw - 24px));
  max-height: calc(100vh - 24px);
  overflow-y: auto;
  overscroll-behavior: contain;
}
.connection-config-copy, .connection-config-hint { margin: 0; color: var(--text-muted); font-size: 11px; line-height: 1.5; }
.connection-config-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 9px; }
.connection-config-port, .connection-config-flow { grid-column: span 2; }
.connection-config-port .input { background: var(--bg-base); color: var(--text-muted); }
.connection-config-error { margin: 0; color: var(--err); font-size: 11px; line-height: 1.45; overflow-wrap: anywhere; }
.connection-config-pop .pop-foot { position: sticky; bottom: 0; padding: 8px 0 0; background: var(--bg-surface); }
.connection-config-spinner { width: 12px; height: 12px; border: 2px solid currentColor; border-right-color: transparent; border-radius: 50%; animation: config-spin 700ms linear infinite; }
@keyframes config-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .connection-config-spinner { animation: none; } }
</style>
