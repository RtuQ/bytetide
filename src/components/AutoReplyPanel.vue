<script setup lang="ts">
import { computed } from 'vue'
import { useSessionStore } from '../stores/session'
import type { AutoReplyRule } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)

function add() {
  if (active.value) store.addAutoReplyRule(active.value.id)
}
function upd(rid: string, patch: Partial<AutoReplyRule>) {
  if (active.value) store.updateAutoReplyRule(active.value.id, rid, patch)
}
function del(rid: string) {
  if (active.value) store.removeAutoReplyRule(active.value.id, rid)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 17 4 12 9 7"/><path d="M20 18v-2a4 4 0 0 0-4-4H4"/></svg>
      <span class="panel-title">自动回复</span>
      <span v-if="active" class="badge">{{ active.autoReply.rules.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="ar-body" v-if="active">
      <div class="ar-line">
        <label class="check ar-master">
          <input
            type="checkbox"
            :checked="active.autoReply.enabled"
            @change="
              store.setAutoReplyEnabled(active.id, ($event.target as HTMLInputElement).checked)
            "
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>启用自动回复</span>
        </label>
        <span class="send-spacer"></span>
        <button class="btn btn-ghost btn-sm" @click="add">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
          <span>添加规则</span>
        </button>
      </div>
      <div v-if="!active.autoReply.rules.length" class="panel-hint">
        收到匹配命令自动回复，可设多条规则
      </div>
      <div v-for="r in active.autoReply.rules" :key="r.id" class="ar-card" :class="{ off: !r.enabled }">
        <div class="ar-line">
          <label class="check">
            <input
              type="checkbox"
              :checked="r.enabled"
              @change="upd(r.id, { enabled: ($event.target as HTMLInputElement).checked })"
              title="启用该规则"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
          </label>
          <input
            class="ar-trigger"
            :value="r.trigger"
            @input="upd(r.id, { trigger: ($event.target as HTMLInputElement).value })"
            placeholder="收到命令"
          />
          <span class="ar-arrow">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
          </span>
          <input
            class="ar-reply"
            :value="r.reply"
            @input="upd(r.id, { reply: ($event.target as HTMLInputElement).value })"
            placeholder="回复内容"
          />
          <button class="ar-x" @click="del(r.id)" title="删除" aria-label="删除规则">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div class="ar-opts">
          <div class="seg">
            <button
              class="seg-item"
              :class="{ active: r.replyMode === 'ascii' }"
              @click="upd(r.id, { replyMode: 'ascii' })"
            >
              ASCII
            </button>
            <button
              class="seg-item"
              :class="{ active: r.replyMode === 'hex' }"
              @click="upd(r.id, { replyMode: 'hex' })"
            >
              HEX
            </button>
          </div>
          <label class="check">
            <input
              type="checkbox"
              :checked="r.useRegex"
              @change="upd(r.id, { useRegex: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>正则</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="r.caseSensitive"
              @change="upd(r.id, { caseSensitive: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>大小写</span>
          </label>
          <label class="check">
            <input
              type="checkbox"
              :checked="r.wholeWord"
              @change="upd(r.id, { wholeWord: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>整词</span>
          </label>
          <label v-if="r.replyMode === 'ascii'" class="check">
            <input
              type="checkbox"
              :checked="r.appendNewline"
              @change="upd(r.id, { appendNewline: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>追加换行</span>
          </label>
        </div>
      </div>
    </div>
    <div v-else class="panel-empty">无活动会话</div>
  </details>
</template>
