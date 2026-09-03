<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useUpdateChecker } from '../composables/useUpdateChecker'
import { usePortCfg, useOpenLog } from '../composables/usePortConfig'
import SettingsPopover from './SettingsPopover.vue'

// PortBar 退役（布局重构 V1）：设置弹层挂进标题行，cfg/openLog 走共享 composable
const { cfg, applyPreset } = usePortCfg()
const { openLog } = useOpenLog()

// 去掉原生标题栏后的自定义标题栏：深色、可拖拽、自带最小化/最大化-还原/关闭。
// 浏览器预览（无 Tauri）下这些 API 会失败，静默忽略即可。
// getCurrentWindow 在无 __TAURI_INTERNALS__ 的浏览器里会直接抛错，兜底为 null。
const win = (() => {
  try {
    return getCurrentWindow()
  } catch {
    return null
  }
})()
const maximized = ref(false)
let unlisten: (() => void) | null = null

// 版本徽标 + 检查更新面板：启动静默检查一次（24h 节流），徽标仅在发现新版本时亮起
const {
  status: updateStatus,
  updateInfo,
  currentVersion,
  errorMsg,
  init: initUpdateChecker,
  checkNow,
  dismiss,
  openReleasePage,
} = useUpdateChecker()
const updateOpen = ref(false)
const showUpdateBadge = computed(() => updateStatus.value === 'available')

function dismissUpdate() {
  dismiss()
  updateOpen.value = false
}

onMounted(async () => {
  initUpdateChecker()
  if (!win) return
  try {
    maximized.value = await win.isMaximized()
    unlisten = await win.onResized(async () => {
      try {
        maximized.value = await win.isMaximized()
      } catch {
        /* ignore */
      }
    })
  } catch {
    /* 非 Tauri 环境 */
  }
})
onBeforeUnmount(() => {
  unlisten?.()
})

async function minimize() {
  try {
    await win?.minimize()
  } catch {
    /* ignore */
  }
}
async function toggleMax() {
  try {
    await win?.toggleMaximize()
  } catch {
    /* ignore */
  }
}
async function close() {
  try {
    await win?.close()
  } catch {
    /* ignore */
  }
}
</script>

<template>
  <div class="titlebar" data-tauri-drag-region>
    <div class="titlebar-brand" data-tauri-drag-region>
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      >
        <polyline points="3 15 7 15 7 9 11 9 11 15 15 15 15 9 19 9" />
        <circle cx="19" cy="9" r="1.9" style="fill: var(--tx)" stroke="none" />
      </svg>
      <span>ByteTide <span class="titlebar-sub">字节潮</span></span>
    </div>
    <div class="titlebar-spacer" data-tauri-drag-region></div>
    <div class="titlebar-actions">
      <SettingsPopover :cfg="cfg" @open-log="openLog" @apply-preset="applyPreset" />
    </div>
    <div class="titlebar-update">
      <button
        class="titlebar-ver"
        :class="{ 'has-update': showUpdateBadge }"
        title="检查更新"
        aria-label="检查更新"
        @click="updateOpen = !updateOpen"
      >
        <span>v{{ currentVersion || '—' }}</span>
        <span v-if="showUpdateBadge" class="ver-dot" aria-hidden="true"></span>
      </button>
      <div v-if="updateOpen" class="update-backdrop" @click="updateOpen = false"></div>
      <div v-if="updateOpen" class="update-pop" role="dialog" aria-label="检查更新">
        <div class="update-pop-head">
          <span>检查更新</span>
          <button class="update-x" title="关闭" aria-label="关闭更新面板" @click="updateOpen = false">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M18 6 6 18" />
              <path d="m6 6 12 12" />
            </svg>
          </button>
        </div>
        <div class="update-pop-body">
          <template v-if="updateStatus === 'available' && updateInfo">
            <p class="update-line">
              发现新版本
              <strong class="update-ver">v{{ updateInfo.version }}</strong>
              <span class="update-cur">当前 v{{ currentVersion }}</span>
            </p>
            <pre v-if="updateInfo.notes" class="update-notes">{{ updateInfo.notes }}</pre>
            <div class="update-actions">
              <button class="btn btn-primary btn-sm" @click="openReleasePage">前往下载页</button>
              <button class="btn btn-ghost btn-sm" @click="dismissUpdate">忽略此版本</button>
            </div>
          </template>
          <template v-else-if="updateStatus === 'checking'">
            <p class="update-line">正在检查更新…</p>
          </template>
          <template v-else-if="updateStatus === 'latest'">
            <p class="update-line">已是最新版本（v{{ currentVersion }}）</p>
          </template>
          <template v-else-if="updateStatus === 'error'">
            <p class="update-line update-err">检查失败：{{ errorMsg || '网络不可用' }}</p>
            <div class="update-actions">
              <button class="btn btn-primary btn-sm" @click="checkNow(true)">重试</button>
              <button class="btn btn-ghost btn-sm" @click="openReleasePage">前往 Releases 页</button>
            </div>
          </template>
          <template v-else-if="updateStatus === 'unconfigured'">
            <p class="update-line">更新仓库尚未配置，请前往 GitHub Releases 页手动下载新版本。</p>
          </template>
          <template v-else>
            <p class="update-line">当前版本 v{{ currentVersion || '—' }}</p>
            <div class="update-actions">
              <button class="btn btn-primary btn-sm" @click="checkNow(true)">检查更新</button>
            </div>
          </template>
        </div>
      </div>
    </div>
    <div class="titlebar-controls">
      <button class="titlebar-btn" title="最小化" aria-label="最小化" @click="minimize">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M5 12h14" />
        </svg>
      </button>
      <button class="titlebar-btn" title="最大化 / 还原" aria-label="最大化或还原" @click="toggleMax">
        <svg
          v-if="!maximized"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linejoin="round"
        >
          <rect x="5" y="5" width="14" height="14" rx="1.5" />
        </svg>
        <svg
          v-else
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linejoin="round"
        >
          <rect x="8" y="8" width="13" height="13" rx="2" />
          <path d="M3 15V5a2 2 0 0 1 2-2h10" />
        </svg>
      </button>
      <button class="titlebar-btn close" title="关闭" aria-label="关闭" @click="close">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M18 6 6 18" />
          <path d="m6 6 12 12" />
        </svg>
      </button>
    </div>
  </div>
</template>
