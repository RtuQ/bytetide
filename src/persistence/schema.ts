/**
 * 持久化 schema 基元（plan Stage 2 Task 7）：信封类型 / codec / 版本常量。
 *
 * 所有 localStorage 键的「值」升级为信封 `{ schema, version, data }`（键名不变，
 * AGENTS.md 的键清单是合同）。schema 名取键的 `serialtool.` 后缀（如 'logConfig'），
 * version 从 SCHEMA_VERSION 起步；旧裸 JSON 数据由 storage.loadStored 在成功读取时
 * 经 migration 链迁移并回写信封（write envelopes only after a successful
 * read/migration——缺失/invalid 一律不落新值）。
 */

import { parseScenarioLibraryEntry, type ScenarioLibraryEntry } from '../types/automation'

/** 当前信封版本。未来引入 v2 时：+1，并在 migrations.ts 为每个 schema 注册 1→2 步。 */
export const SCHEMA_VERSION = 1

export interface StoredEnvelope<T> {
  schema: string
  version: 1
  data: T
}

/**
 * 键级编解码器：parse 校验/归一化（非法 throw，loadStored 统一转 invalid），
 * serialize 在迁移回写时归一化负载（默认恒等）。
 */
export interface Codec<T> {
  readonly schema: string
  parse(raw: unknown): T
  serialize(data: T): unknown
}

const identity = <T>(x: T): T => x

export function makeCodec<T>(
  schema: string,
  parse: (raw: unknown) => T,
  serialize: (data: T) => unknown = identity,
): Codec<T> {
  return { schema, parse, serialize }
}

export function envelope<T>(schema: string, data: T): StoredEnvelope<T> {
  return { schema, version: 1, data }
}

/** 结构判别（不做 schema 名匹配——那由 loadStored 按 codec 比较）。
 *  version 放宽为数字：未来版本（>SCHEMA_VERSION）也要能被识别为信封，
 *  才能走「invalid 不覆盖」路径而不是被当 legacy 裸数据备份重置。 */
export function isEnvelope(v: unknown): v is StoredEnvelope<unknown> {
  return (
    typeof v === 'object' &&
    v !== null &&
    !Array.isArray(v) &&
    typeof (v as { schema?: unknown }).schema === 'string' &&
    typeof (v as { version?: unknown }).version === 'number' &&
    'data' in v
  )
}

// ---------------------------------------------------------------------------
// 场景库 codec（Stage 3 Task 4）：localStorage 键 `serialtool.scenarios`，
// 信封 schema 名 `bytetide.scenario-library`（plan 指定名，非「键去前缀」惯例），
// v1 起步。形状校验委托 types/automation.ts 的结构守卫（与 JSON 导入共用）。
// migrations 注册表未含此名——storage 层对未注册 schema 的 legacy 数据走
// identity 链，行为等价（最终类型守门在本 codec.parse）。
// ---------------------------------------------------------------------------

export const SCENARIO_LIBRARY_SCHEMA = 'bytetide.scenario-library'

export const scenarioLibraryCodec: Codec<ScenarioLibraryEntry[]> = makeCodec<ScenarioLibraryEntry[]>(
  SCENARIO_LIBRARY_SCHEMA,
  (raw) => {
    if (!Array.isArray(raw)) throw new Error('invalid scenario library')
    return raw.map(parseScenarioLibraryEntry)
  },
)
