/**
 * 场景自动化 v1 类型镜像（Stage 3 Task 4，plan：
 * docs/superpowers/plans/2026-09-11-stage-3-automation-and-replay.md）。
 *
 * **本文件是 crates/bytetide-core/src/automation/{model.rs,matcher.rs} 与
 * src-tauri/src/commands/automation.rs 的 serde camelCase 形状的逐字段前端镜像**：
 * - `Scenario`/`ScenarioStep`/`LineMatcher`/`CaptureSpec`/`SendModeDef`/`PinDef`
 *   ← model.rs / matcher.rs（黄金样例 testdata/scenarios/{minimal,full}.json）
 * - `ScenarioErrorInfo`/`ScenarioValidateSummary`/`ScenarioRunView`/进度载荷
 *   ← commands/automation.rs 的 `ScenarioErrorInfo`/`ValidatedScenarioSummary`/
 *   `ScenarioRunView`（skip_serializing_if 的可选字段 → TS 可选属性）
 * - 步骤枚举 `kind` 内部 tag + camelCase 字段（appendNewline/timeoutMs/withinLast），
 *   子枚举小写（ascii/hex、dtr/rts、rx/tx）。改 Rust 侧形状必须同步这里与 fixture。
 *
 * 另含：库条目（前端封装，非后端契约）、结构化解析守卫（localStorage 信封与
 * JSON 导入共用；畸形数据 throw，由调用方转拒绝）、静态叶步计数（与后端
 * count_executed_leaves 同口径）。
 */

/** Rust `SCENARIO_SCHEMA`（model.rs）：schema 标识，必须恰为该值 */
export const SCENARIO_SCHEMA = 'bytetide.scenario'
/** Rust `SCENARIO_VERSION`：唯一支持的 schema 版本 */
export const SCENARIO_VERSION = 1
/** Rust `MAX_NESTING_DEPTH`：Repeat 嵌套层数上限（4 层允许，第 5 层报错） */
export const MAX_NESTING_DEPTH = 4
/** Rust `MAX_EXECUTED_STEPS`：执行步静态上界 */
export const MAX_EXECUTED_STEPS = 10_000
/** Rust `MAX_DELAY_WAIT_MS`：Delay.ms 与 Wait.timeoutMs 上限（毫秒） */
export const MAX_DELAY_WAIT_MS = 600_000
/** Rust `MAX_REPEAT_TIMES`：Repeat.times 上限 */
export const MAX_REPEAT_TIMES = 10_000

/** Rust `SendModeDef`（serde lowercase） */
export type SendModeDef = 'ascii' | 'hex'
/** Rust `PinDef`（serde lowercase；不支持其他引脚） */
export type PinDef = 'dtr' | 'rts'
/** Rust `Dir`（serial/port.rs，serde lowercase）；matcher 的 dir 为 Option → 可缺省 */
export type MatcherDir = 'rx' | 'tx'

/**
 * Rust `LineMatcher`（matcher.rs，全字段 serde default + skip_serializing_if）：
 * 恰一 pattern（literal/regex/hex/mask 四选一），dir 可缺省=任意方向。
 */
export interface LineMatcher {
  dir?: MatcherDir
  literal?: string
  regex?: string
  hex?: string
  mask?: string
}

/** Rust `CaptureSpec`：Wait 命中后的变量捕获，group=0 整匹配，>0 为 regex 捕获组 */
export interface CaptureSpec {
  variable: string
  group: number
}

/**
 * Rust `ScenarioStep`（serde tag="kind" + rename_all="camelCase"）六变体：
 * send/delay/signal/wait/assert/repeat。仅 Send.text 与 Assert.message 参与
 * `${name}` 变量替换（后端语义，前端不做替换只透传模板原样）。
 */
export type ScenarioStep =
  | { kind: 'send'; mode: SendModeDef; text: string; appendNewline: boolean }
  | { kind: 'delay'; ms: number }
  | { kind: 'signal'; pin: PinDef; level: boolean }
  | { kind: 'wait'; matcher: LineMatcher; timeoutMs: number; save?: CaptureSpec }
  | { kind: 'assert'; matcher: LineMatcher; withinLast: number; message: string }
  | { kind: 'repeat'; times: number; steps: ScenarioStep[] }

export type ScenarioStepKind = ScenarioStep['kind']

/** Rust `Scenario`（model.rs）：variables 走 serde default（可缺省=空表） */
export interface Scenario {
  schema: string
  version: number
  name: string
  variables?: Record<string, string>
  steps: ScenarioStep[]
}

/**
 * 稳定错误码（model.rs `ScenarioErrorCode::as_str`，17 个；GUI/CLI 按字面量分支，
 * 勿改字符串）。`invalid_dir` 仅为后端码表完整性保留（封闭枚举在 serde 层拒绝）。
 */
export type ScenarioErrorCode =
  | 'invalid_schema'
  | 'invalid_version'
  | 'empty_name'
  | 'empty_steps'
  | 'matcher_conflict'
  | 'invalid_regex'
  | 'invalid_hex'
  | 'invalid_mask'
  | 'invalid_dir'
  | 'nesting_too_deep'
  | 'step_limit_exceeded'
  | 'delay_too_long'
  | 'wait_too_long'
  | 'repeat_too_many'
  | 'invalid_variable_name'
  | 'undefined_variable'
  | 'capture_group_invalid'

