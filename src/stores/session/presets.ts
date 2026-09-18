import {
  DEFAULT_LOG_CONFIG,
  DEFAULT_PLOT_CONFIG,
  type ConfigPreset,
  type FilterStage,
  type Keyword,
  type LogConfig,
  type PlotConfig,
  type PortConfig,
  type PortPreset,
  type PresetCategory,
  type SendPreset,
  type SendSequence,
  type SeqStep,
} from '../../types'
import { makeCodec } from '../../persistence/schema'
import { loadValue, saveStored } from '../../persistence/storage'
import { t } from '../../i18n'
import { newKeywordId, newRuleId } from './rules'
import type { Session } from './model'

/**
 * 预设/持久化域纯函数（Task 6）：连接配置预设、快捷帧、发送序列、配置预设库、
 * 搜索历史、日志配置——localStorage 读写（src/persistence v1 信封，键名不变，
 * 旧裸 JSON 首次成功读取时自动迁移回写信封）与列表运算。列表运算返回新数组
 * （或 null=拒绝），由门面写回 store 状态；本模块不持有响应式状态、不调 useSessionStore。
 */

// ===================== 序列运行（自动化域：步骤循环 + 可中断休眠） =====================

/** 序列运行进度（UI 实时显示：轮次/步骤）；同一时刻全局最多一个序列在跑 */
export interface SeqRunState {
  sessionId: string
  seqId: string
  round: number
  step: number
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** 可中断休眠：50ms 粒度轮询停止旗标，保证「停止」按钮即时生效 */
export async function sleepInterruptible(ms: number, flag: { stopped: boolean }) {
  let left = Math.max(0, ms)
  while (left > 0 && !flag.stopped) {
    const chunk = Math.min(50, left)
    await sleep(chunk)
    left -= chunk
  }
}

/** 序列运行的门面依赖注入（发送/信号走 store action，存活判断读注册表） */
export interface SequenceRunDeps {
  /** 目标会话是否仍满足运行条件（live + connected） */
  isAlive: () => boolean
  send: (id: string, payload: string, mode: 'ascii' | 'hex') => Promise<void>
  setSignal: (id: string, pin: 'dtr' | 'rts', level: boolean) => Promise<void>
}

/**
 * 运行发送序列步骤循环（原 runSequence 主体）：步骤按序执行（发送/延时/信号），
 * 循环模式轮间隔后重复。停止旗标置位、会话失活（断开/关闭）即中止；发送/信号
 * 失败错误向上冒泡（调用方 finally 清运行态并提示 UI）。
 */
export async function runSequenceSteps(
  id: string,
  seq: SendSequence,
  runState: SeqRunState,
  flag: { stopped: boolean },
  deps: SequenceRunDeps,
): Promise<void> {
  do {
    runState.round += 1
    for (let i = 0; i < seq.steps.length; i++) {
      if (flag.stopped) return
      if (!deps.isAlive()) return
      runState.step = i
      const st = seq.steps[i]
      if (st.kind === 'send') {
        const payload = st.mode === 'ascii' && st.appendNewline ? st.payload + '\n' : st.payload
        await deps.send(id, payload, st.mode)
      } else if (st.kind === 'signal') {
        await deps.setSignal(id, st.pin, st.level)
      }
      if (st.kind === 'delay') await sleepInterruptible(st.ms, flag)
    }
    if (seq.loop && !flag.stopped) await sleepInterruptible(seq.intervalMs, flag)
  } while (seq.loop && !flag.stopped)
}

// ===================== 日志配置（serialtool.logConfig） =====================

export const LOG_CONFIG_KEY = 'serialtool.logConfig'

const logConfigCodec = makeCodec<LogConfig>(
  'logConfig',
  (raw) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid log cfg')
    const parsed = raw as Partial<LogConfig>
    return {
      logPathTemplate: parsed.logPathTemplate ?? '',
      lineTsFormat: parsed.lineTsFormat || DEFAULT_LOG_CONFIG.lineTsFormat,
      viewBufCap:
        typeof parsed.viewBufCap === 'number' && Number.isFinite(parsed.viewBufCap)
          ? parsed.viewBufCap
          : DEFAULT_LOG_CONFIG.viewBufCap,
      midnightRotate: parsed.midnightRotate === true,
    }
  },
)

export function loadLogConfig(): LogConfig {
  return loadValue(LOG_CONFIG_KEY, logConfigCodec, { ...DEFAULT_LOG_CONFIG })
}

