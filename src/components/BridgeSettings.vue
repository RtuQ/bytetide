<script setup lang="ts">
import { computed } from 'vue'
import { useBridgeStore } from '../stores/bridge'
import type { BridgeConfig } from '../types'

// 内容组件：由 SettingsPopover 承载（自身不带触发按钮与浮层壳）
const bridge = useBridgeStore()
const cfg = computed(() => bridge.config)

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

  <div class="row2">
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

  <div class="row2">
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

  <div class="row2">
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
</template>
