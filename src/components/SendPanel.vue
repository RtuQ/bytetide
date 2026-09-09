<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useSessionStore } from '../stores/session'
import type { SendPreset, SendSequence, SeqStep } from '../types'
import { checksumResults, type ChecksumRow } from '../composables/useChecksum'

const store = useSessionStore()
const active = computed(() => store.active)
const text = ref('')
const mode = ref<'ascii' | 'hex'>('ascii')
const appendNewline = ref(true)
const busy = ref(false)

/** 功能页签：单发=原发送框；快捷帧/序列/校验为发送区升级新增 */
const tab = ref<'single' | 'quick' | 'seq' | 'ck'>('single')

async function send() {
  const s = active.value
  if (!s || busy.value) return
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
  if (!s || s.status !== 'connected') return
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
  const name = prompt('快捷帧名称', '快捷帧')
  if (!name) return
  store.saveSendPreset({ name, payload, mode: mode.value })
}
function sendPreset(p: SendPreset) {
  const s = active.value
  if (!s || s.status !== 'connected') return
  store.send(s.id, p.payload, p.mode).catch((e: unknown) => alert(String(e)))
}
function renamePreset(p: SendPreset) {
  const name = prompt('快捷帧名称', p.name)
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
    name: `序列 ${seqLocal}`,
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
  const name = prompt('快捷帧名称', '带校验帧')
  if (!name) return
  store.saveSendPreset({ name, payload: row.hexBytes, mode: 'hex' })
}
</script>