/** 合并补丁并钳制视图缓冲上限：防误设过小（裁剪风暴）或过大（内存失控） */
export function mergeLogConfig(current: LogConfig, patch: Partial<LogConfig>): LogConfig {
  const next = { ...current, ...patch }
  next.viewBufCap = Math.min(Math.max(next.viewBufCap, 10000), 1000000)
  return next
}

export function saveLogConfig(cfg: LogConfig): void {
  saveStored(LOG_CONFIG_KEY, logConfigCodec.schema, cfg)
}

// ===================== 搜索历史（serialtool.searchHistory） =====================

const SEARCH_HISTORY_KEY = 'serialtool.searchHistory'
const SEARCH_HISTORY_MAX = 20

const searchHistoryCodec = makeCodec<string[]>(
  'searchHistory',
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid search history')
    return raw.filter((x): x is string => typeof x === 'string')
  },
)

export function loadSearchHistory(): string[] {
  return loadValue(SEARCH_HISTORY_KEY, searchHistoryCodec, [])
}

/** 追加搜索历史（去重、截断）；空串拒绝返回 null（调用方不动状态） */
export function pushSearchHistory(list: string[], pattern: string): string[] | null {
  const p = pattern.trim()
  if (!p) return null
  return [p, ...list.filter((x) => x !== p)].slice(0, SEARCH_HISTORY_MAX)
}

export function removeSearchHistory(list: string[], pattern: string): string[] {
  return list.filter((x) => x !== pattern)
}

export function saveSearchHistory(list: string[]): void {
  saveStored(SEARCH_HISTORY_KEY, searchHistoryCodec.schema, list)
}

// ===================== 连接配置预设（serialtool.portPresets） =====================

const PRESETS_KEY = 'serialtool.portPresets'

let presetSeq = 0
/** 预设 id 生成（portPresets/sendPresets/configPresets 共用计数器，防同毫秒撞号） */
export function newPresetId(prefix: string): string {
  presetSeq += 1
  return `${prefix}${Date.now().toString(36)}${presetSeq}`
}

const portPresetsCodec = makeCodec<PortPreset[]>(
  'portPresets',
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid port presets')
    return (raw as unknown[]).filter(
      (x): x is { id: string; name: string; config: PortConfig } =>
        !!x &&
        typeof x === 'object' &&
        typeof (x as { id?: unknown }).id === 'string' &&
        typeof (x as { name?: unknown }).name === 'string' &&
        !!(x as { config?: unknown }).config,
    ).map((x) => ({ id: x.id, name: x.name, config: { ...x.config } }))
  },
)

export function loadPresets(): PortPreset[] {
  return loadValue(PRESETS_KEY, portPresetsCodec, [])
}

export function savePresets(list: PortPreset[]): void {
  saveStored(PRESETS_KEY, portPresetsCodec.schema, list)
}

/** 新增连接配置预设（拷贝 config 防外部引用串改）；空名拒绝返回 null */
export function addPortPreset(
  list: PortPreset[],
  name: string,
  config: PortConfig,
): PortPreset[] | null {
  const trimmed = name.trim()
  if (!trimmed) return null
  return [...list, { id: newPresetId('p'), name: trimmed, config: { ...config } }]
}

export function renamePortPreset(list: PortPreset[], id: string, name: string): PortPreset[] | null {
  const trimmed = name.trim()
  if (!trimmed) return null
  return list.map((p) => (p.id === id ? { ...p, name: trimmed } : p))
}

export function removePortPreset(list: PortPreset[], id: string): PortPreset[] {
  return list.filter((p) => p.id !== id)
}

// ===================== 快捷帧（serialtool.sendPresets，全局库 cap 50） =====================

const SEND_PRESETS_KEY = 'serialtool.sendPresets'
export const SEND_PRESETS_CAP = 50

const sendPresetsCodec = makeCodec<SendPreset[]>(
  'sendPresets',
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid send presets')
    return (raw as unknown[]).filter(
      (x): x is SendPreset =>
        !!x &&
        typeof x === 'object' &&
        typeof (x as { id?: unknown }).id === 'string' &&
        typeof (x as { name?: unknown }).name === 'string' &&
        typeof (x as { payload?: unknown }).payload === 'string' &&
        ((x as { mode?: unknown }).mode === 'ascii' || (x as { mode?: unknown }).mode === 'hex'),
    )
  },
)

