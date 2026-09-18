<script setup lang="ts">
import { computed, inject, ref, shallowRef, watchEffect } from 'vue'
import { useSessionStore } from '../stores/session'
import { HIGHLIGHTER_KEY, hlStyle } from '../composables/useHighlighter'
import type { FilterDir, FilterStage, LogLine } from '../types'
import { t } from '../i18n'

const store = useSessionStore()
const active = computed(() => store.active)
const showHist = ref(false)

const { stats, segmentsFor } = inject(HIGHLIGHTER_KEY)!
const matchSet = computed(() => new Set(stats.value.matchLines))

// 搜索命中结果紧邻输入框展示（封顶最近 500 行）。
// 折叠时完全停止计算与渲染--长跑监控中隐藏面板不得做全量行扫描。
// 命中行直接取 stats.matchLines 尾部再按 no 查行，避免对 5 万行再全量 filter
// （matchLines 本身就是高亮器全量扫描的产物，重复 O(N) 会持续吃吞吐）。
const hitsOpen = ref(true)
function syncHitsOpen(e: Event) {
  hitsOpen.value = (e.target as HTMLDetailsElement).open
}
const matchedLines = shallowRef<LogLine[]>([])
watchEffect(() => {
  if (!hitsOpen.value) return
  const s = active.value
  if (!s || matchSet.value.size === 0) {
    matchedLines.value = []
    return
  }
  const nos = stats.value.matchLines
  const lines = s.lines
  if (lines.length === 0) {
    matchedLines.value = []
    return
  }
  // 行号在缓冲内连续（单调分配+尾部裁剪），no -> 下标 O(1)；不做全量 filter
  const base = lines[0]!.no
  const out: LogLine[] = []
  for (let i = nos.length - 1; i >= 0 && out.length < 500; i--) {
    const idx = nos[i]! - base
    if (idx >= 0 && idx < lines.length) out.unshift(lines[idx]!)
  }
  matchedLines.value = out
})
function jumpTo(no: number) {
  if (active.value) store.requestJump(active.value.id, no)
}

function addStage() {
  if (active.value) store.addFilterStage(active.value.id)
}
function updStage(fid: string, p: Partial<FilterStage>) {
  if (active.value) store.updateFilterStage(active.value.id, fid, p)
}
function delStage(fid: string) {
  if (active.value) store.removeFilterStage(active.value.id, fid)
}

function patch(p: {
  pattern?: string
  useRegex?: boolean
  caseSensitive?: boolean
  wholeWord?: boolean
}) {
  if (active.value) store.updateSearch(active.value.id, p)
}

function recordCurrent() {
  const p = active.value?.search.pattern?.trim()
  if (p) store.pushSearchHistory(p)
}

function onInput(e: Event) {
  patch({ pattern: (e.target as HTMLInputElement).value })
  if (!showHist.value && store.searchHistory.length) showHist.value = true
}
function onFocus() {
  if (store.searchHistory.length) showHist.value = true
}
function onBlur() {
  recordCurrent()
  setTimeout(() => {
    showHist.value = false
  }, 150)
}
function onEnter() {
  recordCurrent()
  showHist.value = false
}
function applyHist(item: string) {
  patch({ pattern: item })
  store.pushSearchHistory(item)
  showHist.value = false
}
function delHist(item: string) {
  store.removeSearchHistory(item)
  if (store.searchHistory.length === 0) showHist.value = false
}

