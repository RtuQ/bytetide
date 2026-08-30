<script setup lang="ts">
import { ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { DEFAULT_LOG_CONFIG } from '../types'

const store = useSessionStore()
const open = ref(false)

function toggle() {
  open.value = !open.value
}
function close() {
  open.value = false
}
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
  <div class="logcfg">
    <button
      class="btn btn-ghost btn-icon"
      type="button"
      :class="{ 'is-active': open }"
      title="日志设置"
      aria-label="日志设置"
      @click="toggle"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/></svg>
    </button>

    <div v-if="open" class="logcfg-backdrop" @click="close"></div>
    <div v-if="open" class="logcfg-pop" @click.stop>
      <div class="logcfg-pop-head">
        <span>日志设置</span>
        <button class="btn btn-ghost btn-icon btn-sm" type="button" title="关闭" aria-label="关闭" @click="close">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
        </button>
      </div>
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
      <div class="logcfg-foot">
        <span class="panel-hint">新建 / 重连会话时生效</span>
        <button class="btn btn-sm btn-ghost" type="button" @click="resetDefaults">恢复默认</button>
      </div>
    </div>
  </div>
</template>
