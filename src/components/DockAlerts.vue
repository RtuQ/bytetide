<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import { useAlertStore } from '../stores/alerts'
import type { AlertLevel } from '../types'

/** 告警历史（自 AlertPanel 迁出，数据源与行为照搬：告警事件监听 + REST mirror 的内存环形） */
const store = useSessionStore()
const alerts = useAlertStore()
alerts.load()

const LEVEL_LABEL: Record<AlertLevel, string> = {
  info: '提示',
  warn: '警告',
  err: '错误',
}

function fmtTime(at: number) {
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`
}
function fmtNo(n: number) {
  return n > 0 ? `#${n.toLocaleString()}` : ''
}
/** 点击历史：切到来源会话并跳转到命中行 */
function jumpToHit(sessionId: string, no: number) {
  if (!store.sessions[sessionId]) return
  store.setActive(sessionId)
  if (no > 0) store.requestJump(sessionId, no)
}
</script>

<template>
  <div class="dock-alerts">
    <div class="dock-alerts-head">
      <span class="dock-alerts-title">历史（{{ alerts.hits.length }}）</span>
      <span class="dock-tabs-spacer"></span>
      <button
        class="btn btn-ghost btn-sm"
        :disabled="!alerts.hits.length"
        title="清空全部告警历史"
        @click="alerts.clear()"
      >
        清空
      </button>
    </div>
    <div v-if="!alerts.hits.length" class="dock-empty">暂无告警</div>
    <div v-else class="dock-alerts-list">
      <div
        v-for="h in alerts.hits"
        :key="h.id"
        class="dock-alert-row"
        @click="jumpToHit(h.sessionId, h.no)"
        :title="h.sessionName + ' #' + h.no + '，点击跳转'"
      >
        <span class="dock-lvl" :class="'lv-' + h.level">{{ LEVEL_LABEL[h.level] }}</span>
        <span class="dock-alert-time">{{ fmtTime(h.at) }}</span>
        <span class="dock-alert-name">{{ h.sessionName }}</span>
        <span class="dock-alert-text">{{ h.text }}</span>
        <span class="dock-alert-no">{{ fmtNo(h.no) }}</span>
      </div>
    </div>
  </div>
</template>
