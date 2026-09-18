<script setup lang="ts">
import { computed, ref } from 'vue'
import { open, save } from '@tauri-apps/plugin-dialog'
import { commands } from '../ipc/commands'
import { useSessionStore } from '../stores/session'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'
import type { ConfigPreset, PresetCategory } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)

// 类目标签：值存 MessageKey，使用点 t() 求值（切语言即时刷新）
const CATS: { key: PresetCategory; label: MessageKey; hint: MessageKey }[] = [
  { key: 'filters', label: 'cpreset.cat.filters', hint: 'cpreset.cat.filtersHint' },
  { key: 'keywords', label: 'cpreset.cat.keywords', hint: 'cpreset.cat.keywordsHint' },
  { key: 'autoReply', label: 'cpreset.cat.autoReply', hint: 'cpreset.cat.autoReplyHint' },
  { key: 'plots', label: 'cpreset.cat.plots', hint: 'cpreset.cat.plotsHint' },
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
  // 自动命名 = 类目名 + 日期（点击时求值 t，入库存的是当时语言文案，属用户数据）
  const labelKey = CATS.find((c) => c.key === cat)?.label
  const name =
    saveName.value.trim() ||
    (labelKey ? t(labelKey) : cat) + ` ${new Date().toLocaleDateString()}`
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
  await commands.exportText(path, JSON.stringify({ version: 1, presets: store.configPresets }, null, 2))
}

async function importFile() {
  const path = await open({
    multiple: false,
    filters: [{ name: 'JSON', extensions: ['json'] }],
  })
  if (!path || typeof path !== 'string') return
  try {
    const raw = JSON.parse(await commands.readTextFile(path))
    const n = store.importConfigPresets(raw)
    alert(n > 0 ? t('cpreset.imported', { n }) : t('cpreset.importNone'))
  } catch (e) {
    alert(t('cpreset.importFailed', { msg: String(e instanceof Error ? e.message : e) }))
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
      <span class="panel-title">{{ t('cpreset.title') }}</span>
      <span class="badge">{{ store.configPresets.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active">
      <div class="al-hit">
        <input
          class="fl-save-name"
          :value="saveName"
          @input="saveName = ($event.target as HTMLInputElement).value"
          :placeholder="t('cpreset.namePlaceholder')"
        />
      </div>
      <div v-for="c in CATS" :key="c.key" class="cp-cat">
        <div class="cp-cat-head">
          <span class="cp-cat-title">{{ t(c.label) }}</span>
          <span class="cp-cat-hint">{{ t(c.hint) }}</span>
          <span class="send-spacer"></span>
          <button
            class="btn btn-ghost btn-sm"
            :title="t('cpreset.saveCurrentTitle', { cat: t(c.label) })"
            @click="saveCat(c.key)"
          >
            {{ t('cpreset.saveCurrent') }}
          </button>
        </div>
        <div
          v-for="p in presetsOf(c.key)"
          :key="p.id"
          class="cp-row"
          :title="t('cpreset.rowTitle', { hint: t(c.hint) })"
          @click="apply(p.id)"
        >
          <span class="cp-name">{{ p.name }}</span>
          <span class="cp-date">{{ fmtDate(p.createdAt) }}</span>
          <button
            class="ar-x"
            :title="t('cpreset.remove')"
            :aria-label="t('cpreset.remove')"
            @click.stop="remove(p.id)"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div v-if="!presetsOf(c.key).length" class="cp-empty">{{ t('cpreset.empty') }}</div>
      </div>

      <div class="ar-line cp-io">
        <button class="btn btn-ghost btn-sm" @click="exportAll">{{ t('cpreset.exportAll') }}</button>
        <span class="send-spacer"></span>
        <button class="btn btn-ghost btn-sm" @click="importFile">{{ t('cpreset.importJson') }}</button>
      </div>
    </div>
    <div v-else class="panel-empty">{{ t('cpreset.noSession') }}</div>
  </details>
</template>
