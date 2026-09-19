<script setup lang="ts">
import { computed } from 'vue'
import { useSessionStore } from '../stores/session'
import { useRate, humanizeBytes } from '../composables/useRate'
import { usePerfWatch } from '../composables/usePerfWatch'
import type { SessionStatus } from '../types'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'

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

// code→词条映射：值存 MessageKey，使用点 t(STATUS_KEY[status]) 求值（切语言即时刷新）
const STATUS_KEY: Record<SessionStatus, MessageKey> = {
  connected: 'app.status.connected',
  connecting: 'app.status.connecting',
  disconnected: 'app.status.disconnected',
  error: 'app.status.error',
  offline: 'app.status.offline',
}

/** 现场捕获 armed：活动会话正在写后续窗口（呼吸指示；capture-saved 解除） */
const capActive = computed(() =>
  active.value ? store.captureActive[active.value.id] : undefined,
)

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
      <span class="sb-dot" :class="active.status" :title="t(STATUS_KEY[active.status])"></span>
      <b class="sb-name">{{ active.config.name || active.id }}</b>
      <span class="sb-status">{{ t(STATUS_KEY[active.status]) }}</span>
      <span class="sb-dim">{{ linkText }}</span>
    </span>
    <span class="sb-sect sb-mono">
      <span class="sb-rx" :title="t('app.status.rxRate')">↓ {{ humanizeBytes(rxBps) }}/s</span>
      <span class="sb-tx" :title="t('app.status.txRate')">↑ {{ humanizeBytes(txBps) }}/s</span>
    </span>
    <span v-if="active.droppedLines > 0" class="sb-sect">
      <span class="sb-bad" :title="t('app.status.droppedTitle')">{{ t('app.status.dropped', { count: active.droppedLines }) }}</span>
    </span>
    <span v-if="active.ringDropped > 0" class="sb-sect">
      <span class="sb-bad" :title="t('app.status.ringDroppedTitle')">{{ t('app.status.ringDropped', { count: active.ringDropped }) }}</span>
    </span>
    <span
      v-if="capActive"
      class="sb-sect sb-cap"
      :title="t('app.status.capturingTitle', { pattern: capActive })"
    >
      <span class="sb-cap-dot"></span>{{ t('app.status.capturing') }}
    </span>
    <span class="sb-sect sb-dim" :class="perfCls" :title="t('app.status.lagTitle')">
      {{ t('app.status.lag', { lag: perf.lagMs.value, batch: perf.batchCostMs.value }) }}
    </span>
    <span class="sb-spacer"></span>
    <span class="sb-sect sb-mono sb-dim">{{ t('app.status.lines', { rx: active.rxLines.toLocaleString(), tx: active.txLines.toLocaleString() }) }}</span>
  </footer>
  <footer v-else class="statusbar">
    <span class="sb-sect sb-dim">{{ t('app.status.noSession') }}</span>
    <span class="sb-spacer"></span>
  </footer>
</template>
