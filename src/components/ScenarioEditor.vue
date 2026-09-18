<script setup lang="ts">
import { computed, ref } from 'vue'
import { useAutomationStore } from '../stores/automation'
import { t } from '../i18n'
import type { MessageKey } from '../i18n'
import {
  cloneScenario,
  makeDefaultStep,
  parseStepPath,
  MAX_NESTING_DEPTH,
  type LineMatcher,
  type Scenario,
  type ScenarioStep,
  type ScenarioStepKind,
} from '../types/automation'

/**
 * 场景编辑器（Stage 3 Task 4）：步骤树的增删改/上下移动/嵌套 Repeat（扁平行渲染 +
 * 视觉缩进，深度 ≤4，超限的「循环」按钮禁用并提示）；每步表单复用 .input/.select/
 * .seg/.check 控件；matcher 四选一（literal/regex/hex/mask seg 切换 + dir 任意/rx/tx）；
 * 变量表编辑；保存先经 store 的 scenarioValidate 预检，错误按 step path 定位到
 * 对应步骤行内展示。全部操作为按钮（type=button），键盘可达、无拖拽。
 */
const props = defineProps<{ entryId: string }>()
const emit = defineEmits<{ (e: 'close'): void }>()
const automation = useAutomationStore()

automation.clearValidation()

const entry = computed(() => automation.entries.find((e) => e.id === props.entryId) ?? null)
/** 本地草稿（深拷贝，保存时整包送预检+落库） */
const draft = ref<Scenario | null>(entry.value ? cloneScenario(entry.value.scenario) : null)

// code→词条映射：值存 MessageKey，使用点 t(KIND_LABELS[kind]) 求值（切语言即时刷新）
const KIND_LABELS: Record<ScenarioStepKind, MessageKey> = {
  send: 'scen.kindSend',
  delay: 'scen.kindDelay',
  signal: 'scen.kindSignal',
  wait: 'scen.kindWait',
  assert: 'scen.kindAssert',
  repeat: 'scen.kindRepeat',
}

// ===================== 扁平行模型（depth = Repeat 包裹层数） =====================

interface StepRow {
  type: 'step'
  chain: number[]
  container: number[]
  /** 同级内的下标（上移按钮在首位禁用的依据与 aria-label 用） */
  index: number
  depth: number
  step: ScenarioStep
}
interface AddRow {
  type: 'add'
  container: number[]
  depth: number
}
type Row = StepRow | AddRow

const rows = computed<Row[]>(() => {
  const out: Row[] = []
  if (!draft.value) return out
  const walk = (steps: ScenarioStep[], container: number[], depth: number) => {
    for (let i = 0; i < steps.length; i++) {
      const step = steps[i]
      const chain = [...container, i]
      out.push({ type: 'step', chain, container, index: i, depth, step })
      if (step.kind === 'repeat') walk(step.steps, chain, depth + 1)
    }
    out.push({ type: 'add', container, depth })
  }
  walk(draft.value.steps, [], 0)
  return out
})

// ===================== 结构变更（全部按索引链操作草稿） =====================

function parentArr(container: number[]): ScenarioStep[] {
  if (!draft.value) throw new Error('draft missing')
  let arr = draft.value.steps
  for (const i of container) {
    const st = arr[i]
    if (!st || st.kind !== 'repeat') throw new Error('bad container path')
    arr = st.steps
  }
  return arr
}

function stepAt(chain: number[]): ScenarioStep {
  const container = chain.slice(0, -1)
  const idx = chain[chain.length - 1]
  return parentArr(container)[idx]
}

function addStep(container: number[], kind: ScenarioStepKind): void {
  parentArr(container).push(makeDefaultStep(kind))
}

function removeStep(chain: number[]): void {
  const container = chain.slice(0, -1)
  const idx = chain[chain.length - 1]
  parentArr(container).splice(idx, 1)
}

function moveStep(chain: number[], delta: -1 | 1): void {
  const container = chain.slice(0, -1)
  const idx = chain[chain.length - 1]
  const arr = parentArr(container)
  const j = idx + delta
  if (j < 0 || j >= arr.length) return
  const tmp = arr[idx]
  arr[idx] = arr[j]
  arr[j] = tmp
}

function patchStep(chain: number[], patch: Record<string, unknown>): void {
  const container = chain.slice(0, -1)
  const idx = chain[chain.length - 1]
  const arr = parentArr(container)
  arr[idx] = { ...arr[idx], ...patch } as ScenarioStep
}

