<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'
import { useSessionStore } from '../stores/session'
import { usePortCfg, useOpenLog } from '../composables/usePortConfig'
import NewConnectionPopover from './NewConnectionPopover.vue'
import { POPOVER_EVENT } from '../composables/usePopoverBridge'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'
import type { SessionStatus } from '../types'

const store = useSessionStore()
const { cfg, saveCfg } = usePortCfg()
const { opening, openLog } = useOpenLog()

// 状态点 tooltip：code→词条映射（值存 MessageKey，使用点 t() 求值）
const STATUS_KEY: Record<SessionStatus, MessageKey> = {
  connected: 'app.status.connected',
  connecting: 'app.status.connecting',
  disconnected: 'app.status.disconnected',
  error: 'app.status.error',
  offline: 'app.status.offline',
}

function onOpenLog(event: Event) {
  if ((event as CustomEvent<string>).detail === 'open-log') openLog()
}
onMounted(() => window.addEventListener(POPOVER_EVENT, onOpenLog))
onBeforeUnmount(() => window.removeEventListener(POPOVER_EVENT, onOpenLog))
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
        <span class="dot" :class="s.status" :title="t(STATUS_KEY[s.status])"></span>
        <!-- live 显示端口@波特率；offline/replay 无链路，只显示文件名（replay
             波特率恒 0，旧实现显示成 文件名@0） -->
        <span class="name" :title="s.config.name">{{ s.kind === 'live' ? `${s.config.name || '?'}@${s.config.baudRate}` : s.config.name }}</span>
        <button
          class="close"
          :title="t('app.common.close')"
          :aria-label="t('app.tab.closeAria')"
          @click.stop="store.closeTab(s.id)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
        </button>
      </div>
    </div>
    <div class="tabbar-tail">
      <button
        class="btn btn-ghost btn-icon"
        type="button"
        :disabled="opening"
        :title="opening ? t('app.tab.opening') : t('app.tab.openLogTitle')"
        :aria-label="t('app.tab.openLogAria')"
        @click="openLog"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
      </button>
    </div>
  </div>
</template>
