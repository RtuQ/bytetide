<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSessionStore } from '../stores/session'
import type { SendPreset, SendSequence, SeqStep } from '../types'
import { checksumResults, type ChecksumRow } from '../composables/useChecksum'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'

const store = useSessionStore()
const active = computed(() => store.active)
const text = ref('')
const mode = ref<'ascii' | 'hex'>('ascii')
const appendNewline = ref(true)
const busy = ref(false)

/** 发送能力统一守卫：仅 live 会话且已连接（评审 P2-2——Replay/Offline 会话
 *  的状态可能是 connected，但后端拒发，UI 不应让用户执行必然失败的操作） */
const canSend = computed(
  () => active.value?.kind === 'live' && active.value.status === 'connected',
)
const cannotSendReasonKey = computed<MessageKey>(() =>
  active.value?.kind === 'live' ? 'send.single.notConnected' : 'send.single.readOnly',
)

/** 功能页签：单发=原发送框；快捷帧/序列/校验为发送区升级新增 */
const tab = ref<'single' | 'quick' | 'seq' | 'ck'>('single')

async function send() {
  const s = active.value
  if (!s || !canSend.value || busy.value) return
  let payload = text.value
  if (mode.value === 'ascii' && appendNewline.value) payload += '\n'
  busy.value = true
  try {
    await store.send(s.id, payload, mode.value)
    text.value = ''
  } catch (e: unknown) {
    alert(String(e instanceof Error ? e.message : e))
  } finally {
    busy.value = false
  }
}

// 定时自动发送：按间隔重复发送当前文本到当前会话（断开/切换/卸载时停止）
const autoEnabled = ref(false)
const autoIntervalMs = ref(1000)
let timer: number | null = null

function stopAuto() {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
}
function startAuto() {
  stopAuto()
  if (!autoEnabled.value) return
  timer = window.setInterval(tickAuto, Math.max(50, autoIntervalMs.value))
}
async function tickAuto() {
  const s = active.value
  if (!s || !canSend.value) return
  let p = text.value
  if (mode.value === 'ascii' && appendNewline.value) p += '\n'
  try {
    await store.send(s.id, p, mode.value)
  } catch {
    /* 忽略单次失败 */
  }
}
watch([autoEnabled, autoIntervalMs, () => active.value?.id], () => {
  if (autoEnabled.value) startAuto()
  else stopAuto()
})
// 会话切到不可发送状态（断开/切到 replay 等）自动关停定时发送——
// 防止对回放会话以最低 50ms 间隔持续空转调用后端、静默吞错
watch(canSend, (v) => {
  if (!v) {
    autoEnabled.value = false
    stopAuto()
  }
})
onBeforeUnmount(stopAuto)

function pick(h: string) {
  text.value = h
}

// ---------- DTR/RTS 信号线 ----------
const dtrOn = ref(false)
const rtsOn = ref(false)
const canSignal = computed(() => {
  const s = active.value
  if (!s || s.kind !== 'live' || s.status !== 'connected') return false
  const t = s.config.transport
  return !t || t === 'serial'
})
// 切会话/断开时引脚实际电平未知，重置显示（后端不回读引脚状态）
watch([() => active.value?.id, () => active.value?.status], () => {
  dtrOn.value = false
  rtsOn.value = false
})
async function togglePin(pin: 'dtr' | 'rts') {
  const s = active.value
  if (!s || !canSignal.value) return
  const cur = pin === 'dtr' ? dtrOn : rtsOn
  const next = !cur.value
  try {
    await store.setSignal(s.id, pin, next)
    cur.value = next
  } catch (e: unknown) {
    alert(String(e instanceof Error ? e.message : e))
  }
}

// ---------- 快捷帧 ----------
const qpManage = ref(false)
function saveCurrentAsPreset() {
  const payload = text.value.trim()
  if (!payload) return
  const name = prompt(t('send.quick.namePrompt'), t('send.quick.nameDefault'))
  if (!name) return
  store.saveSendPreset({ name, payload, mode: mode.value })
}
function sendPreset(p: SendPreset) {
  const s = active.value
  if (!s || !canSend.value) return
  store.send(s.id, p.payload, p.mode).catch((e: unknown) => alert(String(e)))
}
function renamePreset(p: SendPreset) {
  const name = prompt(t('send.quick.namePrompt'), p.name)
  if (!name || !name.trim()) return
  store.saveSendPreset({ id: p.id, name, payload: p.payload, mode: p.mode })
}