/** 新循环步骤的层级 = 容器包裹层数 + 1；≤4 层才允许（Rust MAX_NESTING_DEPTH） */
function canAddRepeat(container: number[]): boolean {
  return container.length + 1 <= MAX_NESTING_DEPTH
}

// ===================== matcher 编辑（四选一 pattern + 方向） =====================

type MatcherKind = 'literal' | 'regex' | 'hex' | 'mask'
const MATCHER_KIND_LABELS: Record<MatcherKind, MessageKey> = {
  literal: 'scen.matLiteral',
  regex: 'scen.matRegex',
  hex: 'scen.matHex',
  mask: 'scen.matMask',
}
// 各匹配方式的输入框 placeholder 词条
const MATCHER_PAT_PH: Record<MatcherKind, MessageKey> = {
  literal: 'scen.patLiteral',
  regex: 'scen.patRegex',
  hex: 'scen.patHex',
  mask: 'scen.patMask',
}

function matcherKind(m: LineMatcher): MatcherKind {
  if (m.regex !== undefined) return 'regex'
  if (m.hex !== undefined) return 'hex'
  if (m.mask !== undefined) return 'mask'
  return 'literal'
}

/** seg 切换 pattern 种类：值随身携带，其余种类的字段清空 */
function setMatcherKind(chain: number[], kind: MatcherKind): void {
  const st = stepAt(chain)
  if (st.kind !== 'wait' && st.kind !== 'assert') return
  const m = st.matcher
  const carry = m.literal ?? m.regex ?? m.hex ?? m.mask ?? ''
  const next: LineMatcher = {}
  if (m.dir) next.dir = m.dir
  next[kind] = carry
  patchStep(chain, { matcher: next })
}

function setMatcherValue(chain: number[], value: string): void {
  const st = stepAt(chain)
  if (st.kind !== 'wait' && st.kind !== 'assert') return
  patchStep(chain, { matcher: { ...st.matcher, [matcherKind(st.matcher)]: value } })
}

function setMatcherDir(chain: number[], dir: '' | 'rx' | 'tx'): void {
  const st = stepAt(chain)
  if (st.kind !== 'wait' && st.kind !== 'assert') return
  const next: LineMatcher = { ...st.matcher }
  if (dir) next.dir = dir
  else delete next.dir
  patchStep(chain, { matcher: next })
}

// ===================== 变量表 =====================

const varRows = computed(() => Object.entries(draft.value?.variables ?? {}))

function addVar(): void {
  if (!draft.value) return
  const vars = { ...(draft.value.variables ?? {}) }
  let name = 'var1'
  let n = 1
  while (name in vars) {
    n += 1
    name = `var${n}`
  }
  vars[name] = ''
  draft.value = { ...draft.value, variables: vars }
}

function renameVar(oldKey: string, newKeyRaw: string): void {
  if (!draft.value) return
  const newKey = newKeyRaw.trim()
  if (!newKey || newKey === oldKey) return
  const vars: Record<string, string> = {}
  for (const [k, v] of Object.entries(draft.value.variables ?? {})) {
    vars[k === oldKey ? newKey : k] = v
  }
  draft.value = { ...draft.value, variables: vars }
}

function setVar(key: string, value: string): void {
  if (!draft.value) return
  draft.value = {
    ...draft.value,
    variables: { ...(draft.value.variables ?? {}), [key]: value },
  }
}

function delVar(key: string): void {
  if (!draft.value) return
  const vars: Record<string, string> = { ...(draft.value.variables ?? {}) }
  delete vars[key]
  draft.value = { ...draft.value, variables: vars }
}

// ===================== 校验错误定位 / 保存 =====================

const valError = computed(() => automation.validationError)
/** 后端错误路径（steps[0].steps[1]）→ 索引链，定位到步骤行 */
const errChain = computed(() => (valError.value ? parseStepPath(valError.value.path) : []))

function isErrRow(chain: number[]): boolean {
  return errChain.value.length > 0 && errChain.value.join('.') === chain.join('.')
}

const saving = ref(false)
async function save(): Promise<void> {
  if (!draft.value || saving.value) return
  saving.value = true
  try {
    const ok = await automation.saveScenario(props.entryId, draft.value)
    if (ok) emit('close')
  } finally {
    saving.value = false
  }
}

function close(): void {
  emit('close')
}
</script>

