<script setup lang="ts">
import { computed } from 'vue'
import { useBridgeStore } from '../stores/bridge'
import { useSessionStore } from '../stores/session'
import type { BridgeConfig } from '../types'

const bridge = useBridgeStore()
const cfg = computed(() => bridge.config)

// AI 批注小节：跟随活动会话（REST 桥写入 → 事件实时同步）
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

function patch(p: Partial<BridgeConfig>) {
  bridge.update(p)
}
function onEnabled(e: Event) {
  patch({ enabled: (e.target as HTMLInputElement).checked })
}
function onAllowSend(e: Event) {
  patch({ allowSend: (e.target as HTMLInputElement).checked })
}
function onBind(v: string) {
  patch({ bind: v })
}
function onPort(e: Event) {
  const n = Number((e.target as HTMLInputElement).value)
  if (n > 0 && n < 65536) patch({ port: n })
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M2 12h20"/><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/></svg>
      <span class="panel-title">REST 桥接</span>
      <span v-if="bridge.running" class="bridge-dot" title="桥服务运行中"></span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body">
      <label class="check">
        <input type="checkbox" :checked="cfg.enabled" @change="onEnabled" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>启用桥服务（外部 AI 经 Bearer 令牌读取/分析日志）</span>
      </label>

      <div class="field">
        <span class="field-label">绑定地址</span>
        <div class="seg">
          <button class="seg-item" :class="{ active: cfg.bind === '127.0.0.1' }" @click="onBind('127.0.0.1')">仅本机</button>
          <button class="seg-item" :class="{ active: cfg.bind === '0.0.0.0' }" @click="onBind('0.0.0.0')">全部（远程可达）</button>
        </div>
      </div>

      <div class="plot-grid">
        <div class="field">
          <span class="field-label">端口</span>
          <input
            class="input"
            type="number"
            min="1"
            max="65535"
            :value="cfg.port"
            @change="onPort"
          />
        </div>
        <div class="field">
          <span class="field-label">令牌</span>
          <input class="input input-mono" :value="cfg.token || '（未设置，启用时自动生成）'" readonly />
        </div>
      </div>

      <div class="plot-grid">
        <button class="btn btn-sm" @click="bridge.regenToken()" :disabled="bridge.busy">重置令牌</button>
        <button class="btn btn-sm btn-ghost" @click="bridge.copyToken()" :disabled="!cfg.token">复制令牌</button>
      </div>

      <label class="check">
        <input type="checkbox" :checked="cfg.allowSend" @change="onAllowSend" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>允许 AI 发送 / 交换（命令-响应探测）</span>
      </label>
      <p v-if="cfg.allowSend" class="panel-hint hint-warn">
        注意：开启后 AI 可向设备发送数据。仅在你确需命令-响应探测、且理解发送内容时启用。
      </p>

      <div class="plot-grid">
        <button class="btn btn-sm" @click="bridge.copyUrl()">复制地址</button>
        <span class="bridge-url">{{ bridge.baseUrl }}</span>
      </div>

      <p class="panel-hint">
        <span class="bridge-status" :class="{ on: bridge.running }">
          {{ bridge.running ? '运行中' : '已停止' }}
        </span>
        远程/虚拟机：把地址与令牌复制到 AI 机器，设置环境变量 SERIALTOOL_URL / SERIALTOOL_TOKEN。
      </p>
      <p v-if="bridge.lastError" class="panel-hint hint-warn">{{ bridge.lastError }}</p>

      <!-- AI 批注：AI 经 REST 桥对可疑行留言，日志行标记 + 此处列表实时显示 -->
      <div class="ai-sect">
        <div class="ai-sect-head">
          <span class="field-label">AI 批注</span>
          <span v-if="aiNotes.length" class="badge">{{ aiNotes.length }}</span>
          <button
            v-if="aiNotes.length"
            class="btn btn-ghost btn-sm ai-clear"
            @click="clearNotes()"
          >
            清空
          </button>
        </div>
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
      </div>
    </div>
  </details>
</template>