// ---------- 序列 ----------
const curSeqId = ref('')
const draft = ref<SendSequence | null>(null)
let seqLocal = 0
function loadDraft() {
  const s = store.sendSequences.find((x) => x.id === curSeqId.value)
  draft.value = s ? (JSON.parse(JSON.stringify(s)) as SendSequence) : null
}
watch(curSeqId, loadDraft)
watch(
  () => store.sendSequences.map((x) => x.id).join(','),
  () => {
    // 当前选择被删除时回落到第一个
    if (!store.sendSequences.some((x) => x.id === curSeqId.value)) {
      curSeqId.value = store.sendSequences[0]?.id ?? ''
    } else {
      loadDraft()
    }
  },
)
if (store.sendSequences.length) curSeqId.value = store.sendSequences[0].id
loadDraft()

function saveDraft() {
  if (draft.value) store.saveSendSequence(draft.value)
}
function newSeq() {
  seqLocal += 1
  const seq: SendSequence = {
    id: `sq${Date.now().toString(36)}${seqLocal}`,
    name: t('send.seq.defaultName', { seq: seqLocal }),
    steps: [{ kind: 'send', payload: '', mode: 'ascii', appendNewline: true }],
    loop: false,
    intervalMs: 1000,
  }
  store.saveSendSequence(seq)
  curSeqId.value = seq.id
}
function delSeq() {
  if (draft.value) store.removeSendSequence(draft.value.id)
}
function addStep(kind: SeqStep['kind']) {
  if (!draft.value) return
  if (kind === 'send')
    draft.value.steps.push({ kind: 'send', payload: '', mode: 'ascii', appendNewline: true })
  else if (kind === 'delay') draft.value.steps.push({ kind: 'delay', ms: 500 })
  else draft.value.steps.push({ kind: 'signal', pin: 'dtr', level: true })
  saveDraft()
}
function moveStep(i: number, dir: -1 | 1) {
  if (!draft.value) return
  const j = i + dir
  const steps = draft.value.steps
  if (j < 0 || j >= steps.length) return
  ;[steps[i], steps[j]] = [steps[j], steps[i]]
  saveDraft()
}
function delStep(i: number) {
  if (!draft.value) return
  draft.value.steps.splice(i, 1)
  saveDraft()
}

const runningHere = computed(
  () => store.seqRun?.sessionId === active.value?.id && store.seqRun?.seqId === curSeqId.value,
)
/** 步骤完成 = 运行游标已越过该步（循环回卷时 step 重小，勾自动清） */
function stepDone(i: number) {
  return runningHere.value && !!store.seqRun && store.seqRun.step > i
}
const canRun = computed(() => {
  const s = active.value
  return !!s && s.kind === 'live' && s.status === 'connected'
})
const seqErr = ref('')
async function runSeq() {
  const s = active.value
  if (!s || !curSeqId.value || store.seqRun) return
  seqErr.value = ''
  try {
    await store.runSequence(s.id, curSeqId.value)
  } catch (e: unknown) {
    seqErr.value = String(e instanceof Error ? e.message : e)
  }
}

// ---------- 校验计算器 ----------
const ckInput = ref('01 03 00 00 00 02')
const ckMode = ref<'hex' | 'ascii'>('hex')
const ckEndian = ref<'be' | 'le'>('be')
const ckRows = computed<ChecksumRow[]>(() =>
  checksumResults(ckInput.value, ckMode.value, ckEndian.value),
)
// 结果变化时值闪一下 accent：先摘类再下一帧挂回，保证连续输入时动画重放
const ckFlash = ref(false)
watch(ckRows, () => {
  if (!ckRows.value.length) return
  ckFlash.value = false
  requestAnimationFrame(() => {
    ckFlash.value = true
  })
})
function appendCk(row: ChecksumRow) {
  const cur = text.value.trim()
  text.value = cur ? `${cur} ${row.hexBytes}` : row.hexBytes
  mode.value = 'hex'
  tab.value = 'single'
}
function saveCkPreset() {
  const rows = ckRows.value
  if (!rows.length) return
  const row = rows.find((r) => r.algo === 'crc16-modbus') ?? rows[0]
  const name = prompt(t('send.quick.namePrompt'), t('send.crc.presetDefault'))
  if (!name) return
  store.saveSendPreset({ name, payload: row.hexBytes, mode: 'hex' })
}
</script>

