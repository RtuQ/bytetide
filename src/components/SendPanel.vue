<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSessionStore } from '../stores/session'

const store = useSessionStore()
const active = computed(() => store.active)
const text = ref('')
const mode = ref<'ascii' | 'hex'>('ascii')
const appendNewline = ref(true)
const busy = ref(false)

async function send() {
  const s = active.value
  if (!s || busy.value) return
  let payload = text.value
  if (mode.value === 'ascii' && appendNewline.value) payload += '\n'
  busy.value = true
  try {
    await store.send(s.id, payload, mode.value)
    text.value = ''
  } catch (e: unknown) {
    alert(String(e instanceof Error ? e.message : e))
  } finally {
    busy.value = false
  }
}

// 定时自动发送：按间隔重复发送当前文本到当前会话（断开/切换/卸载时停止）
const autoEnabled = ref(false)
const autoIntervalMs = ref(1000)
let timer: number | null = null

function stopAuto() {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
}
function startAuto() {
  stopAuto()
  if (!autoEnabled.value) return
  timer = window.setInterval(tickAuto, Math.max(50, autoIntervalMs.value))
}
async function tickAuto() {
  const s = active.value
  if (!s || s.status !== 'connected') return
  let p = text.value
  if (mode.value === 'ascii' && appendNewline.value) p += '\n'
  try {
    await store.send(s.id, p, mode.value)
  } catch {
    /* 忽略单次失败 */
  }
}
watch([autoEnabled, autoIntervalMs, () => active.value?.id], () => {
  if (autoEnabled.value) startAuto()
  else stopAuto()
})
onBeforeUnmount(stopAuto)

function pick(h: string) {
  text.value = h
}
</script>

<template>
  <details class="sendpanel" v-if="active">
    <summary class="send-toggle">
      <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
      <svg class="send-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
      <span>发送</span>
      <span class="send-mode">{{ mode === 'hex' ? 'HEX' : 'ASCII' }}</span>
    </summary>

    <div class="send-body">
      <div class="send-head">
      <div class="seg" role="group" aria-label="发送模式">
        <button
          class="seg-item"
          :class="{ active: mode === 'ascii' }"
          @click="mode = 'ascii'"
        >
          ASCII
        </button>
        <button
          class="seg-item"
          :class="{ active: mode === 'hex' }"
          @click="mode = 'hex'"
        >
          HEX
        </button>
      </div>
      <span class="sep"></span>
      <label v-if="mode === 'ascii'" class="check">
        <input type="checkbox" v-model="appendNewline" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>追加换行</span>
      </label>
    </div>

    <textarea
      class="send-text input-mono"
      v-model="text"
      :placeholder="
        mode === 'hex' ? 'Hex，如 41 42 43' : '发送内容（Ctrl+Enter 发送）'
      "
      @keydown.ctrl.enter.prevent="send"
    />

    <div class="send-foot">
      <label class="check">
        <input type="checkbox" v-model="autoEnabled" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>定时</span>
      </label>
      <input
        class="auto-int"
        type="number"
        v-model.number="autoIntervalMs"
        min="50"
        step="100"
      />
      <span class="small muted">ms</span>
      <span class="send-spacer"></span>
      <button
        class="btn btn-primary"
        :disabled="busy || active.status !== 'connected'"
        @click="send"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
        <span>发送</span>
      </button>
    </div>

    <details class="hist">
      <summary>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
        发送历史 ({{ active.sendHistory.length }})
      </summary>
      <div
        v-for="(h, i) in active.sendHistory"
        :key="i"
        class="hist-item"
        @click="pick(h)"
      >
        {{ h }}
      </div>
    </details>
    </div>
  </details>
  <div v-else class="muted panel-empty">无活动会话</div>
</template>
