<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import { DEFAULT_LOG_CONFIG } from '../types'

// 内容组件：由 SettingsPopover 承载（自身不带触发按钮与浮层壳）
const store = useSessionStore()

function onPath(e: Event) {
  store.setLogConfig({ logPathTemplate: (e.target as HTMLInputElement).value })
}
function onTs(e: Event) {
  store.setLogConfig({ lineTsFormat: (e.target as HTMLInputElement).value })
}
function resetDefaults() {
  store.setLogConfig({ ...DEFAULT_LOG_CONFIG })
}
</script>

<template>
  <div class="field">
    <span class="field-label">日志路径模板</span>
    <input
      class="input input-mono"
      :value="store.logConfig.logPathTemplate"
      @input="onPath"
      placeholder="D:\log\%H\%Y-%M-%D_%h%m%s.log"
      spellcheck="false"
    />
    <span class="panel-hint">留空 = 默认路径。支持 %H 端口名、%Y-%M-%D 日期等</span>
  </div>
  <div class="field">
    <span class="field-label">时间戳格式</span>
    <input
      class="input input-mono"
      :value="store.logConfig.lineTsFormat"
      @input="onTs"
      placeholder="[%Y-%M-%D %h:%m:%s.%t]"
      spellcheck="false"
    />
    <span class="panel-hint">留空 = %h:%m:%s.%t（默认）</span>
  </div>
  <div class="logcfg-tokens">
    <span class="tk">%Y</span>年
    <span class="tk">%M</span>月
    <span class="tk">%D</span>日
    <span class="tk">%H</span>端口
    <span class="tk">%h</span>时
    <span class="tk">%m</span>分
    <span class="tk">%s</span>秒
    <span class="tk">%t</span>毫秒
    <span class="tk">%%</span>%
  </div>
  <div class="sm-foot">
    <span class="panel-hint">新建 / 重连会话时生效</span>
    <button class="btn btn-sm btn-ghost" type="button" @click="resetDefaults">恢复默认</button>
  </div>
</template>