export function loadSendPresets(): SendPreset[] {
  return loadValue(SEND_PRESETS_KEY, sendPresetsCodec, [])
}

/** 保存快捷帧：带 id 为改名/改内容，否则新增（超出上限丢最旧）；空名拒绝返回 null */
export function upsertSendPreset(
  list: SendPreset[],
  input: { id?: string; name: string; payload: string; mode: 'ascii' | 'hex' },
): SendPreset[] | null {
  const name = input.name.trim()
  if (!name) return null
  if (input.id) {
    return list.map((p) =>
      p.id === input.id ? { ...p, name, payload: input.payload, mode: input.mode } : p,
    )
  }
  const next = [
    ...list,
    { id: newPresetId('q'), name, payload: input.payload, mode: input.mode },
  ]
  if (next.length > SEND_PRESETS_CAP) {
    return next.slice(next.length - SEND_PRESETS_CAP)
  }
  return next
}

export function removeSendPresetById(list: SendPreset[], id: string): SendPreset[] {
  return list.filter((p) => p.id !== id)
}

export function saveSendPresets(list: SendPreset[]): void {
  saveStored(SEND_PRESETS_KEY, sendPresetsCodec.schema, list)
}

/** 单步发送历史（会话级 sendHistory，去重置顶截断 20） */
export function updateSendHistory(list: string[], text: string): string[] {
  return [text, ...list.filter((t) => t !== text)].slice(0, 20)
}

// ===================== 发送序列（serialtool.sendSequences，全局库） =====================

const SEND_SEQUENCES_KEY = 'serialtool.sendSequences'

export function isSeqStep(x: unknown): x is SeqStep {
  if (!x || typeof x !== 'object') return false
  const s = x as Partial<SeqStep>
  if (s.kind === 'send')
    return (
      typeof s.payload === 'string' &&
      (s.mode === 'ascii' || s.mode === 'hex') &&
      typeof s.appendNewline === 'boolean'
    )
  if (s.kind === 'delay') return typeof s.ms === 'number' && Number.isFinite(s.ms) && s.ms >= 0
  if (s.kind === 'signal') return (s.pin === 'dtr' || s.pin === 'rts') && typeof s.level === 'boolean'
  return false
}

const sendSequencesCodec = makeCodec<SendSequence[]>(
  'sendSequences',
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid send sequences')
    return (raw as unknown[])
      .filter(
        (x): x is SendSequence =>
          !!x &&
          typeof x === 'object' &&
          typeof (x as { id?: unknown }).id === 'string' &&
          typeof (x as { name?: unknown }).name === 'string' &&
          Array.isArray((x as { steps?: unknown }).steps) &&
          typeof (x as { loop?: unknown }).loop === 'boolean' &&
          typeof (x as { intervalMs?: unknown }).intervalMs === 'number' &&
          Number.isFinite((x as { intervalMs?: unknown }).intervalMs),
      )
      .map((x) => ({ ...x, steps: x.steps.filter(isSeqStep) }))
  },
)

export function loadSendSequences(): SendSequence[] {
  return loadValue(SEND_SEQUENCES_KEY, sendSequencesCodec, [])
}

/** 保存序列（整体覆盖同 id；intervalMs 钳制 ≥50ms 防定时器风暴）；空名拒绝返回 null */
export function upsertSendSequence(list: SendSequence[], seq: SendSequence): SendSequence[] | null {
  const name = seq.name.trim()
  if (!name) return null
  const next: SendSequence = {
    ...seq,
    name,
    steps: seq.steps.filter(isSeqStep),
    intervalMs: Math.max(50, seq.intervalMs),
  }
  if (list.some((x) => x.id === next.id)) {
    return list.map((x) => (x.id === next.id ? next : x))
  }
  return [...list, next]
}

export function removeSendSequenceById(list: SendSequence[], id: string): SendSequence[] {
  return list.filter((s) => s.id !== id)
}

export function saveSendSequences(list: SendSequence[]): void {
  saveStored(SEND_SEQUENCES_KEY, sendSequencesCodec.schema, list)
}

// ===================== 配置预设库（serialtool.configPresets） =====================

const CONFIG_PRESETS_KEY = 'serialtool.configPresets'
const CONFIG_PRESET_CATEGORIES: PresetCategory[] = ['filters', 'keywords', 'autoReply', 'plots']
/** 类别名→预设数量上限（防 localStorage 膨胀） */
const CONFIG_PRESETS_CAP = 50

