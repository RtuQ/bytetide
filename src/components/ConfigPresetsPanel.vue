<script setup lang="ts">
import { computed, ref } from 'vue'
import { open, save } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore } from '../stores/session'
import type { ConfigPreset, PresetCategory } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)

const CATS: { key: PresetCategory; label: string; hint: string }[] = [
  { key: 'filters', label: '过滤链', hint: '整组包含/排除条件' },
  { key: 'keywords', label: '关键词', hint: '多色高亮关键词组' },
  { key: 'autoReply', label: '自动回复', hint: '规则与总开关' },
  { key: 'plots', label: '绘图帧配置', hint: '帧头/校验/通道等' },
]

const saveName = ref('')

function presetsOf(cat: PresetCategory): ConfigPreset[] {
  return store.configPresets.filter((p) => p.category === cat)
}

/** 把活动会话对应类别的当前配置存为命名预设 */
function saveCat(cat: PresetCategory) {
  const s = active.value
  if (!s) return
  const data =
    cat === 'filters'
      ? s.filters.map((f) => ({ ...f }))
      : cat === 'keywords'
        ? s.keywords.map((k) => ({ ...k }))
        : cat === 'autoReply'
          ? { enabled: s.autoReply.enabled, rules: s.autoReply.rules.map((r) => ({ ...r })) }
          : { ...s.plot }
  const name =
    saveName.value.trim() ||
    (CATS.find((c) => c.key === cat)?.label ?? cat) + ` ${new Date().toLocaleDateString()}`
  store.saveConfigPreset(cat, name, data)
}

function apply(pid: string) {
  void store.applyConfigPreset(pid)
}
function remove(pid: string) {
  store.removeConfigPreset(pid)
}

async function exportAll() {
  const path = await save({
    defaultPath: 'serial-presets.json',
    filters: [{ name: 'JSON', extensions: ['json'] }],
  })
  if (!path) return
  await invoke('export_text_cmd', {
    path,
    content: JSON.stringify({ version: 1, presets: store.configPresets }, null, 2),
  })
}

async function importFile() {
  const path = await open({
    multiple: false,
    filters: [{ name: 'JSON', extensions: ['json'] }],
  })
  if (!path || typeof path !== 'string') return
  try {
    const raw = JSON.parse(await invoke<string>('read_text_file_cmd', { path }))
    const n = store.importConfigPresets(raw)
    alert(n > 0 ? `已导入 ${n} 条预设` : '未发现可导入的预设（形状不符）')
  } catch (e) {
    alert(`导入失败：${String(e instanceof Error ? e.message : e)}`)
  }
}

function fmtDate(ts: number) {
  const d = new Date(ts)
  return `${d.getMonth() + 1}/${d.getDate()}`
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="m3.3 7 8.7 5 8.7-5"/><path d="M12 22V12"/></svg>
      <span class="panel-title">预设库</span>
      <span class="badge">{{ store.configPresets.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active">
      <div class="al-hit">
        <input
          class="fl-save-name"
          :value="saveName"
          @input="saveName = ($event.target as HTMLInputElement).value"
          placeholder="预设名称（留空自动命名）"
        />
      </div>
      <div v-for="c in CATS" :key="c.key" class="cp-cat">
        <div class="cp-cat-head">
          <span class="cp-cat-title">{{ c.label }}</span>
          <span class="cp-cat-hint">{{ c.hint }}</span>
          <span class="send-spacer"></span>
          <button
            class="btn btn-ghost btn-sm"
            :title="'把当前会话的' + c.label + '存为预设'"
            @click="saveCat(c.key)"
          >
            存当前
          </button>
        </div>
        <div
          v-for="p in presetsOf(c.key)"
          :key="p.id"
          class="cp-row"
          :title="c.hint + '，点击套用到活动会话'"
          @click="apply(p.id)"
        >
          <span class="cp-name">{{ p.name }}</span>
          <span class="cp-date">{{ fmtDate(p.createdAt) }}</span>
          <button
            class="ar-x"
            title="删除预设"
            aria-label="删除预设"
            @click.stop="remove(p.id)"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div v-if="!presetsOf(c.key).length" class="cp-empty">暂无</div>
      </div>

      <div class="ar-line cp-io">
        <button class="btn btn-ghost btn-sm" @click="exportAll">导出全部 JSON</button>
        <span class="send-spacer"></span>
        <button class="btn btn-ghost btn-sm" @click="importFile">导入 JSON</button>
      </div>
    </div>
    <div v-else class="panel-empty">无活动会话</div>
  </details>
</template>
