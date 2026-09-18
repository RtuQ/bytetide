<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { POPOVER_EVENT, requestPopover } from '../composables/usePopoverBridge'
import { connectionErrorHint } from '../composables/useToast'
import { t } from '../i18n'
import type { PortConfig } from '../types'

/** 新建连接弹层：数据源 + 参数字段 + 从预设。cfg 由 PortBar 持有（记忆/预设回填），此处就地改字段。 */
const props = defineProps<{ cfg: PortConfig }>()
const emit = defineEmits<{ connected: [] }>()

const store = useSessionStore()
const open = ref(false)
const busy = ref(false)
const err = ref('')
// hint 对正常断连类文案返回 null（连接失败不会是断连，兜底一份默认文案保卡片四行齐全）
const errorHint = computed(() =>
  err.value ? (connectionErrorHint(err.value) ?? { title: t('conn.failTitle'), action: t('conn.failAction') }) : null,
)

function toggle() {
  open.value = !open.value
  if (open.value) err.value = ''
  if (open.value) requestPopover('new-connection')
}
function close() {
  open.value = false
}

function onPopover(event: Event) {
  const name = (event as CustomEvent<string>).detail
  if (name === 'new-connection') {
    open.value = true
    err.value = ''
  } else close()
}
function onDocumentPointerDown(event: PointerEvent) {
  const target = event.target as HTMLElement
  if (!target.closest('.nc-pop') && !target.closest('.nc-trigger')) close()
}
onMounted(() => {
  window.addEventListener(POPOVER_EVENT, onPopover)
  document.addEventListener('pointerdown', onDocumentPointerDown)
})
onBeforeUnmount(() => {
  window.removeEventListener(POPOVER_EVENT, onPopover)
  document.removeEventListener('pointerdown', onDocumentPointerDown)
})

type Transport = NonNullable<PortConfig['transport']>
const transport = computed<Transport>({
  get: () => (props.cfg.transport as Transport) ?? 'serial',
  set(t) {
    props.cfg.transport = t === 'serial' ? null : t
  },
})

const baudPresets = [9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600]

// 波特率两态：常用值下拉 / 自定义输入。cfg.baudRate 始终存数字（TabBar/状态栏直接读），
// 自由输入的合法性统一在 connect() 校验回写；非法输入暂存 0，由校验拦下。
const baudCustom = ref(false)
const baudText = ref('115200')
// 当前值不是常用值且合法时，下拉里动态补一项展示，保证显示与实际连接值一致
const showCustomBaudOption = computed(() => {
  const b = Number(props.cfg.baudRate)
  return Number.isInteger(b) && b >= 1 && !baudPresets.includes(b)
})
function onBaudSelect(e: Event) {
  const v = (e.target as HTMLSelectElement).value
  if (v === 'custom') {
    baudText.value = String(props.cfg.baudRate ?? '')
    baudCustom.value = true
    return
  }
  props.cfg.baudRate = Number(v)
}
function onBaudInput(e: Event) {
  const raw = (e.target as HTMLInputElement).value.trim()
  baudText.value = raw
  const n = Number(raw)
  props.cfg.baudRate = Number.isFinite(n) && n >= 1 ? n : 0
}

// 网络源展示名（同时用于日志模板中的 %p 占位与标签页名）
function composeNetName(): string | null {
  const host = (props.cfg.tcpHost ?? '').trim()
  if (transport.value === 'tcp-client') {
    const port = Number(props.cfg.tcpPort)
    if (!host || !port || port <= 0) return null
    return `${host}:${port}`
  }
  const port = transport.value === 'udp' ? Number(props.cfg.udpLocalPort) : Number(props.cfg.tcpPort)
  if (!port || port <= 0) return null
  const bh = host || '0.0.0.0'
  return `${transport.value === 'udp' ? 'udp' : 'tcp'}@${bh}:${port}`
}

