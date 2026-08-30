<script setup lang="ts">
import { computed, provide, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { HIGHLIGHTER_KEY, useHighlighter } from '../composables/useHighlighter'
import { DEFAULT_SEARCH } from '../types'
import LogView from './LogView.vue'

const props = defineProps<{ columnIndex: number }>()
const store = useSessionStore()

const sessionId = computed(() => store.columns[props.columnIndex] ?? '')
const session = computed(() => (sessionId.value ? store.sessions[sessionId.value] : null) ?? null)

// 列局部高亮器：LogView 通过 inject(HIGHLIGHTER_KEY) 取最近祖先 provide，
// 因此每列独立统计/高亮，互不干扰；单列模式仍用 App 顶层 provide。
const highlighter = useHighlighter(
  () => session.value?.search ?? DEFAULT_SEARCH,
  () => session.value?.keywords ?? [],
  () => session.value?.lines ?? [],
  () => session.value?.lineCounter ?? 0,
)
provide(HIGHLIGHTER_KEY, highlighter)

// 迷你搜索：直接写当前列会话的 search，复用顶层统计/只看命中逻辑
function onSearch(e: Event) {
  if (!sessionId.value) return
  store.updateSearch(sessionId.value, { pattern: (e.target as HTMLInputElement).value })
}
function toggleRegex(e: Event) {
  if (!sessionId.value) return
  store.updateSearch(sessionId.value, { useRegex: (e.target as HTMLInputElement).checked })
}

// 迷你发送（不带定时器，保持紧凑；定时发送在单列模式用）
const text = ref('')
const mode = ref<'ascii' | 'hex'>('ascii')
const appendNewline = ref(true)
const busy = ref(false)

async function send() {
  const id = sessionId.value
  const s = session.value
  if (!id || !s || busy.value) return
  let payload = text.value
  if (mode.value === 'ascii' && appendNewline.value) payload += '\n'
  busy.value = true
  try {
    await store.send(id, payload, mode.value)
    text.value = ''
  } catch (e: unknown) {
    alert(String(e instanceof Error ? e.message : e))
  } finally {
    busy.value = false
  }
}
function pick(h: string) {
  text.value = h
}

function onPickSession(e: Event) {
  const v = (e.target as HTMLSelectElement).value
  store.setColumnSession(props.columnIndex, v || null)
  if (v) store.setActive(v)
}

// 点击列内任意位置即把该列会话设为 active：侧栏面板（搜索/关键词/告警…）
// 始终跟随用户正在操作的列（activeId 只有标签页点击会改，分屏下会停在旧会话）
function activate() {
  if (sessionId.value && store.activeId !== sessionId.value) store.setActive(sessionId.value)
}
</script>

<template>
  <div class="scol" @mousedown.capture="activate">
    <div class="scol-head">
      <select
        class="select scol-select"
        :value="sessionId"
        @change="onPickSession"
        title="选择该列显示的会话"
      >
        <option value="">选择会话</option>
        <option v-for="s in store.sessionList" :key="s.id" :value="s.id">
          {{ s.config.name }}{{ s.status === 'connected' ? '' : ' · ' + s.status }}
        </option>
      </select>
      <span v-if="session" class="col-dot" :class="session.status"></span>
      <span class="scol-spacer"></span>
      <button
        v-if="store.columns.length > 2"
        class="scol-x"
        title="删除该列"
        @click="store.removeColumn(props.columnIndex)"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
      </button>
    </div>

    <template v-if="session">
      <LogView :session-id="sessionId" />

      <div class="mini-search">
        <input
          class="mini-input input-mono"
          :value="session.search.pattern"
          @input="onSearch"
          placeholder="搜索…"
        />
        <label class="check">
          <input type="checkbox" :checked="session.search.useRegex" @change="toggleRegex" />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>正则</span>
        </label>
      </div>

      <div class="mini-send">
        <div class="mini-send-head">
          <div class="seg" role="group">
            <button class="seg-item" :class="{ active: mode === 'ascii' }" @click="mode = 'ascii'">ASCII</button>
            <button class="seg-item" :class="{ active: mode === 'hex' }" @click="mode = 'hex'">HEX</button>
          </div>
          <label v-if="mode === 'ascii'" class="check">
            <input type="checkbox" v-model="appendNewline" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>换行</span>
          </label>
          <span class="mini-spacer"></span>
          <button
            class="btn btn-primary btn-sm"
            :disabled="busy || session.status !== 'connected'"
            @click="send"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
            <span>发送</span>
          </button>
        </div>
        <textarea
          class="mini-text input-mono"
          v-model="text"
          :placeholder="mode === 'hex' ? 'Hex，如 41 42 43' : '发送内容（Ctrl+Enter 发送）'"
          @keydown.ctrl.enter.prevent="send"
        />
        <details class="hist mini-hist">
          <summary>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
            发送历史 ({{ session.sendHistory.length }})
          </summary>
          <div v-for="(h, i) in session.sendHistory" :key="i" class="hist-item" @click="pick(h)">
            {{ h }}
          </div>
        </details>
      </div>
    </template>

    <div v-else class="scol-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h0a4 4 0 0 1-4-4V8Z"/></svg>
      <span>在顶部选择一个会话</span>
    </div>
  </div>
</template>
