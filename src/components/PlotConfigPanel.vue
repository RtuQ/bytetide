<script setup lang="ts">
import { computed, inject } from 'vue'
import { useSessionStore } from '../stores/session'
import { PLOT_DATA_KEY } from '../composables/usePlotData'
import type { PlotBytes, PlotChecksum, PlotConfig } from '../types'
import { t } from '../i18n'

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
      <span class="panel-title">{{ t('plotcfg.title') }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body" v-if="active && plot">
      <label class="check">
        <input type="checkbox" :checked="plot.enabled" @change="onEnabled" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>{{ t('plotcfg.enable') }}</span>
      </label>

      <div class="field">
        <span class="field-label">{{ t('plotcfg.source') }}</span>
        <div class="seg">
          <button class="seg-item" :class="{ active: plot.source === 'binary' }" @click="patch({ source: 'binary' })">{{ t('plotcfg.sourceBinary') }}</button>
          <button class="seg-item" :class="{ active: plot.source === 'ascii-hex' }" @click="patch({ source: 'ascii-hex' })">{{ t('plotcfg.sourceAsciiHex') }}</button>
        </div>
      </div>

      <div class="field">
        <span class="field-label">{{ t('plotcfg.frameHead') }}</span>
        <input
          class="input input-mono"
          :value="plot.frameHead"
          @input="patch({ frameHead: ($event.target as HTMLInputElement).value })"
          :placeholder="t('plotcfg.frameHeadPh')"
        />
      </div>

      <div class="field">
        <span class="field-label">{{ t('plotcfg.frameTail') }}</span>
        <input
          class="input input-mono"
          :value="plot.frameTail"
          @input="patch({ frameTail: ($event.target as HTMLInputElement).value })"
          :placeholder="t('plotcfg.frameTailPh')"
        />
      </div>

      <div class="plot-grid">
        <div class="field">
          <span class="field-label">{{ t('plotcfg.checksum') }}</span>
          <select
            class="select"
            :value="plot.checksum"
            @change="patch({ checksum: ($event.target as HTMLSelectElement).value as PlotChecksum })"
          >
            <option value="none">{{ t('plotcfg.checksumNone') }}</option>
            <option value="sum">{{ t('plotcfg.checksumSum') }}</option>
            <option value="xor">{{ t('plotcfg.checksumXor') }}</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">{{ t('plotcfg.channels') }}</span>
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
          <span class="field-label">{{ t('plotcfg.bytesPerChannel') }}</span>
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
          <span class="field-label">{{ t('plotcfg.maxPoints') }}</span>
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
        <span class="field-label">{{ t('plotcfg.endian') }}</span>
        <div class="seg">
          <button class="seg-item" :class="{ active: plot.endian === 'big' }" @click="patch({ endian: 'big' })">{{ t('plotcfg.endianBig') }}</button>
          <button class="seg-item" :class="{ active: plot.endian === 'little' }" @click="patch({ endian: 'little' })">{{ t('plotcfg.endianLittle') }}</button>
        </div>
      </div>

      <label class="check">
        <input type="checkbox" :checked="plot.signed" @change="patch({ signed: ($event.target as HTMLInputElement).checked })" />
        <span class="box">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
        </span>
        <span>{{ t('plotcfg.signed') }}</span>
      </label>

      <div class="plot-stat">
        <span>{{ t('plotcfg.stat', { frames: frameCount, points: points.length }) }}</span>
        <span v-if="lastError" class="plot-err">{{ lastError }}</span>
      </div>
    </div>
    <div v-else class="panel-empty">{{ t('plotcfg.noSession') }}</div>
  </details>
</template>
