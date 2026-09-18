<script setup lang="ts">
import { ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { t } from '../i18n'
import type { PortConfig } from '../types'

// 内容组件：由 SettingsPopover 承载（自身不带触发按钮与浮层壳）
const props = defineProps<{ current: PortConfig }>()
const emit = defineEmits<{ apply: [config: PortConfig] }>()

const store = useSessionStore()
const newName = ref('')

function save() {
  if (!newName.value.trim()) return
  store.addPreset(newName.value, props.current)
  newName.value = ''
}
function apply(c: PortConfig) {
  emit('apply', { ...c })
  newName.value = ''
}
function onRename(id: string, e: Event) {
  store.renamePreset(id, (e.target as HTMLInputElement).value)
}
</script>

<template>
  <div class="field">
    <span class="field-label">{{ t('preset.saveLabel') }}</span>
    <div class="preset-save-row">
      <input
        class="input"
        v-model="newName"
        :placeholder="t('preset.namePlaceholder')"
        @keydown.enter="save"
      />
      <button class="btn btn-sm btn-primary" type="button" :disabled="!newName.trim()" @click="save">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
        <span>{{ t('preset.save') }}</span>
      </button>
    </div>
  </div>

  <div v-if="store.presets.length" class="preset-list">
    <div v-for="p in store.presets" :key="p.id" class="preset-row">
      <input
        class="input preset-name"
        :value="p.name"
        @change="onRename(p.id, $event)"
        spellcheck="false"
      />
      <span class="preset-meta">{{ p.config.name || '—' }} · {{ p.config.baudRate }}</span>
      <button class="btn btn-sm btn-ghost" type="button" :title="t('preset.applyTitle')" @click="apply(p.config)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
        <span>{{ t('preset.apply') }}</span>
      </button>
      <button class="btn btn-sm btn-ghost btn-icon" type="button" :title="t('preset.remove')" @click="store.removePreset(p.id)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
      </button>
    </div>
  </div>
  <div v-else class="panel-hint">{{ t('preset.empty') }}</div>

  <div class="sm-foot">
    <span class="panel-hint">{{ t('preset.applyHint') }}</span>
  </div>
</template>
