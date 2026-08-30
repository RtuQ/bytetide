<script setup lang="ts">
import { computed, inject } from 'vue'
import { useSessionStore } from '../stores/session'
import { HIGHLIGHTER_KEY, hlStyle } from '../composables/useHighlighter'
import type { LogLine } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)
const { segmentsFor } = inject(HIGHLIGHTER_KEY)!

// 仅列出仍然存在于缓冲中的书签（被环形淘汰的行号自动隐藏，但保留记录）。
// 无书签时零成本早退——长跑监控的常态路径不做全量行扫描。
const live = computed<LogLine[]>(() => {
  const s = active.value
  if (!s || s.bookmarks.length === 0) return []
  const bm = new Set(s.bookmarks)
  return s.lines.filter((l) => bm.has(l.no))
})

function jump(no: number) {
  if (active.value) store.requestJump(active.value.id, no)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m19 21-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v16z"/></svg>
      <span class="panel-title">书签</span>
      <span v-if="active" class="badge">{{ live.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active">
      <div class="bm-list">
        <div
          v-for="l in live"
          :key="l.no"
          class="ms-row"
          :class="l.dir"
          @click="jump(l.no)"
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
        <div v-if="!live.length" class="ms-empty">
          无书签。点击日志行选中后，按 Ctrl+F2 或工具栏“书签”按钮添加；行可能随缓冲上限（50000 行）滚动淘汰
        </div>
      </div>
      <div class="bm-foot" v-if="live.length">
        <button class="btn btn-ghost btn-sm" @click="store.clearBookmarks(active.id)">
          清空全部
        </button>
      </div>
    </div>
    <div v-else class="panel-empty">无活动会话</div>
  </details>
</template>
