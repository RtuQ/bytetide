<script setup lang="ts">
import { computed } from 'vue'
import { useSessionStore } from '../stores/session'
import { useAlertStore } from '../stores/alerts'
import type { AlertLevel, AlertRule } from '../types'

const store = useSessionStore()
const alerts = useAlertStore()
const active = computed(() => store.active)
alerts.load()

const LEVELS: { key: AlertLevel; label: string }[] = [
  { key: 'info', label: '提示' },
  { key: 'warn', label: '警告' },
  { key: 'err', label: '错误' },
]

function add() {
  if (active.value) store.addAlertRule(active.value.id)
}
function upd(rid: string, patch: Partial<AlertRule>) {
  if (active.value) store.updateAlertRule(active.value.id, rid, patch)
}
function del(rid: string) {
  if (active.value) store.removeAlertRule(active.value.id, rid)
}
function fmtTime(at: number) {
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`
}
/** 点击历史：切到来源会话并跳转到命中行 */
function jumpToHit(sessionId: string, no: number) {
  if (!store.sessions[sessionId]) return
  store.setActive(sessionId)
  if (no > 0) store.requestJump(sessionId, no)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>
      <span class="panel-title">告警</span>
      <span v-if="active" class="badge">{{ active.alerts.rules.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="ar-body" v-if="active">
      <div class="ar-line">
        <label class="check ar-master">
          <input
            type="checkbox"
            :checked="active.alerts.enabled"
            @change="store.setAlertsEnabled(active.id, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>启用告警</span>
        </label>
        <span class="send-spacer"></span>
        <label class="check" title="触发时播放提示音">
          <input
            type="checkbox"
            :checked="alerts.sound"
            @change="alerts.setSound(($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>提示音</span>
        </label>
        <button class="btn btn-ghost btn-sm" @click="add">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
          <span>添加规则</span>
        </button>
      </div>
      <div v-if="!active.alerts.rules.length" class="panel-hint">
        RX 行命中规则时弹系统通知并记入历史；可设次数窗口与冷却防刷屏。仅对实时接收生效。
      </div>

      <div v-for="r in active.alerts.rules" :key="r.id" class="ar-card" :class="{ off: !r.enabled }">
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
            class="ar-reply"
            :value="r.pattern"
            @input="upd(r.id, { pattern: ($event.target as HTMLInputElement).value })"
            placeholder="匹配内容，如 ERROR|ASSERT"
          />
          <button class="ar-x" @click="del(r.id)" title="删除" aria-label="删除规则">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <div class="ar-opts">
          <div class="seg">
            <button
              v-for="lv in LEVELS"
              :key="lv.key"
              class="seg-item"
              :class="{ active: r.level === lv.key }"
              @click="upd(r.id, { level: lv.key })"
            >
              {{ lv.label }}
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
        </div>
        <div class="ar-opts al-nums">
          <span class="al-num-field">
            <input
              class="al-num"
              type="number"
              min="1"
              :value="r.minCount"
              @change="upd(r.id, { minCount: Math.max(1, Number(($event.target as HTMLInputElement).value) || 1) })"
            />
            次<span title="窗口内达到该次数才触发">≥</span>
          </span>
          <span class="al-num-field">
            <input
              class="al-num"
              type="number"
              min="0"
              step="1"
              :value="r.windowSec"
              @change="upd(r.id, { windowSec: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
            />
            s 窗口
          </span>
          <span class="al-num-field">
            <input
              class="al-num"
              type="number"
              min="0"
              step="1"
              :value="r.cooldownSec"
              @change="upd(r.id, { cooldownSec: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
            />
            s 冷却
          </span>
        </div>
      </div>

      <div class="ar-line al-hist-head">
        <span class="al-hist-title">历史（{{ alerts.hits.length }}）</span>
        <span class="send-spacer"></span>
        <button class="btn btn-ghost btn-sm" :disabled="!alerts.hits.length" @click="alerts.clear()">
          清空
        </button>
      </div>
      <div v-if="!alerts.hits.length" class="panel-hint">暂无告警记录</div>
      <div
        v-for="h in alerts.hits"
        :key="h.id"
        class="al-hit"
        @click="jumpToHit(h.sessionId, h.no)"
        :title="h.sessionName + ' #' + h.no + '，点击跳转'"
      >
        <span class="lv-dot" :class="'lv-' + h.level"></span>
        <span class="al-time">{{ fmtTime(h.at) }}</span>
        <span class="al-name">{{ h.sessionName }}</span>
        <span class="al-text">{{ h.text }}</span>
      </div>
    </div>
    <div v-else class="panel-empty">无活动会话</div>
  </details>
</template>
