<script setup lang="ts">
import { ref } from 'vue'
import { useSessionStore } from '../stores/session'
import type { PortConfig } from '../types'

const props = defineProps<{ current: PortConfig }>()
const emit = defineEmits<{ apply: [config: PortConfig] }>()

const store = useSessionStore()
const open = ref(false)
const newName = ref('')

function toggle() {
  open.value = !open.value
  if (!open.value) newName.value = ''
}
function close() {
  open.value = false
  newName.value = ''
}
function save() {
  if (!newName.value.trim()) return
  store.addPreset(newName.value, props.current)
  newName.value = ''
}
function apply(c: PortConfig) {
  emit('apply', { ...c })
  close()
}
function onRename(id: string, e: Event) {
  store.renamePreset(id, (e.target as HTMLInputElement).value)
}
</script>

<template>
  <div class="logcfg">
    <button
      class="btn btn-ghost btn-icon"
      type="button"
      :class="{ 'is-active': open }"
      title="连接配置预设"
      aria-label="连接配置预设"
      @click="toggle"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
    </button>

    <div v-if="open" class="logcfg-backdrop" @click="close"></div>
    <div v-if="open" class="logcfg-pop preset-pop" @click.stop>
      <div class="logcfg-pop-head">
        <span>连接配置预设</span>
        <button class="btn btn-ghost btn-icon btn-sm" type="button" title="关闭" aria-label="关闭" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
        </button>
      </div>

      <div class="field">
        <span class="field-label">保存当前配置为预设</span>
        <div class="preset-save-row">
          <input
            class="input"
            v-model="newName"
            placeholder="预设名称，如 GPS-9600"
            @keydown.enter="save"
          />
          <button class="btn btn-sm btn-primary" type="button" :disabled="!newName.trim()" @click="save">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z"/></svg>
            <span>保存</span>
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
          <button class="btn btn-sm btn-ghost" type="button" title="应用到端口栏" @click="apply(p.config)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            <span>应用</span>
          </button>
          <button class="btn btn-sm btn-ghost btn-icon" type="button" title="删除预设" @click="store.removePreset(p.id)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
          </button>
        </div>
      </div>
      <div v-else class="panel-hint">暂无预设。配置好端口栏后，在上面填名称保存。</div>

      <div class="logcfg-foot">
        <span class="panel-hint">应用只回填到端口栏，需再点“连接”</span>
      </div>
    </div>
  </div>
</template>