<template>
  <div class="sc-editor">
    <div v-if="!draft" class="panel-hint">{{ t('scen.missing') }}</div>
    <template v-else>
      <div class="sc-line">
        <input
          class="input sc-name-input"
          :value="draft.name"
          :aria-label="t('scen.nameAria')"
          :placeholder="t('scen.nameAria')"
          @change="draft.name = ($event.target as HTMLInputElement).value"
        />
      </div>

      <div v-if="valError" class="sc-val-error">
        {{ t('scen.validateFailed', { code: valError.code, path: valError.path, message: valError.message }) }}
      </div>

      <div class="sc-vars">
        <div class="sc-sub">{{ t('scen.varsSub') }}</div>
        <div v-for="[k, v] in varRows" :key="k" class="sc-var-row">
          <input
            class="input input-mono sc-var-key"
            :value="k"
            :aria-label="t('scen.varNameAria')"
            @change="renameVar(k, ($event.target as HTMLInputElement).value)"
          />
          <input
            class="input input-mono sc-var-val"
            :value="v"
            :aria-label="t('scen.varValueAria')"
            :placeholder="t('scen.varValuePh')"
            @change="setVar(k, ($event.target as HTMLInputElement).value)"
          />
          <button
            class="btn btn-ghost btn-sm sc-var-del"
            type="button"
            :title="t('scen.delVar', { name: k })"
            :aria-label="t('scen.delVar', { name: k })"
            @click="delVar(k)"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
          </button>
        </div>
        <button class="btn btn-ghost btn-sm sc-var-add" type="button" @click="addVar">{{ t('scen.addVar') }}</button>
      </div>

      <div class="sc-sub">{{ t('scen.stepsSub') }}</div>
      <div class="sc-steps">
        <template v-for="(row, ri) in rows" :key="row.type === 'step' ? row.chain.join('.') : `add${ri}`">
          <div
            v-if="row.type === 'step'"
            class="sc-row"
            :class="{ 'sc-row-error': isErrRow(row.chain) }"
            :data-depth="row.depth"
            :style="{ paddingInlineStart: `${row.depth * 14 + 6}px` }"
          >
            <div class="sc-row-head">
              <span class="sc-kind" :class="`k-${row.step.kind}`">{{ t(KIND_LABELS[row.step.kind]) }}</span>
              <span class="send-spacer"></span>
              <button
                class="btn btn-ghost btn-sm sc-up"
                type="button"
                :title="row.index === 0 ? undefined : t('scen.moveUpTitle')"
                :aria-label="t('scen.moveUpAria', { n: row.index, kind: t(KIND_LABELS[row.step.kind]) })"
                :disabled="row.index === 0"
                @click="moveStep(row.chain, -1)"
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="18 15 12 9 6 15"/></svg>
              </button>
              <button
                class="btn btn-ghost btn-sm sc-down"
                type="button"
                :aria-label="t('scen.moveDownAria', { n: row.index, kind: t(KIND_LABELS[row.step.kind]) })"
                @click="moveStep(row.chain, 1)"
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
              </button>
              <button
                class="btn btn-ghost btn-sm sc-del"
                type="button"
                :title="t('scen.delStepTitle', { kind: t(KIND_LABELS[row.step.kind]) })"
                :aria-label="t('scen.delStepAria', { n: row.index, kind: t(KIND_LABELS[row.step.kind]) })"
                @click="removeStep(row.chain)"
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
              </button>
            </div>
            <div v-if="isErrRow(row.chain) && valError" class="sc-row-err-msg">{{ valError.message }}</div>

            <!-- send -->
            <div v-if="row.step.kind === 'send'" class="sc-fields">
              <select
                class="select sc-f-mode"
                :value="row.step.mode"
                :aria-label="t('scen.sendModeAria')"
                @change="patchStep(row.chain, { mode: ($event.target as HTMLSelectElement).value })"
              >
                <option value="ascii">ASCII</option>
                <option value="hex">HEX</option>
              </select>
              <input
                class="input input-mono sc-f-text"
                :value="row.step.text"
                :placeholder="t('scen.sendTextPh')"
                :aria-label="t('scen.sendTextAria')"
                @change="patchStep(row.chain, { text: ($event.target as HTMLInputElement).value })"
              />
              <label class="check sc-f-nl">
                <input
                  type="checkbox"
                  :checked="row.step.appendNewline"
                  @change="patchStep(row.chain, { appendNewline: ($event.target as HTMLInputElement).checked })"
                />
                <span class="box"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg></span>
                <span>{{ t('scen.newline') }}</span>
              </label>
            </div>

            <!-- delay -->
            <div v-else-if="row.step.kind === 'delay'" class="sc-fields">
              <input
                class="input input-mono sc-f-num"
                type="number"
                min="0"
                :value="row.step.ms"
                :aria-label="t('scen.delayAria')"
                @change="patchStep(row.chain, { ms: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
              />
              <span class="sc-unit">{{ t('scen.delayUnit') }}</span>
            </div>

            <!-- signal -->
            <div v-else-if="row.step.kind === 'signal'" class="sc-fields">
              <select
                class="select sc-f-pin"
                :value="row.step.pin"
                :aria-label="t('scen.pinAria')"
                @change="patchStep(row.chain, { pin: ($event.target as HTMLSelectElement).value })"
              >
                <option value="dtr">DTR</option>
                <option value="rts">RTS</option>
              </select>
              <div class="seg" role="group" :aria-label="t('scen.levelAria')">
                <button
                  class="seg-item"
                  :class="{ active: row.step.level }"
                  type="button"
                  @click="patchStep(row.chain, { level: true })"
                >{{ t('scen.high') }}</button>
                <button
                  class="seg-item"
                  :class="{ active: !row.step.level }"
                  type="button"
                  @click="patchStep(row.chain, { level: false })"
                >{{ t('scen.low') }}</button>
              </div>
            </div>

            <!-- wait / assert 共用 matcher 编辑 -->
            <template v-else-if="row.step.kind === 'wait' || row.step.kind === 'assert'">
              <div class="sc-fields sc-matcher">
                <select
                  class="select sc-f-dir"
                  :value="row.step.matcher.dir ?? ''"
                  :aria-label="t('scen.dirAria')"
                  @change="setMatcherDir(row.chain, ($event.target as HTMLSelectElement).value as '' | 'rx' | 'tx')"
                >
                  <option value="">{{ t('scen.dirAny') }}</option>
                  <option value="rx">RX</option>
                  <option value="tx">TX</option>
                </select>
                <div class="seg" role="group" :aria-label="t('scen.matcherAria')">
                  <button
                    v-for="mk in (['literal', 'regex', 'hex', 'mask'] as const)"
                    :key="mk"
                    class="seg-item"
                    :class="{ active: matcherKind(row.step.matcher) === mk }"
                    type="button"
                    @click="setMatcherKind(row.chain, mk)"
                  >{{ t(MATCHER_KIND_LABELS[mk]) }}</button>
                </div>
                <input
                  class="input input-mono sc-f-pattern"
                  :value="row.step.matcher[matcherKind(row.step.matcher)] ?? ''"
                  :placeholder="t(MATCHER_PAT_PH[matcherKind(row.step.matcher)])"
                  :aria-label="t('scen.patternAria')"
                  @change="setMatcherValue(row.chain, ($event.target as HTMLInputElement).value)"
                />
              </div>
              <div class="sc-fields">
                <template v-if="row.step.kind === 'wait'">
                  <input
                    class="input input-mono sc-f-num"
                    type="number"
                    min="0"
                    :value="row.step.timeoutMs"
                    :aria-label="t('scen.timeoutAria')"
                    @change="patchStep(row.chain, { timeoutMs: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
                  />
                  <span class="sc-unit">{{ t('scen.timeoutUnit') }}</span>
                  <label class="check sc-f-save">
                    <input
                      type="checkbox"
                      :checked="!!row.step.save"
                      @change="patchStep(row.chain, { save: ($event.target as HTMLInputElement).checked ? { variable: 'value', group: 0 } : undefined })"
                    />
                    <span class="box"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg></span>
                    <span>{{ t('scen.captureVar') }}</span>
                  </label>
                  <template v-if="row.step.save">
                    <input
                      class="input input-mono sc-f-var"
                      :value="row.step.save.variable"
                      :aria-label="t('scen.capVarAria')"
                      @change="row.step.save && patchStep(row.chain, { save: { ...row.step.save, variable: ($event.target as HTMLInputElement).value } })"
                    />
                    <input
                      class="input input-mono sc-f-group"
                      type="number"
                      min="0"
                      :value="row.step.save.group"
                      :aria-label="t('scen.capGroupAria')"
                      @change="row.step.save && patchStep(row.chain, { save: { ...row.step.save, group: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) } })"
                    />
                    <span class="sc-unit">{{ t('scen.capGroupUnit') }}</span>
                  </template>
                </template>
                <template v-else>
                  <input
                    class="input input-mono sc-f-num"
                    type="number"
                    min="0"
                    :value="row.step.withinLast"
                    :aria-label="t('scen.withinAria')"
                    @change="patchStep(row.chain, { withinLast: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
                  />
                  <span class="sc-unit">{{ t('scen.withinUnit') }}</span>
                  <input
                    class="input input-mono sc-f-msg"
                    :value="row.step.message"
                    :placeholder="t('scen.assertMsgPh')"
                    :aria-label="t('scen.assertMsgAria')"
                    @change="patchStep(row.chain, { message: ($event.target as HTMLInputElement).value })"
                  />
                </template>
              </div>
            </template>

            <!-- repeat -->
            <div v-else-if="row.step.kind === 'repeat'" class="sc-fields">
              <span class="sc-unit">{{ t('scen.repeatPrefix') }}</span>
              <input
                class="input input-mono sc-f-num"
                type="number"
                min="0"
                :value="row.step.times"
                :aria-label="t('scen.timesAria')"
                @change="patchStep(row.chain, { times: Math.max(0, Number(($event.target as HTMLInputElement).value) || 0) })"
              />
              <span class="sc-unit">{{ t('scen.timesUnit') }}</span>
            </div>
          </div>

          <div
            v-else
            class="sc-addrow"
            :style="{ paddingInlineStart: `${row.depth * 14 + 6}px` }"
          >
            <button
              v-for="k in (['send', 'delay', 'signal', 'wait', 'assert', 'repeat'] as const)"
              :key="k"
              class="btn btn-ghost btn-sm sc-add"
              :data-kind="k"
              type="button"
              :disabled="k === 'repeat' && !canAddRepeat(row.container)"
              :title="k === 'repeat' && !canAddRepeat(row.container)
                ? t('scen.maxDepthTitle', { n: MAX_NESTING_DEPTH })
                : t('scen.addStepTitle', { kind: t(KIND_LABELS[k]) })"
              @click="addStep(row.container, k)"
            >＋{{ t(KIND_LABELS[k]) }}</button>
          </div>
        </template>
      </div>

      <div class="sc-actions">
        <button class="btn btn-primary btn-sm sc-save" type="button" :disabled="saving" @click="save">
          {{ saving ? t('scen.saving') : t('scen.save') }}
        </button>
        <button class="btn btn-ghost btn-sm sc-close" type="button" @click="close">{{ t('scen.backToList') }}</button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.sc-editor {
  display: flex;
  flex-direction: column;
  gap: 8px;
  font-size: 12px;
}
.sc-line {
  display: flex;
}
.sc-name-input {
  flex: 1;
}
.sc-val-error {
  color: var(--err);
  border: 1px solid var(--err);
  border-radius: var(--r-sm);
  padding: 4px 8px;
  word-break: break-all;
}
.sc-sub {
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.5px;
  color: var(--text-dim);
  text-transform: uppercase;
}
.sc-vars {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.sc-var-row {
  display: flex;
  align-items: center;
  gap: 4px;
}
.sc-var-key {
  width: 110px;
}
.sc-var-val {
  flex: 1;
  min-width: 0;
}
.sc-steps {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.sc-row {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding-block: 4px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.sc-row-error {
  border-color: var(--err);
}
.sc-row-err-msg {
  color: var(--err);
  padding-inline-end: 6px;
  word-break: break-all;
}
.sc-row-head {
  display: flex;
  align-items: center;
  gap: 2px;
  padding-inline-end: 4px;
}
.sc-kind {
  font-weight: 600;
  color: var(--text-muted);
}
.sc-kind.k-wait,
.sc-kind.k-assert {
  color: var(--accent);
}
.sc-kind.k-repeat {
  color: var(--tx);
}
.sc-fields {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
  padding-inline-end: 6px;
}
.sc-matcher {
  flex-wrap: nowrap;
}
.sc-f-text,
.sc-f-pattern,
.sc-f-msg {
  flex: 1;
  min-width: 60px;
}
.sc-f-num {
  width: 84px;
}
.sc-f-var {
  width: 90px;
}
.sc-f-group {
  width: 56px;
}
.sc-unit {
  color: var(--text-dim);
  white-space: nowrap;
}
.sc-addrow {
  display: flex;
  gap: 2px;
  flex-wrap: wrap;
}
.sc-add {
  font-size: 11px;
  padding-inline: 6px;
}
.sc-actions {
  display: flex;
  gap: 6px;
}
</style>
