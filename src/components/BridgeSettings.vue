<script setup lang="ts">
import { computed, ref } from 'vue'
import { useBridgeStore } from '../stores/bridge'
import { t } from '../i18n'
import type { BridgeConfig } from '../types'

// 内容组件：由 SettingsPopover 承载（自身不带触发按钮与浮层壳）
const bridge = useBridgeStore()
const cfg = computed(() => bridge.config)

/** 远程绑定待确认态：点「全部（远程可达）」先挂警告，确认才真正下发补丁 */
const pendingRemote = ref(false)

/** 运行态徽标文案（disabled/starting/running/error 四态，来自后端 BridgeRuntime） */
const statusLabel = computed(() => {
  switch (bridge.runtime.state) {
    case 'running':
      return t('bridge.running')
    case 'starting':
      return t('bridge.starting')
    case 'error':
      return t('bridge.startFailed')
    default:
      return t('bridge.stopped')
  }
})
/** 绑定失败详情（后端 last_error，仅描述故障，不含令牌） */
const runtimeError = computed(() =>
  bridge.runtime.state === 'error' ? bridge.runtime.lastError : null,
)

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
  if (v !== '0.0.0.0') {
    pendingRemote.value = false
    patch({ bind: v })
    return
  }
  // 远程可达属于对外暴露动作：先出警告，用户显式确认后才带 confirmRemote 下发
  pendingRemote.value = true
}
function confirmRemoteBind() {
  pendingRemote.value = false
  bridge.update({ bind: '0.0.0.0', confirmRemote: true })
}
function onPort(e: Event) {
  const n = Number((e.target as HTMLInputElement).value)
  if (n > 0 && n < 65536) patch({ port: n })
}
</script>

<template>
  <label class="check">
    <input type="checkbox" :checked="cfg.enabled" @change="onEnabled" />
    <span class="box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
    </span>
    <span>{{ t('bridge.enable') }}</span>
  </label>

  <div class="field">
    <span class="field-label">{{ t('bridge.bind') }}</span>
    <div class="seg">
      <button class="seg-item" :class="{ active: cfg.bind === '127.0.0.1' }" @click="onBind('127.0.0.1')">{{ t('bridge.bindLocal') }}</button>
      <button class="seg-item" :class="{ active: cfg.bind === '0.0.0.0' }" @click="onBind('0.0.0.0')">{{ t('bridge.bindAll') }}</button>
    </div>
  </div>

  <p v-if="pendingRemote" class="panel-hint hint-warn" role="alertdialog" :aria-label="t('bridge.remoteConfirmAria')">
    {{ t('bridge.remoteWarning', { mode: t('bridge.bindAll') }) }}
    <span class="row2" style="margin-top: 6px;">
      <button class="btn btn-sm" @click="confirmRemoteBind">{{ t('bridge.confirmRemote') }}</button>
      <button class="btn btn-sm btn-ghost" @click="pendingRemote = false">{{ t('bridge.cancel') }}</button>
    </span>
  </p>

  <div class="row2">
    <div class="field">
      <span class="field-label">{{ t('bridge.port') }}</span>
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
      <span class="field-label">{{ t('bridge.token') }}</span>
      <input class="input input-mono" :value="cfg.token || t('bridge.tokenEmpty')" readonly />
    </div>
  </div>

  <div class="row2">
    <button class="btn btn-sm" @click="bridge.regenToken()" :disabled="bridge.busy">{{ t('bridge.regenToken') }}</button>
    <button class="btn btn-sm btn-ghost" @click="bridge.copyToken()" :disabled="!cfg.token">{{ t('bridge.copyToken') }}</button>
  </div>

  <label class="check">
    <input type="checkbox" :checked="cfg.allowSend" @change="onAllowSend" />
    <span class="box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
    </span>
    <span>{{ t('bridge.allowSend') }}</span>
  </label>
  <p v-if="cfg.allowSend" class="panel-hint hint-warn">{{ t('bridge.allowSendWarn') }}</p>

  <div class="row2">
    <button class="btn btn-sm" @click="bridge.copyUrl()">{{ t('bridge.copyUrl') }}</button>
    <span class="bridge-url">{{ bridge.baseUrl }}</span>
  </div>

  <p class="panel-hint">
    <span
      class="bridge-status"
      :class="{ on: bridge.runtime.state === 'running' }"
      :style="bridge.runtime.state === 'error' ? { color: 'var(--err)' } : undefined"
      role="status"
    >{{ statusLabel }}</span>
    <template v-if="bridge.runtime.state === 'running' && bridge.runtime.bound">{{ t('bridge.listening', { addr: bridge.runtime.bound }) }}</template>
    {{ t('bridge.remoteHint') }}
  </p>
  <p v-if="runtimeError" class="panel-hint" :style="{ color: 'var(--err)' }">
    {{ runtimeError }}{{ t('bridge.runtimeErrorHint') }}
  </p>
  <p v-if="bridge.lastError" class="panel-hint hint-warn">{{ bridge.lastError }}</p>
</template>
