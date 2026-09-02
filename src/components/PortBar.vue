<script setup lang="ts">
import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { useSessionStore } from '../stores/session'
import { theme, toggleTheme } from '../composables/useTheme'
import LogSettingsPanel from './LogSettingsPanel.vue'
import PresetsPanel from './PresetsPanel.vue'
import BridgePopover from './BridgePopover.vue'
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
type Transport = NonNullable<PortConfig['transport']>
const transport = ref<Transport>((cfg.value.transport as Transport) ?? 'serial')
function setTransport(t: Transport) {
  transport.value = t
  cfg.value.transport = t === 'serial' ? null : t
}
// 网络源展示名（同时用于日志模板中的 %p 占位与标签页名）
function composeNetName(): string | null {
  const host = (cfg.value.tcpHost ?? '').trim()
  if (transport.value === 'tcp-client') {
    const port = Number(cfg.value.tcpPort)
    if (!host || !port || port <= 0) return null
    return `${host}:${port}`
  }
  const port = transport.value === 'udp' ? Number(cfg.value.udpLocalPort) : Number(cfg.value.tcpPort)
  if (!port || port <= 0) return null
  const bh = transport.value === 'udp' ? (host || '0.0.0.0') : (host || '0.0.0.0')
  return `${transport.value === 'udp' ? 'udp' : 'tcp'}@${bh}:${port}`
}
const baudPresets = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600]
const busy = ref(false)
const opening = ref(false)
const err = ref('')

async function connect() {
  err.value = ''
  if (transport.value !== 'serial') {
    const name = composeNetName()
    if (!name) {
      err.value = transport.value === 'tcp-client' ? '请填写主机和端口' : '请填写端口'
      return
    }
    cfg.value.name = name
    cfg.value.transport = transport.value
  } else {
    cfg.value.name = cfg.value.name.trim()
    if (!cfg.value.name) {
      err.value = '请选择端口'
      return
    }
    cfg.value.transport = null
  }
  busy.value = true
  try {
    await store.openTab({ ...cfg.value })
    saveCfg(cfg.value)
  } catch (e: unknown) {
    err.value = String(e instanceof Error ? e.message : e)
  } finally {
    busy.value = false
  }
}

// 打开日志文件离线分析：对话框选文件 -> 后端读取 -> 解析载入为离线标签页
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
    <label class="field">
      <span class="field-label">数据源</span>
      <div class="seg" role="group" aria-label="数据源类型">
        <button class="seg-item" :class="{ active: transport === 'serial' }" @click="setTransport('serial')">串口</button>
        <button class="seg-item" :class="{ active: transport === 'tcp-client' }" @click="setTransport('tcp-client')">TCP连接</button>
        <button class="seg-item" :class="{ active: transport === 'tcp-server' }" @click="setTransport('tcp-server')">TCP服务</button>
        <button class="seg-item" :class="{ active: transport === 'udp' }" @click="setTransport('udp')">UDP</button>
      </div>
    </label>

    <template v-if="transport === 'serial'">
    <label class="field field-port">
      <span class="field-label">端口</span>
      <select class="select" v-model="cfg.name" title="选择串口">
        <option value="" disabled>选择端口</option>
        <option v-for="p in store.ports" :key="p.name" :value="p.name">
          {{ p.name }}{{ p.product ? ' · ' + p.product : '' }}
        </option>
      </select>
    </label>

    <label class="field">
      <span class="field-label">波特率</span>
      <select class="select" v-model="cfg.baudRate" title="波特率">
        <option v-for="b in baudPresets" :key="b" :value="b">{{ b }}</option>
      </select>
    </label>

    <label class="field">
      <span class="field-label">数据位</span>
      <select class="select" v-model="cfg.dataBits" title="数据位">
        <option :value="8">8</option>
        <option :value="7">7</option>
        <option :value="6">6</option>
        <option :value="5">5</option>
      </select>
    </label>

    <label class="field">
      <span class="field-label">校验</span>
      <select class="select" v-model="cfg.parity" title="校验位">
        <option value="none">N</option>
        <option value="odd">O</option>
        <option value="even">E</option>
      </select>
    </label>

    <label class="field">
      <span class="field-label">停止位</span>
      <select class="select" v-model="cfg.stopBits" title="停止位">
        <option value="1">1</option>
        <option value="2">2</option>
      </select>
    </label>

    <label class="field">
      <span class="field-label">流控</span>
      <select class="select" v-model="cfg.flowControl" title="流控">
        <option value="none">None</option>
        <option value="software">Xon/Xoff</option>
        <option value="hardware">RTS/CTS</option>
      </select>
    </label>
    </template>

    <template v-else>
      <label class="field field-port">
        <span class="field-label">{{ transport === 'tcp-client' ? '主机' : '监听地址（可空）' }}</span>
        <input
          class="input input-mono"
          v-model="cfg.tcpHost"
          :placeholder="transport === 'tcp-client' ? '如 192.168.1.50' : '0.0.0.0'"
        />
      </label>
      <label class="field">
        <span class="field-label">{{ transport === 'udp' ? '本地端口' : '端口' }}</span>
        <input
          class="input input-mono addr-port"
          type="number"
          min="1"
          max="65535"
          :value="transport === 'udp' ? cfg.udpLocalPort : cfg.tcpPort"
          @input="
            transport === 'udp'
              ? (cfg.udpLocalPort = Number(($event.target as HTMLInputElement).value) || null)
              : (cfg.tcpPort = Number(($event.target as HTMLInputElement).value) || null)
          "
          placeholder="如 9000"
        />
      </label>
    </template>

    <div class="portbar-actions">
      <button class="btn btn-primary" :disabled="busy" @click="connect">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h0a4 4 0 0 1-4-4V8Z"/></svg>
        <span>{{ busy ? '连接中…' : '连接' }}</span>
      </button>
      <button
        class="btn btn-ghost btn-icon"
        title="刷新端口列表"
        aria-label="刷新端口列表"
        @click="store.refreshPorts()"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v5h-5"/></svg>
      </button>
      <button
        class="btn btn-ghost btn-sm"
        :disabled="opening"
        :title="opening ? '打开中…' : '打开日志文件进行离线分析'"
        @click="openLog"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z"/></svg>
        <span>{{ opening ? '打开中…' : '打开日志' }}</span>
      </button>
      <LogSettingsPanel />
      <BridgePopover />
      <PresetsPanel :current="cfg" @apply="onApplyPreset" />
      <button
        class="btn btn-ghost"
        :class="{ 'is-on': store.splitMode }"
        :title="store.splitMode ? '退出分屏，回到单列视图' : '在同一界面分屏显示多个端口'"
        @click="store.splitMode ? store.exitSplit() : store.enterSplit()"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="18" rx="1"/><rect x="14" y="3" width="7" height="18" rx="1"/></svg>
        <span>{{ store.splitMode ? '单列' : '分屏' }}</span>
      </button>
      <button
        class="btn btn-ghost btn-icon"
        :title="theme === 'dark' ? '切换到亮色主题' : '切换到深色主题'"
        :aria-label="theme === 'dark' ? '切换到亮色主题' : '切换到深色主题'"
        @click="toggleTheme"
      >
        <svg v-if="theme === 'dark'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/></svg>
        <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
      </button>
    </div>

    <span v-if="err" class="err-msg">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/></svg>
      {{ err }}
    </span>
  </div>
</template>
