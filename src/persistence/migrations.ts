import { SCHEMA_VERSION } from './schema'

/**
 * migration 注册表（plan Stage 2 Task 7）：schema 名 → 逐版本升级函数链。
 *
 * 本阶段全部 schema 从 v1 起步，legacy 裸数据按「版本 0」跑 0→1 identity 链；
 * 链产出后再交给 codec.parse 校验（两层防线：链改形状、parse 定类型）。
 * 未来引入 v2：SCHEMA_VERSION+1，并为需要迁移的 schema 注册 {from:1,to:2} 步。
 */

export interface MigrationStep {
  from: number
  to: number
  migrate: (data: unknown) => unknown
}

export type MigrationOutcome =
  | { ok: true; version: number; data: unknown }
  | { ok: false; reason: string }

/** 全部走信封的 schema 名（= localStorage 键去掉 `serialtool.` 前缀）。 */
const PRODUCTION_SCHEMAS = [
  'theme',
  'lastPortConfig',
  'logConfig',
  'searchHistory',
  'portPresets',
  'sendPresets',
  'sendSequences',
  'configPresets',
  'parserScript',
  'alertSound',
  'update.lastCheck',
  'update.dismissedVersion',
  'sidebar',
  'panels',
  'centerSplit',
  'dock',
] as const

const registry = new Map<string, MigrationStep[]>()

/** 注册（整组替换）某 schema 的升级链；步骤按 from 升序在注册时排序。 */
export function registerMigrations(schema: string, steps: MigrationStep[]): void {
  registry.set(schema, [...steps].sort((a, b) => a.from - b.from))
}

export function knownSchemas(): readonly string[] {
  return PRODUCTION_SCHEMAS
}

/** 是否为注册过的生产 schema（storage 层对未注册 schema 的 legacy 数据走 identity）。 */
export function isKnownSchema(schema: string): boolean {
  return registry.has(schema)
}

// 生产 schema 的起步链：legacy(0) → v1 恒等（数据形状由各 codec.parse 负责）。
for (const s of PRODUCTION_SCHEMAS) {
  registerMigrations(s, [{ from: 0, to: SCHEMA_VERSION, migrate: (d) => d }])
}

function runChain(schema: string, fromVersion: number, data: unknown): MigrationOutcome {
  const steps = registry.get(schema)
  if (!steps) return { ok: false, reason: `unknown schema: ${schema}` }
  let version = fromVersion
  let cur = data
  while (version < SCHEMA_VERSION) {
    const step = steps.find((s) => s.from === version)
    if (!step) return { ok: false, reason: `missing migration ${schema} v${version}` }
    if (step.to <= version) {
      return { ok: false, reason: `regressive migration ${schema} v${version}->v${step.to}` }
    }
    try {
      cur = step.migrate(cur)
    } catch (e) {
      return { ok: false, reason: `migration ${schema} v${version} threw: ${String(e)}` }
    }
    version = step.to
  }
  return { ok: true, version, data: cur }
}

/** legacy 裸数据（无信封）= 版本 0 起跑。 */
export function migrateLegacy(schema: string, data: unknown): MigrationOutcome {
  return runChain(schema, 0, data)
}

/** 信封数据从 version 升到当前版本；version === 当前 → 原样（无需迁移）。
 *  version 大于 SCHEMA_VERSION 返回失败（未来版本），由 storage 层按
 *  「invalid 不覆盖」处理——绝不能把未来版本的数据跑链降级。 */
export function migrateEnvelope(schema: string, version: number, data: unknown): MigrationOutcome {
  if (version === SCHEMA_VERSION) return { ok: true, version, data }
  if (version > SCHEMA_VERSION) {
    return { ok: false, reason: `future envelope version ${version} > ${SCHEMA_VERSION}` }
  }
  return runChain(schema, version, data)
}
