<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import { DEFAULT_LOG_CONFIG } from '../types'
import { t } from '../i18n'

// 内容组件：由 SettingsPopover 承载（自身不带触发按钮与浮层壳）
const store = useSessionStore()

function onPath(e: Event) {
  store.setLogConfig({ logPathTemplate: (e.target as HTMLInputElement).value })
}
function onTs(e: Event) {
  store.setLogConfig({ lineTsFormat: (e.target as HTMLInputElement).value })
}
function onBufCap(e: Event) {
  store.setLogConfig({ viewBufCap: Number((e.target as HTMLSelectElement).value) })
}
function onMidnight(e: Event) {
  store.setLogConfig({ midnightRotate: (e.target as HTMLInputElement).checked })
}
function resetDefaults() {
  store.setLogConfig({ ...DEFAULT_LOG_CONFIG })
}
</script>

<template>
  <div class="field">
    <span class="field-label">{{ t('lv.logset.pathLabel') }}</span>
    <input
      class="input input-mono"
      :value="store.logConfig.logPathTemplate"
      @input="onPath"
      placeholder="D:\log\%H\%Y-%M-%D_%h%m%s.log"
      spellcheck="false"
    />
    <span class="panel-hint">{{ t('lv.logset.pathHint') }}</span>
  </div>
  <div class="field">
    <span class="field-label">{{ t('lv.logset.tsLabel') }}</span>
    <input
      class="input input-mono"
      :value="store.logConfig.lineTsFormat"
      @input="onTs"
      placeholder="[%Y-%M-%D %h:%m:%s.%t]"
      spellcheck="false"
    />
    <span class="panel-hint">{{ t('lv.logset.tsHint') }}</span>
  </div>
  <div class="logcfg-tokens">
    <span class="tk">%Y</span>{{ t('lv.logset.tokYear') }}
    <span class="tk">%M</span>{{ t('lv.logset.tokMonth') }}
    <span class="tk">%D</span>{{ t('lv.logset.tokDay') }}
    <span class="tk">%H</span>{{ t('lv.logset.tokPort') }}
    <span class="tk">%S</span>{{ t('lv.logset.tokHost') }}
    <span class="tk">%h</span>{{ t('lv.logset.tokHour') }}
    <span class="tk">%m</span>{{ t('lv.logset.tokMin') }}
    <span class="tk">%s</span>{{ t('lv.logset.tokSec') }}
    <span class="tk">%t</span>{{ t('lv.logset.tokMs') }}
    <span class="tk">%%</span>%
  </div>
  <div class="field">
    <span class="field-label">{{ t('lv.logset.bufLabel') }}</span>
    <select class="select" :value="store.logConfig.viewBufCap" @change="onBufCap">
      <option :value="50000">{{ t('lv.logset.buf50k') }}</option>
      <option :value="100000">{{ t('lv.logset.buf100k') }}</option>
      <option :value="200000">{{ t('lv.logset.buf200k') }}</option>
      <option :value="500000">{{ t('lv.logset.buf500k') }}</option>
    </select>
    <span class="panel-hint">{{ t('lv.logset.bufHint') }}</span>
  </div>
  <label class="check">
    <input
      type="checkbox"
      :checked="store.logConfig.midnightRotate"
      @change="onMidnight"
    />
    <span class="box">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
    </span>
    <span>{{ t('lv.logset.midnight') }}</span>
  </label>
  <span class="panel-hint">{{ t('lv.logset.midnightHint') }}</span>
  <span class="panel-hint">{{ t('lv.logset.appendHint') }}</span>
  <div class="sm-foot">
    <span class="panel-hint">{{ t('lv.logset.newSessionHint') }}</span>
    <button class="btn btn-sm btn-ghost" type="button" @click="resetDefaults">{{ t('lv.logset.reset') }}</button>
  </div>
</template>
