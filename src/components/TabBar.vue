<script setup lang="ts">
import { useSessionStore } from '../stores/session'

const store = useSessionStore()
</script>

<template>
  <div class="tabbar">
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
</template>