async function connect() {
  err.value = ''
  if (transport.value !== 'serial') {
    const name = composeNetName()
    if (!name) {
      err.value = transport.value === 'tcp-client' ? t('conn.errHostPort') : t('conn.errPort')
      return
    }
    props.cfg.name = name
    props.cfg.transport = transport.value
  } else {
    props.cfg.name = props.cfg.name.trim()
    if (!props.cfg.name) {
      err.value = t('conn.errSelectPort')
      return
    }
    // 自定义波特率：连接前钳成整数回写，非法值在前端拦下（后端 u32，TabBar/状态栏直接读该数字）
    const baud = Number(props.cfg.baudRate)
    if (!Number.isInteger(baud) || baud < 1 || baud > 12_000_000) {
      err.value = t('conn.errBaud')
      return
    }
    props.cfg.baudRate = baud
    props.cfg.transport = null
  }
  busy.value = true
  try {
    await store.openTab({ ...props.cfg })
    emit('connected')
    close()
  } catch (e: unknown) {
    // 弹层内的富错误卡（分类+建议+刷新/重试）就是完整反馈，不再叠一条更薄的 toast
    err.value = String(e instanceof Error ? e.message : e)
  } finally {
    busy.value = false
  }
}

// 从预设回填：整份配置覆盖表单（含数据源），连接仍需手动点
function applyPreset(id: string) {
  const p = store.presets.find((x) => x.id === id)
  if (!p) return
  Object.assign(props.cfg, { ...p.config })
}
</script>