/** 校验错误（commands/automation.rs `ScenarioErrorInfo`）：path 如 `steps[3].steps[1]`，
 *  根字段为 schema/version/name/steps/variables[".."] */
export interface ScenarioErrorInfo {
  code: ScenarioErrorCode | string
  path: string
  message: string
}

/** `scenario_validate_cmd` 返回（commands/automation.rs `ValidatedScenarioSummary`）：
 *  stepCount = 执行叶步静态上界（Repeat 展开口径） */
export interface ScenarioValidateSummary {
  ok: boolean
  error?: ScenarioErrorInfo
  stepCount: number
  name: string
}

/** 运行状态（commands/automation.rs `RunStatus::as_str`） */
export type ScenarioRunStatus = 'running' | 'passed' | 'failed' | 'cancelled'

/** scenario_report_cmd 的 format 参数（`json`=pretty JSON | `junit`=JUnit XML） */
export type ScenarioReportFormat = 'json' | 'junit'

/** 进度水位（`ProgressView`）：当前执行到第几个叶子步 / 静态总步数 */
export interface ScenarioProgressView {
  currentStep: number
  totalSteps: number
}

/** `scenario_status_cmd` 返回与 `scenario-finished` 事件载荷（`ScenarioRunView`）：
 *  可选字段在后端 skip_serializing_if 缺省（durationMs=未完成、error=仅 failed、
 *  progress=尚无水位时缺省） */
export interface ScenarioRunView {
  runId: string
  sessionId: string
  status: ScenarioRunStatus
  startedEpochMs: number
  durationMs?: number
  error?: string
  progress?: ScenarioProgressView
}

/** `scenario-progress` 事件载荷：每叶子步开始一条、稀疏。kind=叶步种类近似识别
 *  （send/signal/delay/wait/assert；repeat 不是叶步不产生进度） */
export interface ScenarioProgressPayload {
  runId: string
  sessionId: string
  currentStep: number
  totalSteps: number
  kind: string
}

// ===================== 前端库条目（非后端契约） =====================

/** 场景库条目：id 为前端库内标识（localStorage `serialtool.scenarios`）；
 *  lastRun* 为最近一次运行的徽标数据（运行开始置 running，finished 事件回写终态） */
export interface ScenarioLibraryEntry {
  id: string
  scenario: Scenario
  lastRunStatus?: ScenarioRunStatus
  lastRunAt?: number
}

// ===================== 结构化解析守卫（信封/导入共用） =====================

function isObj(x: unknown): x is Record<string, unknown> {
  return typeof x === 'object' && x !== null && !Array.isArray(x)
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined
}

function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined
}

/** 解析 matcher（畸形 pattern 字段静默丢弃——后端 validate 层负责语义校验） */
export function parseLineMatcher(raw: unknown): LineMatcher {
  if (!isObj(raw)) throw new Error('matcher must be an object')
  const out: LineMatcher = {}
  const dir = raw.dir
  if (dir === 'rx' || dir === 'tx') out.dir = dir
  for (const key of ['literal', 'regex', 'hex', 'mask'] as const) {
    const v = str(raw[key])
    if (v !== undefined) out[key] = v
  }
  return out
}

function parseCaptureSpec(raw: unknown): CaptureSpec | undefined {
  if (!isObj(raw)) return undefined
  const variable = str(raw.variable)
  const group = num(raw.group)
  if (!variable || group === undefined) return undefined
  return { variable, group }
}

function parseStep(raw: unknown): ScenarioStep {
  if (!isObj(raw)) throw new Error('step must be an object')
  const kind = raw.kind
  switch (kind) {
    case 'send': {
      const mode = raw.mode
      if (mode !== 'ascii' && mode !== 'hex') throw new Error('invalid send mode')
      if (typeof raw.text !== 'string' || typeof raw.appendNewline !== 'boolean')
        throw new Error('invalid send fields')
      return { kind: 'send', mode, text: raw.text, appendNewline: raw.appendNewline }
    }
    case 'delay': {
      const ms = num(raw.ms)
      if (ms === undefined) throw new Error('invalid delay ms')
      return { kind: 'delay', ms }
    }
    case 'signal': {
      const pin = raw.pin
      if (pin !== 'dtr' && pin !== 'rts') throw new Error('invalid signal pin')
      if (typeof raw.level !== 'boolean') throw new Error('invalid signal level')
      return { kind: 'signal', pin, level: raw.level }
    }
    case 'wait': {
      const timeoutMs = num(raw.timeoutMs)
      if (timeoutMs === undefined) throw new Error('invalid wait timeoutMs')
      const step: ScenarioStep = { kind: 'wait', matcher: parseLineMatcher(raw.matcher), timeoutMs }
      const save = parseCaptureSpec(raw.save)
      if (save) step.save = save
      return step
    }
    case 'assert': {
      const withinLast = num(raw.withinLast)
      if (withinLast === undefined || typeof raw.message !== 'string')
        throw new Error('invalid assert fields')
      return {
        kind: 'assert',
        matcher: parseLineMatcher(raw.matcher),
        withinLast,
        message: raw.message,
      }
    }
    case 'repeat': {
      const times = num(raw.times)
      if (times === undefined || !Array.isArray(raw.steps)) throw new Error('invalid repeat fields')
      return { kind: 'repeat', times, steps: raw.steps.map(parseStep) }
    }
    default:
      throw new Error(`unknown step kind: ${String(kind)}`)
  }
}