<template>
  <details class="sendpanel" v-if="active">
    <summary class="send-toggle">
      <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
      <svg class="send-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
      <span>{{ t('send.title') }}</span>
      <span class="send-mode">{{ mode === 'hex' ? 'HEX' : 'ASCII' }}</span>
    </summary>

    <div class="send-body">
      <div class="send-tabs-row">
        <div class="seg" role="group" :aria-label="t('send.tab.group')">
          <button class="seg-item" :class="{ active: tab === 'single' }" @click="tab = 'single'">{{ t('send.tab.single') }}</button>
          <button class="seg-item" :class="{ active: tab === 'quick' }" @click="tab = 'quick'">
            {{ t('send.tab.quick') }}<span v-if="store.sendPresets.length" class="tab-n">{{ store.sendPresets.length }}</span>
          </button>
          <button class="seg-item" :class="{ active: tab === 'seq' }" @click="tab = 'seq'">
            {{ t('send.tab.seq') }}<span v-if="store.sendSequences.length" class="tab-n">{{ store.sendSequences.length }}</span>
          </button>
          <button class="seg-item" :class="{ active: tab === 'ck' }" @click="tab = 'ck'">{{ t('send.tab.ck') }}</button>
        </div>
        <span class="send-spacer"></span>
        <span class="send-pins">
          <span class="pin-label">{{ t('send.pin.label') }}</span>
          <button
            class="btn btn-ghost btn-sm"
            :class="{ 'is-on': dtrOn }"
            :disabled="!canSignal"
            :title="canSignal ? t('send.pin.dtrTitle') : t('send.pin.serialOnly')"
            :aria-label="t('send.pin.dtrAria')"
            @click="togglePin('dtr')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/></svg>
            <span>DTR</span>
          </button>
          <button
            class="btn btn-ghost btn-sm"
            :class="{ 'is-on': rtsOn }"
            :disabled="!canSignal"
            :title="canSignal ? t('send.pin.rtsTitle') : t('send.pin.serialOnly')"
            :aria-label="t('send.pin.rtsAria')"
            @click="togglePin('rts')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/></svg>
            <span>RTS</span>
          </button>
        </span>
      </div>

      <!-- 单发（原有功能原样保留） -->
      <div v-show="tab === 'single'" class="tabpane">
        <div class="send-head">
          <div class="seg" role="group" :aria-label="t('send.mode.group')">
            <button class="seg-item" :class="{ active: mode === 'ascii' }" @click="mode = 'ascii'">ASCII</button>
            <button class="seg-item" :class="{ active: mode === 'hex' }" @click="mode = 'hex'">HEX</button>
          </div>
          <span class="sep"></span>
          <label v-if="mode === 'ascii'" class="check">
            <input type="checkbox" v-model="appendNewline" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('send.single.appendNewline') }}</span>
          </label>
        </div>

        <textarea
          class="send-text input-mono"
          v-model="text"
          :placeholder="mode === 'hex' ? t('send.single.hexPlaceholder') : t('send.single.textPlaceholder')"
          @keydown.ctrl.enter.prevent="send"
        />

        <div class="send-foot">
          <label class="check">
            <input type="checkbox" v-model="autoEnabled" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('send.single.timed') }}</span>
          </label>
          <input class="auto-int" type="number" v-model.number="autoIntervalMs" min="50" step="100" />
          <span class="small muted">ms</span>
          <span class="send-spacer"></span>
          <button
            class="btn btn-primary"
            :disabled="busy || !canSend"
            :title="canSend ? t('send.single.send') : t('send.single.cannotSend', { reason: t(cannotSendReasonKey) })"
            @click="send"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
            <span>{{ t('send.single.send') }}</span>
          </button>
        </div>

        <details class="hist">
          <summary>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
            {{ t('send.single.history', { count: active.sendHistory.length }) }}
          </summary>
          <div v-for="(h, i) in active.sendHistory" :key="i" class="hist-item" @click="pick(h)">
            {{ h }}
          </div>
        </details>
      </div>

      <!-- 快捷帧 -->
      <div v-show="tab === 'quick'" class="tabpane">
        <div class="qp-toolbar">
          <button class="btn btn-sm" :disabled="!text.trim()" :title="t('send.quick.saveTitle')" @click="saveCurrentAsPreset">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><path d="M17 21v-8H7v8M7 3v5h8"/></svg>
            <span>{{ t('send.quick.save') }}</span>
          </button>
          <span class="send-spacer"></span>
          <label class="check">
            <input type="checkbox" v-model="qpManage" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>{{ t('send.quick.manage') }}</span>
          </label>
        </div>
        <div v-if="store.sendPresets.length" class="qp-grid">
          <div
            v-for="p in store.sendPresets"
            :key="p.id"
            class="qp-chip"
            role="button"
            tabindex="0"
            :title="qpManage ? t('send.quick.manageHint') : t('send.quick.clickToSend')"
            @click="sendPreset(p)"
            @keydown.enter.prevent="sendPreset(p)"
          >
            <span class="qp-name">{{ p.name }}<span class="tag" :class="p.mode">{{ p.mode.toUpperCase() }}</span></span>
            <span class="qp-payload">{{ p.payload }}</span>
            <span v-if="qpManage" class="qp-acts">
              <button class="btn btn-icon btn-sm" :title="t('send.quick.rename', { name: p.name })" :aria-label="t('send.quick.rename', { name: p.name })" @click.stop="renamePreset(p)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>
              </button>
              <button class="btn btn-icon btn-sm" :title="t('send.quick.delete', { name: p.name })" :aria-label="t('send.quick.delete', { name: p.name })" @click.stop="store.removeSendPreset(p.id)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
              </button>
            </span>
          </div>
        </div>
        <div v-else class="qp-empty">{{ t('send.quick.empty') }}</div>
      </div>

      <!-- 序列 -->
      <div v-show="tab === 'seq'" class="tabpane">
        <div class="seq-top">
          <div class="seq-selcol">
            <span class="field-label">{{ t('send.seq.label') }}</span>
            <select class="select" :value="curSeqId" :aria-label="t('send.seq.selectAria')" @change="curSeqId = ($event.target as HTMLSelectElement).value">
              <option v-for="s in store.sendSequences" :key="s.id" :value="s.id">{{ t('send.seq.option', { name: s.name, count: s.steps.length }) }}</option>
            </select>
          </div>
          <button class="btn btn-sm seq-mt" @click="newSeq">{{ t('send.seq.new') }}</button>
          <button class="btn btn-sm seq-mt" :disabled="!draft" @click="delSeq">{{ t('send.seq.delete') }}</button>
          <span class="send-spacer"></span>
          <template v-if="draft">
            <label class="check seq-mt">
              <input type="checkbox" v-model="draft.loop" @change="saveDraft" />
              <span class="box">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
              </span>
              <span>{{ t('send.seq.loop') }}</span>
            </label>
            <span class="seq-mt seq-int">
              <input class="auto-int" type="number" v-model.number="draft.intervalMs" min="50" step="100" :title="t('send.seq.intervalTitle')" :aria-label="t('send.seq.intervalAria')" @change="saveDraft" />
              <span class="small muted">ms</span>
            </span>
            <button class="btn btn-primary seq-mt" :disabled="!canRun || !!store.seqRun" :title="t('send.seq.runTitle')" @click="runSeq">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"/></svg>
              <span>{{ t('send.seq.run') }}</span>
            </button>
            <button class="btn seq-mt" :disabled="!runningHere" :title="t('send.seq.stopTitle')" @click="store.stopSequence()">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="14" height="14" rx="2"/></svg>
              <span>{{ t('send.seq.stop') }}</span>
            </button>
          </template>
        </div>

        <div v-if="draft" class="seq-steps">
          <div
            v-for="(st, i) in draft.steps"
            :key="i"
            class="seq-step"
            :class="{ running: runningHere && store.seqRun?.step === i, done: stepDone(i) }"
          >
            <span class="idx">
              <svg v-if="stepDone(i)" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
              <template v-else>{{ i + 1 }}</template>
            </span>
            <span class="kind" :class="`k-${st.kind}`">
              <svg v-if="st.kind === 'send'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
              <svg v-else-if="st.kind === 'delay'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
              <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/></svg>
              <span>{{ st.kind === 'send' ? t('send.seq.stepSend') : st.kind === 'delay' ? t('send.seq.stepDelay') : t('send.seq.stepSignal') }}</span>
            </span>
            <template v-if="st.kind === 'send'">
              <input class="input input-mono seq-payload" v-model="st.payload" :placeholder="t('send.seq.payloadPlaceholder')" @change="saveDraft" />
              <select class="select seq-sel" v-model="st.mode" :aria-label="t('send.mode.group')" @change="saveDraft">
                <option value="ascii">ASCII</option>
                <option value="hex">HEX</option>
              </select>
              <label class="check">
                <input type="checkbox" v-model="st.appendNewline" @change="saveDraft" />
                <span class="box">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                </span>
                <span>{{ t('send.seq.newline') }}</span>
              </label>
            </template>
            <template v-else-if="st.kind === 'delay'">
              <input class="input seq-ms" type="number" v-model.number="st.ms" min="10" step="50" :aria-label="t('send.seq.delayAria')" @change="saveDraft" />
              <span class="small muted">ms</span>
            </template>
            <template v-else>
              <select class="select seq-sel" v-model="st.pin" :aria-label="t('send.pin.label')" @change="saveDraft">
                <option value="dtr">DTR</option>
                <option value="rts">RTS</option>
              </select>
              <select class="select seq-sel" v-model="st.level" :aria-label="t('send.seq.levelAria')" @change="saveDraft">
                <option :value="true">{{ t('send.seq.levelHigh') }}</option>
                <option :value="false">{{ t('send.seq.levelLow') }}</option>
              </select>
            </template>
            <span class="seq-acts">
              <button class="btn btn-ghost btn-sm" :title="t('send.seq.moveUp')" :aria-label="t('send.seq.moveUpAria')" :disabled="i === 0" @click="moveStep(i, -1)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>
              </button>
              <button class="btn btn-ghost btn-sm" :title="t('send.seq.moveDown')" :aria-label="t('send.seq.moveDownAria')" :disabled="i === draft.steps.length - 1" @click="moveStep(i, 1)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              <button class="btn btn-ghost btn-sm" :title="t('send.seq.delStep')" :aria-label="t('send.seq.delStep')" @click="delStep(i)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
              </button>
            </span>
          </div>
          <div class="seq-addrow">
            <button class="btn btn-ghost btn-sm" @click="addStep('send')">{{ t('send.seq.addSend') }}</button>
            <button class="btn btn-ghost btn-sm" @click="addStep('delay')">{{ t('send.seq.addDelay') }}</button>
            <button class="btn btn-ghost btn-sm" @click="addStep('signal')">{{ t('send.seq.addSignal') }}</button>
            <span class="send-spacer"></span>
            <span class="small muted">{{ t('send.seq.signalHint') }}</span>
          </div>
        </div>
        <div v-else class="qp-empty">{{ t('send.seq.empty') }}</div>

        <div class="seq-runline">
          <template v-if="store.seqRun && runningHere">
            <span class="prog">{{ t('send.seq.running') }}</span> {{ t('send.seq.progress', { round: store.seqRun.round, step: store.seqRun.step + 1, total: draft?.steps.length ?? 0 }) }}
          </template>
          <template v-else-if="store.seqRun">{{ t('send.seq.runningElsewhere') }}</template>
          <template v-else-if="seqErr"><span class="seq-err">{{ seqErr }}</span></template>
          <template v-else>{{ t('send.seq.ready') }}</template>
        </div>
      </div>

      <!-- 校验 -->
      <div v-show="tab === 'ck'" class="tabpane">
        <div class="ck-in">
          <textarea
            class="textarea input-mono ck-text"
            v-model="ckInput"
            :placeholder="ckMode === 'hex' ? t('send.crc.hexPlaceholder') : t('send.crc.asciiPlaceholder')"
            rows="2"
          ></textarea>
          <div class="ck-opts">
            <div class="seg" role="group" :aria-label="t('send.crc.modeGroup')">
              <button class="seg-item" :class="{ active: ckMode === 'hex' }" @click="ckMode = 'hex'">HEX</button>
              <button class="seg-item" :class="{ active: ckMode === 'ascii' }" @click="ckMode = 'ascii'">ASCII</button>
            </div>
            <select class="select ck-endian" v-model="ckEndian" :title="t('send.crc.endianTitle')" :aria-label="t('send.crc.endianAria')">
              <option value="be">{{ t('send.crc.bigEndian') }}</option>
              <option value="le">{{ t('send.crc.littleEndian') }}</option>
            </select>
          </div>
        </div>
        <div v-if="ckRows.length" class="ck-table" :class="{ 'ck-flash': ckFlash }" @animationend="ckFlash = false">
          <div v-for="row in ckRows" :key="row.algo" class="ck-row">
            <span class="algo">{{ row.algo }}</span>
            <span class="val">{{ row.hexBytes }}</span>
            <span class="small muted">{{ t('send.crc.bytes', { count: row.bytes }) }}</span>
            <button class="btn btn-sm" :title="t('send.crc.appendTitle', { algo: row.algo })" @click="appendCk(row)">{{ t('send.crc.append') }}</button>
          </div>
        </div>
        <div v-else class="qp-empty">{{ t('send.crc.empty') }}</div>
        <div class="send-foot">
          <span class="small muted">{{ t('send.crc.appendHint') }}</span>
          <span class="send-spacer"></span>
          <button class="btn btn-sm" :disabled="!ckRows.length" :title="t('send.crc.saveTitle')" @click="saveCkPreset">{{ t('send.crc.save') }}</button>
        </div>
      </div>
    </div>
  </details>
  <div v-else class="muted panel-empty">{{ t('app.status.noSession') }}</div>
</template>