<template>
  <button class="btn btn-primary nc-trigger" type="button" :disabled="busy" @click="toggle">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>
    <span>{{ busy ? t('conn.connecting') : t('conn.newConnection') }}</span>
  </button>

  <div v-if="open" class="portbar-pop left nc-pop" @click.stop>
    <div class="portbar-pop-head">
      <span>{{ t('conn.newConnection') }}</span>
      <button class="btn btn-ghost btn-icon btn-sm" type="button" :title="t('app.common.close')" :aria-label="t('app.common.close')" @click="close">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
      </button>
    </div>

    <div class="field">
      <span class="field-label">{{ t('conn.dataSource') }}</span>
      <div class="seg" role="group" :aria-label="t('conn.dataSourceType')">
        <button class="seg-item" :class="{ active: transport === 'serial' }" @click="transport = 'serial'">{{ t('conn.srcSerial') }}</button>
        <button class="seg-item" :class="{ active: transport === 'tcp-client' }" @click="transport = 'tcp-client'">{{ t('conn.srcTcpClient') }}</button>
        <button class="seg-item" :class="{ active: transport === 'tcp-server' }" @click="transport = 'tcp-server'">{{ t('conn.srcTcpServer') }}</button>
        <button class="seg-item" :class="{ active: transport === 'udp' }" @click="transport = 'udp'">{{ t('conn.srcUdp') }}</button>
      </div>
    </div>

    <template v-if="transport === 'serial'">
      <div class="field">
        <span class="field-label">{{ t('conn.port') }}</span>
        <div class="nc-port-row">
          <select class="select" v-model="cfg.name" :title="t('conn.selectSerialTitle')">
            <option value="" disabled>{{ t('conn.selectPort') }}</option>
            <option v-for="p in store.ports" :key="p.name" :value="p.name">
              {{ p.name }}{{ p.product ? ' · ' + p.product : '' }}
            </option>
          </select>
          <button
            class="btn btn-ghost btn-icon"
            type="button"
            :title="t('conn.refreshPortsTitle')"
            :aria-label="t('conn.refreshPortsTitle')"
            @click="store.refreshPorts()"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v5h-5"/></svg>
          </button>
        </div>
      </div>

      <details class="nc-advanced">
        <summary>
          <svg class="nc-chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
          {{ t('conn.advanced') }} <span>{{ t('conn.advancedSub') }}</span>
        </summary>
      <div class="row3">
      <div class="field">
        <span class="field-label">{{ t('conn.baud') }}</span>
        <select
          v-if="!baudCustom"
          class="select"
          :value="cfg.baudRate"
          :title="t('conn.baudTitle')"
          @change="onBaudSelect"
        >
          <option v-if="showCustomBaudOption" :value="cfg.baudRate">{{ t('conn.baudCustomOption', { baud: cfg.baudRate }) }}</option>
          <option v-for="b in baudPresets" :key="b" :value="b">{{ b }}</option>
          <option value="custom">{{ t('conn.customValue') }}</option>
        </select>
        <div v-else class="baud-row">
          <input
            class="input input-mono"
            type="text"
            inputmode="numeric"
            :value="baudText"
            :title="t('conn.baudCustomTitle')"
            :placeholder="t('conn.baudPlaceholder')"
            spellcheck="false"
            @input="onBaudInput"
          />
          <button
            class="btn btn-ghost btn-icon"
            type="button"
            :title="t('conn.baudBackTitle')"
            :aria-label="t('conn.baudBackTitle')"
            @click="baudCustom = false"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
          </button>
        </div>
      </div>
        <div class="field">
          <span class="field-label">{{ t('conn.dataBits') }}</span>
          <select class="select" v-model="cfg.dataBits" :title="t('conn.dataBits')">
            <option :value="8">8</option><option :value="7">7</option>
            <option :value="6">6</option><option :value="5">5</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">{{ t('conn.parity') }}</span>
          <select class="select" v-model="cfg.parity" :title="t('conn.parityTitle')">
            <option value="none">N</option><option value="odd">O</option><option value="even">E</option>
          </select>
        </div>
      </div>
      <div class="row3">
        <div class="field">
          <span class="field-label">{{ t('conn.stopBits') }}</span>
          <select class="select" v-model="cfg.stopBits" :title="t('conn.stopBits')">
            <option value="1">1</option><option value="2">2</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">{{ t('conn.flowControl') }}</span>
          <select class="select" v-model="cfg.flowControl" :title="t('conn.flowControl')">
            <option value="none">None</option><option value="software">Xon/Xoff</option><option value="hardware">RTS/CTS</option>
          </select>
        </div>
        <div class="field">
          <span class="field-label">{{ t('conn.fromPreset') }}</span>
          <select class="select" :title="t('conn.fromPresetTitle')" @change="applyPreset(($event.target as HTMLSelectElement).value); ($event.target as HTMLSelectElement).value = ''">
            <option value="">{{ t('conn.noPreset') }}</option>
            <option v-for="p in store.presets" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
        </div>
      </div>
      </details>
    </template>

    <template v-else>
      <div class="field">
        <span class="field-label">{{ transport === 'tcp-client' ? t('conn.host') : t('conn.listenAddr') }}</span>
        <input
          class="input input-mono"
          v-model="cfg.tcpHost"
          :placeholder="transport === 'tcp-client' ? t('conn.hostPlaceholder') : '0.0.0.0'"
          spellcheck="false"
        />
      </div>
      <div class="field">
        <span class="field-label">{{ transport === 'udp' ? t('conn.localPort') : t('conn.port') }}</span>
        <input
          class="input input-mono"
          type="number"
          min="1"
          max="65535"
          :value="transport === 'udp' ? cfg.udpLocalPort : cfg.tcpPort"
          @input="
            transport === 'udp'
              ? (cfg.udpLocalPort = Number(($event.target as HTMLInputElement).value) || null)
              : (cfg.tcpPort = Number(($event.target as HTMLInputElement).value) || null)
          "
          :placeholder="t('conn.portPlaceholder')"
        />
      </div>
    </template>

    <div v-if="err" class="nc-error">
      <div class="nc-error-title">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 8v4M12 16h.01"/></svg>
        <b>{{ errorHint?.title }}</b>
      </div>
      <div class="nc-error-detail">{{ err }}</div>
      <div class="nc-error-action">{{ errorHint?.action }}</div>
      <div class="nc-error-buttons">
        <button v-if="transport === 'serial'" class="btn btn-ghost btn-sm" type="button" @click="store.refreshPorts()">{{ t('conn.refreshPorts') }}</button>
        <button class="btn btn-primary btn-sm" type="button" @click="connect">{{ t('conn.retry') }}</button>
      </div>
    </div>

    <div class="pop-foot">
      <button class="btn" type="button" @click="close">{{ t('conn.cancel') }}</button>
      <button class="btn btn-primary" type="button" :disabled="busy" @click="connect">
        {{ busy ? t('conn.connecting') : t('conn.connect') }}
      </button>
    </div>
  </div>
</template>
