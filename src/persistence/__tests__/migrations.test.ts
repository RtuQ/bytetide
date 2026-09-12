import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  SCHEMA_VERSION,
  makeCodec,
  envelope,
  isEnvelope,
} from '../schema'
import {
  registerMigrations,
  migrateLegacy,
  migrateEnvelope,
  knownSchemas,
} from '../migrations'
import { setStorageBackend, loadStored } from '../storage'
import type { StorageLike } from '../storage'

/**
 * migration 注册表单测：legacy 裸数据按版本 0 起跑跑链、信封旧版本逐级升级、
 * 未知 schema / 缺步 / 回退步拒绝；生产 schema 全部注册了 0→1 identity 链。
 */

let store: StorageLike
beforeEach(() => {
  const m = new Map<string, string>()
  store = {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
  }
  setStorageBackend(store)
})
afterEach(() => setStorageBackend(undefined))

describe('schema.ts 基元', () => {
  it('SCHEMA_VERSION 冻结为 1（envelope.version 的合同值）', () => {
    expect(SCHEMA_VERSION).toBe(1)
  })
  it('isEnvelope 结构判别：schema:string + version:number + 含 data（未来版本也算信封，交 loadStored 按不覆盖处理）', () => {
    expect(isEnvelope({ schema: 's', version: 1, data: null })).toBe(true)
    expect(isEnvelope({ schema: 's', version: 2, data: null })).toBe(true)
    expect(isEnvelope({ schema: 's', version: 1 })).toBe(false) // 缺 data
    expect(isEnvelope({ version: 1, data: null })).toBe(false) // 缺 schema
    expect(isEnvelope({ schema: 3, version: 1, data: null })).toBe(false)
    expect(isEnvelope(null)).toBe(false)
    expect(isEnvelope('x')).toBe(false)
    expect(isEnvelope([1])).toBe(false)
  })
  it('envelope() 产出的信封满足 StoredEnvelope 形状', () => {
    const e = envelope('s', { a: 1 })
    expect(e).toEqual({ schema: 's', version: 1, data: { a: 1 } })
  })
})

describe('migrateLegacy：裸数据按版本 0 跑链', () => {
  it('生产 schema 的 identity 链：原样返回', () => {
    const data = { logPathTemplate: 'x', lineTsFormat: '', viewBufCap: 5, midnightRotate: true }
    const r = migrateLegacy('logConfig', data)
    expect(r).toEqual({ ok: true, version: SCHEMA_VERSION, data })
  })

  it('注册过的自定义链被应用（0→1 变换）', () => {
    registerMigrations('test.chain', [
      { from: 0, to: 1, migrate: (d) => ({ stepped: true, prev: d }) },
    ])
    const r = migrateLegacy('test.chain', { a: 1 })
    expect(r).toEqual({ ok: true, version: 1, data: { stepped: true, prev: { a: 1 } } })
  })

  it('未知 schema → 拒绝（防拼错 schema 名静默丢数据）', () => {
    expect(migrateLegacy('nope.nope', {})).toMatchObject({ ok: false })
  })

  it('缺步（空链）→ 拒绝；回退步（to <= from）→ 拒绝防死循环', () => {
    registerMigrations('test.gap', [])
    expect(migrateLegacy('test.gap', {})).toMatchObject({ ok: false })
    registerMigrations('test.loop', [{ from: 0, to: 0, migrate: (d) => d }])
    expect(migrateLegacy('test.loop', {})).toMatchObject({ ok: false })
  })
})

describe('migrateEnvelope：旧信封逐级升级', () => {
  it('version === SCHEMA_VERSION → 原样（无需迁移）', () => {
    expect(migrateEnvelope('logConfig', 1, { x: 1 })).toEqual({
      ok: true,
      version: 1,
      data: { x: 1 },
    })
  })

  it('version > SCHEMA_VERSION → 拒绝（未来版本由上层按 invalid 处理）', () => {
    expect(migrateEnvelope('logConfig', 2, {})).toMatchObject({ ok: false })
  })

  it('version 0 的旧信封跑 0→1 链', () => {
    registerMigrations('test.env0', [{ from: 0, to: 1, migrate: () => 'upgraded' }])
    expect(migrateEnvelope('test.env0', 0, 'old')).toEqual({
      ok: true,
      version: 1,
      data: 'upgraded',
    })
  })
})

describe('knownSchemas：生产键覆盖', () => {
  it('包含全部迁移键（AGENTS.md localStorage 键清单 minus 分隔符前缀）', () => {
    const schemas = knownSchemas()
    for (const s of [
      'theme',
      'lastPortConfig',
      'logConfig',
      'searchHistory',
      'portPresets',
      'configPresets',
      'sendPresets',
      'sendSequences',
      'parserScript',
      'alertSound',
      'update.lastCheck',
      'update.dismissedVersion',
      'sidebar',
      'panels',
      'centerSplit',
      'dock',
    ]) {
      expect(schemas).toContain(s)
    }
    // 无重复
    expect(new Set(schemas).size).toBe(schemas.length)
  })

  it('注册表覆盖全部生产 schema 的 legacy 链（identity）', () => {
    for (const s of knownSchemas()) {
      expect(migrateLegacy(s, { probe: true })).toMatchObject({ ok: true })
    }
  })
})

describe('loadStored × migration 链端到端', () => {
  it('legacy 数据经注册链变换后作为 migrated 落盘', () => {
    registerMigrations('test.e2e', [
      { from: 0, to: 1, migrate: (d) => (typeof d === 'number' ? { v: d } : d) },
    ])
    const codec = makeCodec<{ v: number }>('test.e2e', (raw) => {
      if (!raw || typeof raw !== 'object' || typeof (raw as { v?: unknown }).v !== 'number') {
        throw new Error('bad')
      }
      return raw as { v: number }
    })
    store.setItem('k.e2e', '42')
    const r = loadStored('k.e2e', codec, { v: 0 })
    expect(r.kind).toBe('migrated')
    if (r.kind === 'migrated') expect(r.data).toEqual({ v: 42 })
    expect(JSON.parse(store.getItem('k.e2e')!)).toEqual({
      schema: 'test.e2e',
      version: 1,
      data: { v: 42 },
    })
  })

  it('链产出后 codec.parse 仍拒绝 → invalid', () => {
    registerMigrations('test.e2e2', [{ from: 0, to: 1, migrate: () => 'junk' }])
    const codec = makeCodec<{ v: number }>('test.e2e2', () => {
      throw new Error('always')
    })
    store.setItem('k.e2e2', '1')
    expect(loadStored('k.e2e2', codec, { v: 0 }).kind).toBe('invalid')
  })
})
