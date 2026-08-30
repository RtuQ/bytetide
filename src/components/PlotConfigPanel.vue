<script setup lang="ts">
import { computed, inject } from 'vue'
import { useSessionStore } from '../stores/session'
import { PLOT_DATA_KEY } from '../composables/usePlotData'
import type { PlotBytes, PlotChecksum, PlotConfig } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)
const plot = computed(() => active.value?.plot)
const { points, frameCount, lastError } = inject(PLOT_DATA_KEY)!

function patch(p: Partial<PlotConfig>) {
  if (active.value) store.updatePlot(active.value.id, p)
}
function onEnabled(e: Event) {
  if (active.value) store.setPlotEnabled(active.value.id, (e.target as HTMLInputElement).checked)
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 3v18h18"/><path d="m19 9-5 5-4-4-3 3"/></svg>
      <span class="panel-title">数据绘图</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active && plot">
      <label class="check">
        <input type="checkbox" :checked="plot.enabled" @change="onEnabled" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>绘图（开启后切到主区域波形图，自动启用 HEX 视图）</span>
      </label>

      <div class="field">
        <span class="field-label">数据源</span>
        <div class="seg">
          <button class="seg-item" :class="{ active: plot.source === 'binary' }" @click="patch({ source: 'binary' })">二进制</button>
          <button class="seg-item" :class="{ active: plot.source === 'ascii-hex' }" @click="patch({ source: 'ascii-hex' })">ASCII hex</button>
        </div>
      </div>

      <div class="field">
        <span class="field-label">帧头 (hex)</span>
        <input
          class="input input-mono"
          :value="plot.frameHead"
          @input="patch({ frameHead: ($event.target as HTMLInputElement).value })"
          placeholder="如 01 00"
        />
      </div>

      <div class="field">
        <span class="field-label">帧尾 (hex, 可空)</span>
        <input
          class="input input-mono"
          :value="plot.frameTail"
          @input="patch({ frameTail: ($event.target as HTMLInputElement).value })"
          placeholder="如 AA 55"
        />
      </div>

      <div class="plot-grid">
        <div class="field">
          <span class="field-label">校验</span>
          <select
            class="select"
            :value="plot.checksum"
            @change="patch({ checksum: ($event.target as HTMLSelectElement).value as PlotChecksum })"
          >
            <option value="none">无</option>
            <option value="sum">累加和</option>
            <option value="xor">XOR</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">通道数</span>
          <input
            class="input"
            type="number"
            min="1"
            max="8"
            :value="plot.channels"
            @input="patch({ channels: Number(($event.target as HTMLInputElement).value) || 1 })"
          />
        </div>
      </div>

      <div class="plot-grid">
        <div class="field">
          <span class="field-label">每通道字节</span>
          <select
            class="select"
            :value="plot.bytesPerChannel"
            @change="patch({ bytesPerChannel: Number(($event.target as HTMLSelectElement).value) as PlotBytes })"
          >
            <option value="1">1</option>
            <option value="2">2</option>
            <option value="4">4</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">最大点数</span>
          <select
            class="select"
            :value="plot.maxPoints"
            @change="patch({ maxPoints: Number(($event.target as HTMLSelectElement).value) })"
          >
            <option value="1000">1000</option>
            <option value="2000">2000</option>
            <option value="5000">5000</option>
            <option value="10000">10000</option>
          </select>
        </div>
      </div>

      <div class="field">
        <span class="field-label">端序</span>
        <div class="seg">
          <button class="seg-item" :class="{ active: plot.endian === 'big' }" @click="patch({ endian: 'big' })">大端</button>
          <button class="seg-item" :class="{ active: plot.endian === 'little' }" @click="patch({ endian: 'little' })">小端</button>
        </div>
      </div>

      <label class="check">
        <input type="checkbox" :checked="plot.signed" @change="patch({ signed: ($event.target as HTMLInputElement).checked })" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>有符号</span>
      </label>

      <div class="plot-stat">
        <span>解析 <b>{{ frameCount }}</b> 帧 / <b>{{ points.length }}</b> 点</span>
        <span v-if="lastError" class="plot-err">{{ lastError }}</span>
      </div>
    </div>
    <div v-else class="panel-empty">无活动会话</div>
  </details>
</template>
