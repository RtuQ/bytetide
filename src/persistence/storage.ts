import { SCHEMA_VERSION, envelope, isEnvelope, type Codec } from './schema'
import { migrateEnvelope, migrateLegacy, isKnownSchema } from './migrations'

/**
 * 版本化 localStorage 存取（plan Stage 2 Task 7）：读四态 LoadResult + 信封写。
 *
 * - missing：键缺失（或无可用后端）→ 调用方用 fallback，**不写盘**
 * - migrated：裸旧格式经 migration 链 + codec.parse 成功 → 此时才回写信封
 *   （write envelopes only after a successful read/migration）
 * - ok：v1 信封且 schema 匹配
 * - invalid：烂 JSON / parse 失败 → 先备份到 `${key}.invalid.<timestamp>` 再重置；
 *   未来版本（version > 当前）/ schema 不匹配 → invalid 但**不覆盖不备份**
 *   （保留现场，降级版本或其他 schema 的数据不能被悄悄清掉）
 *
 * localStorage 访问可注入：测试 setStorageBackend(内存实现)，生产默认
 * globalThis.localStorage（惰性解析，node 无后端时安全降级为 missing）。
 */

export interface StorageLike {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

export type LoadResult<T> =
  | { kind: 'ok'; data: T }
  | { kind: 'missing' }
  | { kind: 'migrated'; data: T }
  | { kind: 'invalid'; backupKey?: string }

export interface SaveResult {
  ok: boolean
  error?: string
}

// undefined = 未注入，跟随 globalThis.localStorage；null = 显式禁用（测试用）。
let backendOverride: StorageLike | null | undefined = undefined

/** 注入存储后端；传 undefined 恢复默认（globalThis.localStorage）。 */
export function setStorageBackend(s: StorageLike | null | undefined): void {
  backendOverride = s
}

export function resolveStorage(): StorageLike | null {
  if (backendOverride !== undefined) return backendOverride
  try {
    const g = globalThis as { localStorage?: StorageLike }
    return g.localStorage ?? null
  } catch {
    return null
  }
}

/** invalid 收尾：尽力备份原值 → 重置原键。备份失败也照样重置（不让烂数据滞留）。 */
function invalidate(key: string, storage: StorageLike, raw: string): LoadResult<never> {
  const backupKey = `${key}.invalid.${Date.now()}`
  let backedUp = false
  try {
    storage.setItem(backupKey, raw)
    backedUp = true
  } catch {
    /* 备份失败不阻塞重置 */
  }
  try {
    storage.removeItem(key)
  } catch {
    /* ignore */
  }
  return backedUp ? { kind: 'invalid', backupKey } : { kind: 'invalid' }
}

export function loadStored<T>(key: string, codec: Codec<T>, _fallback: T): LoadResult<T> {
  const storage = resolveStorage()
  if (!storage) return { kind: 'missing' }
  let raw: string | null
  try {
    raw = storage.getItem(key)
  } catch {
    return { kind: 'missing' }
  }
  if (raw === null) return { kind: 'missing' }

  let parsed: unknown
  let jsonOk = true
  try {
    parsed = JSON.parse(raw)
  } catch {
    jsonOk = false
  }

  // ---- 信封路径 ----
  if (jsonOk && isEnvelope(parsed)) {
    if (parsed.schema !== codec.schema) return { kind: 'invalid' }
    if (parsed.version > SCHEMA_VERSION) return { kind: 'invalid' } // 未来版本：不动现场
    const needsUpgrade = parsed.version < SCHEMA_VERSION
    if (needsUpgrade) {
      const m = migrateEnvelope(codec.schema, parsed.version, parsed.data)
      if (!m.ok) return invalidate(key, storage, raw)
      let value: T
      try {
        value = codec.parse(m.data)
      } catch {
        return invalidate(key, storage, raw)
      }
      writeBack(key, codec, value, storage)
      return { kind: 'migrated', data: value }
    }
    try {
      return { kind: 'ok', data: codec.parse(parsed.data) }
    } catch {
      return invalidate(key, storage, raw)
    }
  }

  // ---- legacy 裸数据路径（合法 JSON 的任意值，或非 JSON 的旧标量原文） ----
  // 未注册的 schema（测试/新键遗漏注册）不做链迁移，直接交 codec.parse 校验——
  // 链的职责是已知 schema 的形状升级，codec.parse 才是最终的类型守门。
  const candidate = jsonOk ? parsed : raw
  const m = isKnownSchema(codec.schema)
    ? migrateLegacy(codec.schema, candidate)
    : ({ ok: true, version: SCHEMA_VERSION, data: candidate } as const)
  if (!m.ok) return invalidate(key, storage, raw)
  let value: T
  try {
    value = codec.parse(m.data)
  } catch {
    return invalidate(key, storage, raw)
  }
  writeBack(key, codec, value, storage)
  return { kind: 'migrated', data: value }
}

/** 迁移回写（信封只在新值成功产出后落盘）；quota 等失败不影响读取结果。 */
function writeBack<T>(key: string, codec: Codec<T>, value: T, storage: StorageLike): void {
  try {
    storage.setItem(key, JSON.stringify(envelope(codec.schema, codec.serialize(value))))
  } catch {
    /* 回写失败静默：本次读到的是对的数据，下次启动重走迁移 */
  }
}

/** 便捷层：ok/migrated → data；missing/invalid → fallback。 */
export function loadValue<T>(key: string, codec: Codec<T>, fallback: T): T {
  const r = loadStored(key, codec, fallback)
  return r.kind === 'ok' || r.kind === 'migrated' ? r.data : fallback
}

export function saveStored<T>(key: string, schema: string, data: T): SaveResult {
  const storage = resolveStorage()
  if (!storage) return { ok: false, error: 'localStorage unavailable' }
  try {
    storage.setItem(key, JSON.stringify(envelope(schema, data)))
    return { ok: true }
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) }
  }
}

export function removeStored(key: string): void {
  const storage = resolveStorage()
  if (!storage) return
  try {
    storage.removeItem(key)
  } catch {
    /* ignore */
  }
}
