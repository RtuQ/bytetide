<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import { useAlertStore } from '../stores/alerts'
import type { AlertLevel } from '../types'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'

/** 告警历史（自 AlertPanel 迁出，数据源与行为照搬：告警事件监听 + REST mirror 的内存环形） */
const store = useSessionStore()
const alerts = useAlertStore()
alerts.load()

// code→词条映射：值存 MessageKey，使用点 t() 求值（切语言即时刷新）
const LEVEL_LABEL: Record<AlertLevel, MessageKey> = {
  info: 'alerth.levelInfo',
  warn: 'alerth.levelWarn',
  err: 'alerth.levelErr',
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
      <span class="dock-alerts-title">{{ t('alerth.title', { n: alerts.hits.length }) }}</span>
      <span class="dock-tabs-spacer"></span>
      <button
        class="btn btn-ghost btn-sm"
        :disabled="!alerts.hits.length"
        :title="t('alerth.clearTitle')"
        @click="alerts.clear()"
      >
        {{ t('alerth.clear') }}
      </button>
    </div>
    <div v-if="!alerts.hits.length" class="dock-empty">
      <span>{{ t('alerth.empty') }}</span>
      <small>{{ t('alerth.emptyHint') }}</small>
    </div>
    <div v-else class="dock-alerts-list">
      <div
        v-for="h in alerts.hits"
        :key="h.id"
        class="dock-alert-row"
        @click="jumpToHit(h.sessionId, h.no)"
        :title="t('alerth.rowTitle', { name: h.sessionName, no: h.no })"
      >
        <span class="dock-lvl" :class="'lv-' + h.level">{{ t(LEVEL_LABEL[h.level]) }}</span>
        <span class="dock-alert-time">{{ fmtTime(h.at) }}</span>
        <span class="dock-alert-name">{{ h.sessionName }}</span>
        <span class="dock-alert-text">{{ h.text }}</span>
        <span class="dock-alert-no">{{ fmtNo(h.no) }}</span>
      </div>
    </div>
  </div>
</template>
