<script setup lang="ts">
import { computed } from 'vue'
import { useSessionStore } from '../stores/session'

// AI 批注：AI 经 REST 桥对可疑行留言，日志行标记 + 此处列表实时显示（跟随活动会话）
const store = useSessionStore()
const active = computed(() => store.active)
const aiNotes = computed(() => active.value?.aiNotes ?? [])

function jumpToNote(no: number) {
  if (active.value) store.requestJump(active.value.id, no)
}
function removeNote(id: string) {
  if (active.value) store.removeAiNote(active.value.id, id)
}
function clearNotes() {
  if (active.value) store.clearAiNotes(active.value.id)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l1.9 5.7a2 2 0 0 0 1.4 1.4L21 12l-5.7 1.9a2 2 0 0 0-1.4 1.4L12 21l-1.9-5.7a2 2 0 0 0-1.4-1.4L3 12l5.7-1.9a2 2 0 0 0 1.4-1.4L12 3z"/></svg>
      <span class="panel-title">AI 批注</span>
      <span v-if="aiNotes.length" class="badge">{{ aiNotes.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body">
      <div v-if="aiNotes.length" class="ai-list">
        <div v-for="n in aiNotes" :key="n.id" class="ai-item" @click="jumpToNote(n.no)">
          <div class="ai-meta">
            <span class="ms-no">{{ n.no }}</span>
            <span class="ms-ts">{{ n.ts }}</span>
            <button
              class="ai-x"
              title="删除此批注"
              aria-label="删除此批注"
              @click.stop="removeNote(n.id)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
            </button>
          </div>
          <div class="ai-note">{{ n.note }}</div>
          <div v-if="n.text" class="ai-excerpt">{{ n.text }}</div>
        </div>
      </div>
      <p v-else class="panel-hint">暂无批注。AI 分析时可对可疑行写入批注，此处与日志行标记实时显示。</p>
      <div v-if="aiNotes.length" class="ai-sect-foot">
        <button class="btn btn-ghost btn-sm" @click="clearNotes()">清空</button>
      </div>
    </div>
  </details>
</template>
