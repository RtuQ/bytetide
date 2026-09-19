<script setup lang="ts">
import { computed, inject } from 'vue'
import { useSessionStore } from '../stores/session'
import { HIGHLIGHTER_KEY } from '../composables/useHighlighter'
import type { Keyword } from '../types'
import { t } from '../i18n'

const store = useSessionStore()
const active = computed(() => store.active)

const { stats } = inject(HIGHLIGHTER_KEY)!
const kwCounts = computed(() => stats.value.kwCounts)

function add() {
  if (active.value) store.addKeyword(active.value.id)
}
function upd(kid: string, patch: Partial<Keyword>) {
  if (active.value) store.updateKeyword(active.value.id, kid, patch)
}
function del(kid: string) {
  if (active.value) store.removeKeyword(active.value.id, kid)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12.586 2.586A2 2 0 0 0 11.172 2H4a2 2 0 0 0-2 2v7.172a2 2 0 0 0 .586 1.414l8.704 8.704a2.426 2.426 0 0 0 3.42 0l6.58-6.58a2.426 2.426 0 0 0 0-3.42z"/><circle cx="7.5" cy="7.5" r="1.5"/></svg>
      <span class="panel-title">{{ t('kw.title') }}</span>
      <span v-if="active" class="badge">{{ active.keywords.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="kw-body" v-if="active">
      <button class="btn btn-ghost btn-sm" @click="add">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
        <span>{{ t('kw.add') }}</span>
      </button>
      <div v-if="!active.keywords.length" class="panel-hint">
        {{ t('kw.hint') }}
      </div>
      <div v-for="k in active.keywords" :key="k.id" class="kw-card">
        <div class="kw-line">
          <input
            type="color"
            class="kw-color"
            :value="k.color"
            @input="upd(k.id, { color: ($event.target as HTMLInputElement).value })"
            :title="t('kw.colorTitle')"
          />
          <input
            class="kw-text"
            :value="k.pattern"
            @input="upd(k.id, { pattern: ($event.target as HTMLInputElement).value })"
            :placeholder="t('kw.patternPh')"
          />
          <span class="kw-count" :title="t('kw.countTitle')">{{ kwCounts[k.id] ?? 0 }}</span>
          <button class="kw-x" @click="del(k.id)" :title="t('kw.del')" :aria-label="t('kw.delAria')">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div class="kw-opts">
          <label class="check">
            <input
              type="checkbox"
              :checked="k.useRegex"
              @change="upd(k.id, { useRegex: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('kw.regex') }}</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="k.caseSensitive"
              @change="upd(k.id, { caseSensitive: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('kw.case') }}</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="k.wholeWord"
              @change="upd(k.id, { wholeWord: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('kw.whole') }}</span>
          </label>
        </div>
      </div>
    </div>
    <div v-else class="panel-empty">{{ t('kw.noSession') }}</div>
  </details>
</template>
