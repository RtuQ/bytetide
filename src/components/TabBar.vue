<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import { usePortCfg, useOpenLog } from '../composables/usePortConfig'
import NewConnectionPopover from './NewConnectionPopover.vue'

const store = useSessionStore()
const { cfg, saveCfg } = usePortCfg()
const { opening, openLog } = useOpenLog()
</script>

<template>
  <div class="tabbar">
    <NewConnectionPopover :cfg="cfg" @connected="saveCfg" />
    <div class="tabs-scroll">
      <div
        v-for="s in store.sessionList"
        :key="s.id"
        class="tab"
        :class="{ active: s.id === store.activeId }"
        @click="store.setActive(s.id)"
      >
        <span class="dot" :class="s.status" :title="s.status"></span>
        <span class="name" :title="s.config.name">{{ s.kind === 'offline' ? s.config.name : `${s.config.name || '?'}@${s.config.baudRate}` }}</span>
        <button
          class="close"
          title="关闭"
          aria-label="关闭标签"
          @click.stop="store.closeTab(s.id)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
        </button>
      </div>
      <div v-if="!store.sessionList.length" class="tabbar-empty">尚未打开串口</div>
    </div>
    <div class="tabbar-tail">
      <button
        class="btn btn-ghost btn-icon"
        type="button"
        :disabled="opening"
        :title="opening ? '打开中…' : '打开日志文件进行离线分析'"
        aria-label="打开日志"
        @click="openLog"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
      </button>
    </div>
  </div>
</template>