<template>
  <details class="sendpanel" v-if="active">
    <summary class="send-toggle">
      <svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
      <svg class="send-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
      <span>发送</span>
      <span class="send-mode">{{ mode === 'hex' ? 'HEX' : 'ASCII' }}</span>
    </summary>

    <div class="send-body">
      <div class="send-tabs-row">
        <div class="seg" role="group" aria-label="发送功能区">
          <button class="seg-item" :class="{ active: tab === 'single' }" @click="tab = 'single'">单发</button>
          <button class="seg-item" :class="{ active: tab === 'quick' }" @click="tab = 'quick'">
            快捷帧<span v-if="store.sendPresets.length" class="tab-n">{{ store.sendPresets.length }}</span>
          </button>
          <button class="seg-item" :class="{ active: tab === 'seq' }" @click="tab = 'seq'">
            序列<span v-if="store.sendSequences.length" class="tab-n">{{ store.sendSequences.length }}</span>
          </button>
          <button class="seg-item" :class="{ active: tab === 'ck' }" @click="tab = 'ck'">校验</button>
        </div>
        <span class="send-spacer"></span>
        <span class="send-pins">
          <span class="pin-label">信号线</span>
          <button
            class="btn btn-ghost btn-sm"
            :class="{ 'is-on': dtrOn }"
            :disabled="!canSignal"
            :title="canSignal ? 'DTR（数据终端就绪）电平切换' : '仅串口源已连接时可用'"
            aria-label="切换 DTR 电平"
            @click="togglePin('dtr')"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/></svg>
            <span>DTR</span>
          </button>
          <button
            class="btn btn-ghost btn-sm"
            :class="{ 'is-on': rtsOn }"
            :disabled="!canSignal"
            :title="canSignal ? 'RTS（请求发送）电平切换' : '仅串口源已连接时可用'"
            aria-label="切换 RTS 电平"
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
          <div class="seg" role="group" aria-label="发送模式">
            <button class="seg-item" :class="{ active: mode === 'ascii' }" @click="mode = 'ascii'">ASCII</button>
            <button class="seg-item" :class="{ active: mode === 'hex' }" @click="mode = 'hex'">HEX</button>
          </div>
          <span class="sep"></span>
          <label v-if="mode === 'ascii'" class="check">
            <input type="checkbox" v-model="appendNewline" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>追加换行</span>
          </label>
        </div>

        <textarea
          class="send-text input-mono"
          v-model="text"
          :placeholder="mode === 'hex' ? 'Hex，如 41 42 43' : '发送内容（Ctrl+Enter 发送）'"
          @keydown.ctrl.enter.prevent="send"
        />

        <div class="send-foot">
          <label class="check">
            <input type="checkbox" v-model="autoEnabled" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>定时</span>
          </label>
          <input class="auto-int" type="number" v-model.number="autoIntervalMs" min="50" step="100" />
          <span class="small muted">ms</span>
          <span class="send-spacer"></span>
          <button class="btn btn-primary" :disabled="busy || active.status !== 'connected'" @click="send">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m22 2-7 20-4-9-9-4Z"/><path d="M22 2 11 13"/></svg>
            <span>发送</span>
          </button>
        </div>

        <details class="hist">
          <summary>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
            发送历史 ({{ active.sendHistory.length }})
          </summary>
          <div v-for="(h, i) in active.sendHistory" :key="i" class="hist-item" @click="pick(h)">
            {{ h }}
          </div>
        </details>
      </div>

      <!-- 快捷帧 -->
      <div v-show="tab === 'quick'" class="tabpane">
        <div class="qp-toolbar">
          <button class="btn btn-sm" :disabled="!text.trim()" title="把单发输入框当前内容存为快捷帧" @click="saveCurrentAsPreset">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"/><path d="M17 21v-8H7v8M7 3v5h8"/></svg>
            <span>存当前文本</span>
          </button>
          <span class="send-spacer"></span>
          <label class="check">
            <input type="checkbox" v-model="qpManage" />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>管理</span>
          </label>
        </div>
        <div v-if="store.sendPresets.length" class="qp-grid">
          <div
            v-for="p in store.sendPresets"
            :key="p.id"
            class="qp-chip"
            role="button"
            tabindex="0"
            :title="qpManage ? '管理模式：用右侧按钮编辑/删除' : '点击发送'"
            @click="sendPreset(p)"
            @keydown.enter.prevent="sendPreset(p)"
          >
            <span class="qp-name">{{ p.name }}<span class="tag" :class="p.mode">{{ p.mode.toUpperCase() }}</span></span>
            <span class="qp-payload">{{ p.payload }}</span>
            <span v-if="qpManage" class="qp-acts">
              <button class="btn btn-icon btn-sm" :title="`重命名 ${p.name}`" :aria-label="`重命名 ${p.name}`" @click.stop="renamePreset(p)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>
              </button>
              <button class="btn btn-icon btn-sm" :title="`删除 ${p.name}`" :aria-label="`删除 ${p.name}`" @click.stop="store.removeSendPreset(p.id)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
              </button>
            </span>
          </div>
        </div>
        <div v-else class="qp-empty">还没有快捷帧——在「单发」输入内容后点上方「存当前文本」</div>
      </div>

      <!-- 序列 -->
      <div v-show="tab === 'seq'" class="tabpane">
        <div class="seq-top">
          <div class="seq-selcol">
            <span class="field-label">序列</span>
            <select class="select" :value="curSeqId" aria-label="选择序列" @change="curSeqId = ($event.target as HTMLSelectElement).value">
              <option v-for="s in store.sendSequences" :key="s.id" :value="s.id">{{ s.name }}（{{ s.steps.length }} 步）</option>
            </select>
          </div>
          <button class="btn btn-sm seq-mt" @click="newSeq">＋新建</button>
          <button class="btn btn-sm seq-mt" :disabled="!draft" @click="delSeq">删除</button>
          <span class="send-spacer"></span>
          <template v-if="draft">
            <label class="check seq-mt">
              <input type="checkbox" v-model="draft.loop" @change="saveDraft" />
              <span class="box">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
              </span>
              <span>循环</span>
            </label>
            <span class="seq-mt seq-int">
              <input class="auto-int" type="number" v-model.number="draft.intervalMs" min="50" step="100" title="循环轮间隔" aria-label="循环轮间隔毫秒" @change="saveDraft" />
              <span class="small muted">ms</span>
            </span>
            <button class="btn btn-primary seq-mt" :disabled="!canRun || !!store.seqRun" title="按步骤顺序执行（发送/延时/信号）" @click="runSeq">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="6 3 20 12 6 21 6 3"/></svg>
              <span>运行</span>
            </button>
            <button class="btn seq-mt" :disabled="!runningHere" title="中止当前序列" @click="store.stopSequence()">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="5" width="14" height="14" rx="2"/></svg>
              <span>停止</span>
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
              <span>{{ st.kind === 'send' ? '发送' : st.kind === 'delay' ? '延时' : '信号' }}</span>
            </span>
            <template v-if="st.kind === 'send'">
              <input class="input input-mono seq-payload" v-model="st.payload" placeholder="发送内容" @change="saveDraft" />
              <select class="select seq-sel" v-model="st.mode" aria-label="发送模式" @change="saveDraft">
                <option value="ascii">ASCII</option>
                <option value="hex">HEX</option>
              </select>
              <label class="check">
                <input type="checkbox" v-model="st.appendNewline" @change="saveDraft" />
                <span class="box">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                </span>
                <span>换行</span>
              </label>
            </template>
            <template v-else-if="st.kind === 'delay'">
              <input class="input seq-ms" type="number" v-model.number="st.ms" min="10" step="50" aria-label="延时毫秒" @change="saveDraft" />
              <span class="small muted">ms</span>
            </template>
            <template v-else>
              <select class="select seq-sel" v-model="st.pin" aria-label="信号线" @change="saveDraft">
                <option value="dtr">DTR</option>
                <option value="rts">RTS</option>
              </select>
              <select class="select seq-sel" v-model="st.level" aria-label="电平" @change="saveDraft">
                <option :value="true">拉高</option>
                <option :value="false">拉低</option>
              </select>
            </template>
            <span class="seq-acts">
              <button class="btn btn-ghost btn-sm" title="上移" aria-label="上移该步骤" :disabled="i === 0" @click="moveStep(i, -1)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m18 15-6-6-6 6"/></svg>
              </button>
              <button class="btn btn-ghost btn-sm" title="下移" aria-label="下移该步骤" :disabled="i === draft.steps.length - 1" @click="moveStep(i, 1)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
              </button>
              <button class="btn btn-ghost btn-sm" title="删除该步骤" aria-label="删除该步骤" @click="delStep(i)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
              </button>
            </span>
          </div>
          <div class="seq-addrow">
            <button class="btn btn-ghost btn-sm" @click="addStep('send')">＋发送</button>
            <button class="btn btn-ghost btn-sm" @click="addStep('delay')">＋延时</button>
            <button class="btn btn-ghost btn-sm" @click="addStep('signal')">＋信号</button>
            <span class="send-spacer"></span>
            <span class="small muted">信号步骤 = DTR/RTS 拉高/拉低（bootloader 复位进 ISP 用）</span>
          </div>
        </div>
        <div v-else class="qp-empty">还没有序列——点上方「＋新建」创建</div>

        <div class="seq-runline">
          <template v-if="store.seqRun && runningHere">
            <span class="prog">运行中</span> · 轮次 {{ store.seqRun.round }} · 步骤 {{ store.seqRun.step + 1 }}/{{ draft?.steps.length ?? 0 }}
          </template>
          <template v-else-if="store.seqRun">其他会话正在运行序列</template>
          <template v-else-if="seqErr"><span class="seq-err">{{ seqErr }}</span></template>
          <template v-else>就绪</template>
        </div>
      </div>

      <!-- 校验 -->
      <div v-show="tab === 'ck'" class="tabpane">
        <div class="ck-in">
          <textarea
            class="textarea input-mono ck-text"
            v-model="ckInput"
            :placeholder="ckMode === 'hex' ? '输入 HEX，如 01 03 00 00 00 02' : '输入 ASCII 文本'"
            rows="2"
          ></textarea>
          <div class="ck-opts">
            <div class="seg" role="group" aria-label="校验输入模式">
              <button class="seg-item" :class="{ active: ckMode === 'hex' }" @click="ckMode = 'hex'">HEX</button>
              <button class="seg-item" :class="{ active: ckMode === 'ascii' }" @click="ckMode = 'ascii'">ASCII</button>
            </div>
            <select class="select ck-endian" v-model="ckEndian" title="多字节校验的字节序" aria-label="校验字节序">
              <option value="be">大端 AB</option>
              <option value="le">小端 BA</option>
            </select>
          </div>
        </div>
        <div v-if="ckRows.length" class="ck-table" :class="{ 'ck-flash': ckFlash }" @animationend="ckFlash = false">
          <div v-for="row in ckRows" :key="row.algo" class="ck-row">
            <span class="algo">{{ row.algo }}</span>
            <span class="val">{{ row.hexBytes }}</span>
            <span class="small muted">{{ row.bytes }} 字节</span>
            <button class="btn btn-sm" :title="`把 ${row.algo} 校验字节追加到单发输入框`" @click="appendCk(row)">＋追加</button>
          </div>
        </div>
        <div v-else class="qp-empty">输入为空——输入 HEX 或 ASCII 后实时计算</div>
        <div class="send-foot">
          <span class="small muted">「＋追加」把校验字节拼到「单发」输入框尾部</span>
          <span class="send-spacer"></span>
          <button class="btn btn-sm" :disabled="!ckRows.length" title="把 modbus 校验结果存为快捷帧" @click="saveCkPreset">存为快捷帧</button>
        </div>
      </div>
    </div>
  </details>
  <div v-else class="muted panel-empty">无活动会话</div>
</template>
