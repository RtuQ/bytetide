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
    <span class="tk">%S</span>主机
    <span class="tk">%h</span>时
    <span class="tk">%m</span>分
    <span class="tk">%s</span>秒
    <span class="tk">%t</span>毫秒
    <span class="tk">%%</span>%
  </div>
  <div class="field">
    <span class="field-label">视图缓冲上限</span>
    <select class="select" :value="store.logConfig.viewBufCap" @change="onBufCap">
      <option :value="50000">5 万行</option>
      <option :value="100000">10 万行</option>
      <option :value="200000">20 万行（默认）</option>
      <option :value="500000">50 万行</option>
    </select>
    <span class="panel-hint">超出上限即从最旧行开始裁剪；调大只对之后的行生效，不找回已裁剪的行</span>
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
    <span>午夜自动分段</span>
  </label>
  <span class="panel-hint">连接/重连时生效；录制开启时跨天自动另起新分段文件</span>
  <span class="panel-hint">日志文件按连接追加写入：静态模板路径不会被覆盖，只有清屏会截断当前文件</span>
  <div class="sm-foot">
    <span class="panel-hint">新建 / 重连会话时生效</span>
    <button class="btn btn-sm btn-ghost" type="button" @click="resetDefaults">恢复默认</button>
  </div>
</template>
