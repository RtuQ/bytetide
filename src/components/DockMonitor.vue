<script setup lang="ts">
import { computed, ref } from 'vue'
import { save } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore } from '../stores/session'
import { useLineStats } from '../composables/useLineStats'
import { usePerfWatch } from '../composables/usePerfWatch'
import { humanizeBytes, useRate } from '../composables/useRate'
import MiniChart from './MiniChart.vue'

/** 监控页签（自 MatchStats 整体迁入，计算逻辑原样搬运；跟随活动会话） */
const store = useSessionStore()
const active = computed(() => store.active)
const perf = usePerfWatch()

const perfCls = computed(() =>
  perf.lagMs.value >= 2000 ? 'bad' : perf.lagMs.value >= 300 ? 'warn' : 'ok',
)
function fmtT(at: number) {
  const d = new Date(at)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}:${String(d.getSeconds()).padStart(2, '0')}`
}

// 导出状态反馈：'idle' | 'pending' | 'ok' | 'err'，3s 后自动清回 idle
const exportStatus = ref<{ kind: 'idle' | 'pending' | 'ok' | 'err'; msg?: string }>({
  kind: 'idle',
})
let exportTimer: ReturnType<typeof setTimeout> | null = null
function setExportStatus(kind: 'ok' | 'err', msg: string) {
  exportStatus.value = { kind, msg }
  if (exportTimer) clearTimeout(exportTimer)
  exportTimer = setTimeout(() => {
    exportStatus.value = { kind: 'idle' }
    exportTimer = null
  }, 3000)
}
async function exportDiag() {
  // 防止连点导致多个保存对话框
  if (exportStatus.value.kind === 'pending') return
  let path: string | null = null
  try {
    path = await save({
      defaultPath: `perf-diag-${new Date().toISOString().slice(0, 10)}.json`,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    })
  } catch (e) {
    setExportStatus('err', `对话框失败：${String(e)}`)
    return
  }
  if (!path) return
  exportStatus.value = { kind: 'pending' }
  try {
    await invoke('export_text_cmd', {
      path,
      content: JSON.stringify({ exportedAt: Date.now(), entries: perf.entries.value }, null, 2),
    })
    setExportStatus('ok', path)
  } catch (e) {
    setExportStatus('err', String(e))
  }
}

// RX/TX 字节速率（1s 采样，与会话切换解耦：切到累计值更小的会话自动归零）
const rxBps = useRate(() => active.value?.rxBytes ?? 0)
const txBps = useRate(() => active.value?.txBytes ?? 0)

// 行速率/字节速率曲线/RX Δ间隔（随 lineCounter 版本节流重算）
const totalBytes = computed(() =>
  active.value ? (active.value.rxBytes ?? 0) + (active.value.txBytes ?? 0) : 0,
)
const { lineHist, gapStats: gaps, gapSamples, byteHist } = useLineStats(
  () => active.value?.lines ?? [],
  () => totalBytes.value,
  () => active.value?.lineCounter ?? 0,
)
const lineVals = computed(() => lineHist.value.map((b) => b.lines))
const maxLineRate = computed(() => Math.max(0, ...lineVals.value))
const lastByteRate = computed(() =>
  byteHist.value.length ? byteHist.value[byteHist.value.length - 1]! : 0,
)
</script>

<template>
  <div class="dock-monitor">
    <template v-if="active">
      <div class="perf-line" :class="perfCls" title="显示滞后=当前墙钟−最新行后端时间戳；批均=单批次处理耗时">
        <span class="perf-dot"></span>
        处理延迟 <b>{{ perf.lagMs.value }}</b>ms · 批均 {{ perf.batchCostMs.value }}ms
        <span
          v-if="perf.entries.value.length"
          class="perf-exp"
          :class="`s-${exportStatus.kind}`"
          role="button"
          :title="exportStatus.kind === 'pending' ? '导出中…' : exportStatus.kind === 'ok' ? `已导出：${exportStatus.msg}` : exportStatus.kind === 'err' ? `导出失败：${exportStatus.msg}` : '导出诊断记录 JSON'"
          @click.stop="exportDiag"
        >{{ exportStatus.kind === 'pending' ? '导出中…' : exportStatus.kind === 'ok' ? '已导出' : exportStatus.kind === 'err' ? '导出失败' : '导出诊断' }}</span>
      </div>
      <details v-if="perf.entries.value.length" class="perf-detail">
        <summary>诊断记录（{{ perf.entries.value.length }}）</summary>
        <div class="perf-rows">
          <div v-for="(d, i) in perf.entries.value.slice(0, 80)" :key="d.at + '-' + i" :class="{ 'perf-row-hidden': d.vis === 'hidden' }">
            {{ fmtT(d.at) }} {{ d.kind }} lag={{ d.lagMs }}ms batch={{ d.batchMs }}ms n={{ d.lines }} vis={{ d.vis }} [{{ d.sessionId }}]
          </div>
        </div>
      </details>
      <div class="dock-mon-body">
        <div class="dock-mon-cards">
          <div class="dock-mon-card" title="最近 1 秒接收字节数">
            <div class="dock-mon-k">RX 速率</div>
            <div class="dock-mon-v rx">{{ humanizeBytes(rxBps) }}<small>/s</small></div>
          </div>
          <div class="dock-mon-card" title="最近 1 秒发送字节数">
            <div class="dock-mon-k">TX 速率</div>
            <div class="dock-mon-v tx">{{ humanizeBytes(txBps) }}<small>/s</small></div>
          </div>
          <div class="dock-mon-card" :title="`缓冲行数 ${active.lines.length.toLocaleString()}`">
            <div class="dock-mon-k">RX / TX 行数</div>
            <div class="dock-mon-v">{{ active.rxLines.toLocaleString() }} / {{ active.txLines.toLocaleString() }}</div>
          </div>
          <div class="dock-mon-card" title="前端缓冲上限（50k 行）裁剪掉的行数累计">
            <div class="dock-mon-k">丢行</div>
            <div class="dock-mon-v" :class="{ bad: active.droppedLines > 0 }">{{
              active.droppedLines.toLocaleString()
            }}</div>
          </div>
        </div>
        <div class="dock-mon-charts">
          <div class="dock-mon-chart">
            <div class="dock-mon-cl"><span>行速率（60s）</span><span>峰值 <b>{{ maxLineRate }}</b>/s</span></div>
            <div class="dock-mon-canvas">
              <MiniChart :values="lineVals" color="var(--accent)" />
            </div>
          </div>
          <div class="dock-mon-chart">
            <div class="dock-mon-cl"><span>字节速率（60s）</span><span>当前 <b>{{ humanizeBytes(lastByteRate) }}</b>/s</span></div>
            <div class="dock-mon-canvas">
              <MiniChart :values="byteHist" color="var(--rx)" />
            </div>
          </div>
          <div class="dock-mon-chart">
            <div class="dock-mon-cl">
              <span>RX Δ间隔（{{ gaps.count }} 对）</span>
              <span>min {{ gaps.min }} · avg {{ gaps.avg }} · p95 <b>{{ gaps.p95 }}</b> · max {{ gaps.max }} ms</span>
            </div>
            <div class="dock-mon-canvas">
              <MiniChart :values="gapSamples" color="var(--tx)" />
            </div>
          </div>
        </div>
      </div>
    </template>
    <div v-else class="dock-empty">无活动会话</div>
  </div>
</template>
