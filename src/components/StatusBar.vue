<script setup lang="ts">
import { computed } from 'vue'
import { useSessionStore } from '../stores/session'
import { useRate, humanizeBytes } from '../composables/useRate'
import { usePerfWatch } from '../composables/usePerfWatch'
import type { SessionStatus } from '../types'

// 底部状态栏（布局重构 V1）：连接状态/速率/丢行/渲染健康常驻可见，跟随活动会话。
// 纯展示组件，数据全部来自现有 store/composable；解析器状态位待 plan-parser-v1 落地后点亮。
const store = useSessionStore()
const active = computed(() => store.active)

const rxBps = useRate(() => active.value?.rxBytes ?? 0)
const txBps = useRate(() => active.value?.txBytes ?? 0)
const perf = usePerfWatch()
const perfCls = computed(() =>
  perf.lagMs.value >= 2000 ? 'sb-bad' : perf.lagMs.value >= 300 ? 'sb-warn' : '',
)

const STATUS_TEXT: Record<SessionStatus, string> = {
  connected: '已连接',
  connecting: '连接中',
  disconnected: '已断开',
  error: '错误',
  offline: '离线',
}

/** 端口/传输参数摘要：串口显 115200 8N1；网络源显 host:port */
const linkText = computed(() => {
  const c = active.value?.config
  if (!c) return ''
  if (c.transport && c.transport !== 'serial') {
    const host = c.tcpHost || '?'
    return c.tcpPort ? `${host}:${c.tcpPort}` : host
  }
  const parity = c.parity === 'none' ? 'N' : c.parity === 'even' ? 'E' : c.parity === 'odd' ? 'O' : '?'
  return `${c.baudRate} ${c.dataBits}${parity}${c.stopBits}`
})
</script>

<template>
  <footer class="statusbar" v-if="active">
    <span class="sb-sect">
      <span class="sb-dot" :class="active.status" :title="STATUS_TEXT[active.status]"></span>
      <b class="sb-name">{{ active.config.name || active.id }}</b>
      <span class="sb-dim">{{ linkText }}</span>
    </span>
    <span class="sb-sect sb-mono">
      <span class="sb-rx" title="接收速率">↓ {{ humanizeBytes(rxBps) }}/s</span>
      <span class="sb-tx" title="发送速率">↑ {{ humanizeBytes(txBps) }}/s</span>
    </span>
    <span v-if="active.droppedLines > 0" class="sb-sect">
      <span class="sb-bad" title="前端缓冲裁剪掉的行数（重连迁移保留）">丢行 {{ active.droppedLines }}</span>
    </span>
    <span class="sb-sect sb-dim" :class="perfCls" title="显示滞后=当前墙钟−最新行后端时间戳；批均=单批次处理耗时">
      滞后 {{ perf.lagMs.value }}ms · 批均 {{ perf.batchCostMs.value }}ms
    </span>
    <span class="sb-spacer"></span>
    <span class="sb-sect sb-mono sb-dim">RX {{ active.rxLines.toLocaleString() }} · TX {{ active.txLines.toLocaleString() }} 行</span>
  </footer>
  <footer v-else class="statusbar">
    <span class="sb-sect sb-dim">无活动会话</span>
    <span class="sb-spacer"></span>
  </footer>
</template>