/** 解析场景（localStorage 信封 / JSON 导入共用）：形状不符 throw，由调用方转拒绝；
 *  schema/version/name 的**值**合法性（恰为 bytetide.scenario / 1）由后端
 *  validate 层把关，这里只守结构。 */
export function parseScenario(raw: unknown): Scenario {
  if (!isObj(raw)) throw new Error('scenario must be an object')
  const schema = str(raw.schema)
  const version = num(raw.version)
  const name = str(raw.name)
  if (schema === undefined || version === undefined || name === undefined)
    throw new Error('scenario requires schema/version/name')
  if (!Array.isArray(raw.steps) || raw.steps.length === 0)
    throw new Error('scenario requires non-empty steps')
  const variables: Record<string, string> = {}
  if (raw.variables !== undefined) {
    if (!isObj(raw.variables)) throw new Error('variables must be an object')
    for (const [k, v] of Object.entries(raw.variables)) {
      if (typeof v === 'string') variables[k] = v
    }
  }
  return { schema, version, name, variables, steps: raw.steps.map(parseStep) }
}

/** 解析库条目（scenario-library codec 用）：id 必须为字符串，scenario 走结构守卫 */
export function parseScenarioLibraryEntry(raw: unknown): ScenarioLibraryEntry {
  if (!isObj(raw)) throw new Error('library entry must be an object')
  const id = str(raw.id)
  if (!id) throw new Error('library entry requires id')
  const entry: ScenarioLibraryEntry = { id, scenario: parseScenario(raw.scenario) }
  const st = raw.lastRunStatus
  if (st === 'running' || st === 'passed' || st === 'failed' || st === 'cancelled')
    entry.lastRunStatus = st
  const at = num(raw.lastRunAt)
  if (at !== undefined) entry.lastRunAt = at
  return entry
}

// ===================== 纯函数工具 =====================

/** 深拷贝场景（编辑器草稿 / duplicate 用；structuredClone 覆盖全部可序列化形状） */
export function cloneScenario(s: Scenario): Scenario {
  return JSON.parse(JSON.stringify(s)) as Scenario
}

/** 静态叶步计数（与后端 count_executed_leaves 同口径：repeat 展开 times × 体），
 *  面板「步数」列与校验摘要 stepCount 展示共用 */
export function countScenarioLeaves(steps: ScenarioStep[]): number {
  let n = 0
  for (const s of steps) {
    if (s.kind === 'repeat') n += s.times * countScenarioLeaves(s.steps)
    else n += 1
  }
  return n
}

/** 由索引链构造后端错误路径（`steps[0].steps[2]`）；顶层块路径为 `steps` */
export function stepPath(indices: number[]): string {
  return indices.map((i) => `steps[${i}]`).join('.')
}

/** 解析后端错误路径为索引链（如 `steps[3].steps[1]` → [3,1]；根路径返回 []） */
export function parseStepPath(path: string): number[] {
  const out: number[] = []
  const re = /steps\[(\d+)\]/g
  let m: RegExpExecArray | null
  while ((m = re.exec(path)) !== null) out.push(Number(m[1]))
  return out
}

/** 编辑器新建步骤的出厂默认值（值均落在后端限制内） */
export function makeDefaultStep(kind: ScenarioStepKind): ScenarioStep {
  switch (kind) {
    case 'send':
      return { kind: 'send', mode: 'ascii', text: '', appendNewline: true }
    case 'delay':
      return { kind: 'delay', ms: 100 }
    case 'signal':
      return { kind: 'signal', pin: 'dtr', level: true }
    case 'wait':
      return {
        kind: 'wait',
        matcher: { dir: 'rx', literal: '' },
        timeoutMs: 5000,
      }
    case 'assert':
      return {
        kind: 'assert',
        matcher: { dir: 'rx', literal: '' },
        withinLast: 10,
        message: '',
      }
    case 'repeat':
      return { kind: 'repeat', times: 3, steps: [] }
  }
}

/** 新场景出厂值（schema/version 预填；patch 可覆盖 steps/variables 等字段） */
export function makeScenario(
  name: string,
  patch: Partial<Omit<Scenario, 'schema' | 'version' | 'name'>> = {},
): Scenario {
  return {
    schema: SCENARIO_SCHEMA,
    version: SCENARIO_VERSION,
    name,
    variables: {},
    steps: [{ kind: 'send', mode: 'ascii', text: '', appendNewline: true }],
    ...patch,
  }
}
