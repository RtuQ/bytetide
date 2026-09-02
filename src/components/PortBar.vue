<script setup lang="ts">
import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { useSessionStore } from '../stores/session'
import NewConnectionPopover from './NewConnectionPopover.vue'
import SettingsPopover from './SettingsPopover.vue'
import type { PortConfig } from '../types'

const store = useSessionStore()

const DEFAULT_CFG: PortConfig = {
  name: '',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
}
const STORAGE_KEY = 'serialtool.lastPortConfig'

function loadCfg(): PortConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) return { ...DEFAULT_CFG, ...(JSON.parse(raw) as Partial<PortConfig>) }
  } catch {
    /* ignore */
  }
  return { ...DEFAULT_CFG }
}
function saveCfg(c: PortConfig) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(c))
  } catch {
    /* ignore */
  }
}

/** 载入并补齐网络源字段（旧存档无这些键） */
function normalizedCfg(): PortConfig {
  const c = loadCfg()
  return {
    ...c,
    transport: (c.transport as PortConfig['transport']) ?? 'serial',
    tcpHost: c.tcpHost ?? '',
    tcpPort: c.tcpPort ?? null,
    udpLocalPort: c.udpLocalPort ?? null,
  }
}
const cfg = ref<PortConfig>(normalizedCfg())

// 打开日志文件离线分析：对话框选文件 -> 后端读取 -> 解析载入为离线标签页
const opening = ref(false)
async function openLog() {
  if (opening.value) return
  opening.value = true
  try {
    const sel = await open({
      multiple: false,
      filters: [{ name: 'Log', extensions: ['log', 'txt', 'tsv', 'csv'] }],
    })
    const path = typeof sel === 'string' ? sel : Array.isArray(sel) ? sel[0] : null
    if (!path) return
    await store.loadOfflineSession(path)
  } catch (e: unknown) {
    alert(String(e instanceof Error ? e.message : e))
  } finally {
    opening.value = false
  }
}

function onApplyPreset(c: PortConfig) {
  cfg.value = { ...c }
}
</script>

<template>
  <div class="portbar">
    <NewConnectionPopover :cfg="cfg" @connected="saveCfg(cfg)" />
    <button
      class="btn btn-ghost"
      type="button"
      :disabled="opening"
      :title="opening ? '打开中…' : '打开日志文件进行离线分析'"
      @click="openLog"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
      <span>{{ opening ? '打开中…' : '打开日志' }}</span>
    </button>
    <span class="sp"></span>
    <SettingsPopover :cfg="cfg" @open-log="openLog" @apply-preset="onApplyPreset" />
  </div>
</template>