const configPresetsCodec = makeCodec<ConfigPreset[]>(
  'configPresets',
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid config presets')
    return (raw as unknown[]).filter(
      (x): x is ConfigPreset =>
        !!x &&
        typeof x === 'object' &&
        typeof (x as { id?: unknown }).id === 'string' &&
        typeof (x as { name?: unknown }).name === 'string' &&
        typeof (x as { createdAt?: unknown }).createdAt === 'number' &&
        CONFIG_PRESET_CATEGORIES.includes((x as { category?: unknown }).category as PresetCategory) &&
        (x as { data?: unknown }).data != null,
    )
  },
)

export function loadConfigPresets(): ConfigPreset[] {
  return loadValue(CONFIG_PRESETS_KEY, configPresetsCodec, [])
}

export function saveConfigPresets(list: ConfigPreset[]): void {
  saveStored(CONFIG_PRESETS_KEY, configPresetsCodec.schema, list)
}

export function removeConfigPresetById(list: ConfigPreset[], pid: string): ConfigPreset[] {
  return list.filter((p) => p.id !== pid)
}

/** 保存命名预设到库（data 形状由调用方保证），同类超过上限丢弃最旧 */
export function insertConfigPreset(
  list: ConfigPreset[],
  category: PresetCategory,
  name: string,
  data: unknown,
): ConfigPreset[] {
  const next: ConfigPreset = {
    id: newPresetId('cp'),
    name: name.trim() || t('logic.preset.defaultName', { category }),
    category,
    createdAt: Date.now(),
    data,
  }
  const merged = [next, ...list]
  const counts = new Map<string, number>()
  const out: ConfigPreset[] = []
  for (const p of merged) {
    const n = counts.get(p.category) ?? 0
    if (n >= CONFIG_PRESETS_CAP) continue
    counts.set(p.category, n + 1)
    out.push(p)
  }
  return out
}

/** 导入预设包 JSON（整库合并，id 冲突重生成）；返回 { 合并后列表, 导入条数 } */
export function mergeConfigPresetImport(
  existing: ConfigPreset[],
  raw: unknown,
): { list: ConfigPreset[]; imported: number } {
  const arr = raw && typeof raw === 'object' ? (raw as { presets?: unknown }).presets : null
  if (!Array.isArray(arr)) return { list: existing, imported: 0 }
  let n = 0
  const ids = new Set(existing.map((p) => p.id))
  const merged = [...existing]
  for (const x of arr) {
    if (
      !x ||
      typeof x !== 'object' ||
      typeof (x as ConfigPreset).category !== 'string' ||
      !CONFIG_PRESET_CATEGORIES.includes((x as ConfigPreset).category)
    )
      continue
    const p = x as ConfigPreset
    const id = ids.has(p.id) ? newPresetId('cp') : p.id
    ids.add(id)
    merged.push({
      id,
      name: typeof p.name === 'string' ? p.name : t('logic.preset.defaultName', { category: p.category }),
      category: p.category,
      createdAt: typeof p.createdAt === 'number' ? p.createdAt : Date.now(),
      data: p.data,
    })
    n += 1
  }
  return { list: merged, imported: n }
}

/**
 * 套用预设到会话（原 applyConfigPreset 的按类别落地）：data 来自导入文件时可能
 * 畸形，各分支做最小形状校验后再落地。plots 类别的后端推送由门面负责（仅
 * 非 offline 会话），本函数只改本地状态。返回是否成功套用。
 */
export function applyConfigPresetToSession(s: Session, p: ConfigPreset): boolean {
  if (p.category === 'filters' && Array.isArray(p.data)) {
    s.filters = p.data.map((f) => ({ ...(f as FilterStage), id: newRuleId() }))
  } else if (p.category === 'keywords' && Array.isArray(p.data)) {
    s.keywords = p.data.map((k) => ({ ...(k as Keyword), id: newKeywordId() }))
  } else if (p.category === 'autoReply' && p.data && typeof p.data === 'object') {
    const d = p.data as Partial<typeof s.autoReply>
    s.autoReply = {
      enabled: !!d.enabled,
      rules: Array.isArray(d.rules)
        ? d.rules.map((r) => ({ ...r, id: newRuleId() }))
        : [],
    }
  } else if (p.category === 'plots' && p.data && typeof p.data === 'object') {
    s.plot = { ...DEFAULT_PLOT_CONFIG, ...(p.data as object) } as PlotConfig
  } else {
    return false
  }
  return true
}