const filteredHist = computed(() => {
  const q = active.value?.search.pattern?.toLowerCase() ?? ''
  const hist = store.searchHistory
  if (!q) return hist
  return hist.filter((h) => h.toLowerCase().includes(q))
})
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
      <span class="panel-title">{{ t('lv.search.title') }}</span>
      <span v-if="active && stats.matchLines.length" class="badge">{{ stats.matchLines.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active">
      <div class="search-wrap">
        <svg class="search-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
        <input
          class="search-input"
          :value="active.search.pattern"
          @input="onInput"
          @focus="onFocus"
          @blur="onBlur"
          @keydown.enter.prevent="onEnter"
          @keydown.esc="showHist = false"
          :placeholder="t('lv.search.placeholder')"
        />
        <div v-if="showHist && filteredHist.length" class="search-hist">
          <div
            v-for="h in filteredHist"
            :key="h"
            class="search-hist-item"
            @mousedown.prevent="applyHist(h)"
          >
            <svg class="search-hist-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
            <span class="txt">{{ h }}</span>
            <button
              class="search-hist-del"
              type="button"
              :title="t('lv.search.delHist')"
              :aria-label="t('lv.search.delHist')"
              @mousedown.prevent.stop="delHist(h)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
            </button>
          </div>
        </div>
      </div>
      <div class="opts">
        <label class="check">
          <input
            type="checkbox"
            :checked="active.search.useRegex"
            @change="patch({ useRegex: ($event.target as HTMLInputElement).checked })"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>{{ t('lv.search.regex') }}</span>
        </label>
        <label class="check">
          <input
            type="checkbox"
            :checked="active.search.caseSensitive"
            @change="patch({ caseSensitive: ($event.target as HTMLInputElement).checked })"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>{{ t('lv.search.caseSensitive') }}</span>
        </label>
        <label class="check">
          <input
            type="checkbox"
            :checked="active.search.wholeWord"
            @change="patch({ wholeWord: ($event.target as HTMLInputElement).checked })"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>{{ t('lv.search.wholeWord') }}</span>
        </label>
      </div>

      <!-- 命中结果：紧贴搜索输入；折叠即停算，展开由用户控制 -->
      <details class="sh-wrap" open @toggle="syncHitsOpen">
        <summary class="sh-head" :title="t('lv.search.hitsToggleTitle')">
          <svg class="sh-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
          <b>{{ stats.total }}</b> {{ t('lv.search.hitsUnit', { n: stats.matchLines.length }) }}
        </summary>
        <div v-if="hitsOpen" class="sh-list">
          <div
            v-for="l in matchedLines"
            :key="l.no"
            class="ms-row"
            :class="l.dir"
            @click="jumpTo(l.no)"
          >
            <span class="ms-no">{{ l.no }}</span>
            <span class="ms-ts">{{ l.ts }}</span>
            <span class="ms-tx">
              <span
                v-for="(seg, i) in segmentsFor(l.text)"
                :key="i"
                :style="seg.color ? hlStyle(seg.color) : undefined"
                >{{ seg.text }}</span
              >
            </span>
          </div>
          <div v-if="!matchedLines.length" class="ms-empty">{{ t('lv.search.noHits') }}</div>
        </div>
      </details>

      <div class="fl-head">
        <span class="fl-title">{{ t('lv.search.filterChain', { n: active.filters.length }) }}</span>
        <span class="send-spacer"></span>
        <button class="btn btn-ghost btn-sm" @click="addStage">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
          <span>{{ t('lv.search.addStage') }}</span>
        </button>
      </div>
      <div v-if="!active.filters.length" class="panel-hint">
        {{ t('lv.search.chainHint') }}
      </div>
      <div
        v-for="f in active.filters"
        :key="f.id"
        class="ar-card fl-card"
        :class="{ off: !f.enabled }"
      >
        <div class="ar-line">
          <div class="seg">
            <button
              class="seg-item"
              :class="{ active: f.mode === 'include' }"
              :title="t('lv.search.includeTitle')"
              @click="updStage(f.id, { mode: 'include' })"
            >
              {{ t('lv.search.include') }}
            </button>
            <button
              class="seg-item"
              :class="{ active: f.mode === 'exclude' }"
              :title="t('lv.search.excludeTitle')"
              @click="updStage(f.id, { mode: 'exclude' })"
            >
              {{ t('lv.search.exclude') }}
            </button>
          </div>
          <input
            class="ar-reply"
            :value="f.text"
            @input="updStage(f.id, { text: ($event.target as HTMLInputElement).value })"
            :placeholder="t('lv.search.stagePlaceholder')"
          />
          <button class="ar-x" @click="delStage(f.id)" :title="t('lv.search.delStageTitle')" :aria-label="t('lv.search.delStageAria')">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div class="ar-opts">
          <label class="check">
            <input
              type="checkbox"
              :checked="f.enabled"
              @change="updStage(f.id, { enabled: ($event.target as HTMLInputElement).checked })"
              :title="t('lv.search.enableTitle')"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('lv.search.enable') }}</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="f.useRegex"
              @change="updStage(f.id, { useRegex: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('lv.search.regex') }}</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="f.caseSensitive"
              @change="updStage(f.id, { caseSensitive: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('lv.search.caseShort') }}</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="f.wholeWord"
              @change="updStage(f.id, { wholeWord: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('lv.search.wholeWord') }}</span>
          </label>
          <select
            class="select select-sm"
            :value="f.dir"
            :title="t('lv.search.dirTitle')"
            @change="updStage(f.id, { dir: ($event.target as HTMLSelectElement).value as FilterDir })"
          >
            <option value="any">{{ t('lv.search.dirAny') }}</option>
            <option value="rx">{{ t('lv.search.dirRx') }}</option>
            <option value="tx">{{ t('lv.search.dirTx') }}</option>
          </select>
        </div>
      </div>
    </div>
    <div v-else class="panel-empty">{{ t('lv.noSession') }}</div>
  </details>
</template>
