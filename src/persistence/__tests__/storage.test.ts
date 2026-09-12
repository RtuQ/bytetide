import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setStorageBackend, loadStored, loadValue, saveStored, removeStored } from '../storage'
import { makeCodec } from '../schema'
import type { StorageLike } from '../storage'

/**
 * 版本化持久化存储单测（plan Task 7 Step 1 的七种场景 + 边界）：
 * 缺失 / 裸旧格式迁移 / v1 信封 / 未来版本 / 烂 JSON / quota / invalid 备份。
 * localStorage 经 setStorageBackend 注入内存实现（生产默认 globalThis.localStorage）。
 */

function makeMemStorage(): StorageLike & { dump: () => Record<string, string> } {
  const m = new Map<string, string>()
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
    dump: () => Object.fromEntries(m),
  }
}

/** 造一个「能通过」的 codec：数组按字符串白名单过滤（非法项剔除由 parse 内决定） */
function listCodec(schema = 'test.list') {
  return makeCodec<string[]>(schema, (raw) => {
    if (!Array.isArray(raw)) throw new Error('not an array')
    return raw.filter((x): x is string => typeof x === 'string')
  })
}

const TS = 1727000000000
let store: ReturnType<typeof makeMemStorage>

beforeEach(() => {
  store = makeMemStorage()
  setStorageBackend(store)
  vi.spyOn(Date, 'now').mockReturnValue(TS)
})

afterEach(() => {
  setStorageBackend(undefined)
  vi.restoreAllMocks()
})

describe('loadStored：缺失（不写盘）', () => {
  it('键不存在 → missing，且不在存储里落下信封', () => {
    const r = loadStored('k.missing', listCodec(), [])
    expect(r).toEqual({ kind: 'missing' })
    expect(Object.keys(store.dump())).toHaveLength(0)
  })

  it('无后端（setStorageBackend(null)）→ missing / save 失败不 throw', () => {
    setStorageBackend(null)
    expect(loadStored('k.missing', listCodec(), [])).toEqual({ kind: 'missing' })
    expect(saveStored('k.missing', 'test.list', [])).toEqual({
      ok: false,
      error: 'localStorage unavailable',
    })
  })
})

describe('loadStored：裸旧格式（无信封）→ migrated 并回写信封', () => {
  it('裸 JSON 数组 → migrated + 数据经 codec.parse', () => {
    store.setItem('k.legacy', JSON.stringify(['a', 'b', 3, null]))
    const r = loadStored('k.legacy', listCodec(), [])
    expect(r.kind).toBe('migrated')
    if (r.kind === 'migrated') expect(r.data).toEqual(['a', 'b'])
    // 回写信封：{schema, version:1, data}
    const env = JSON.parse(store.dump()['k.legacy']!)
    expect(env).toEqual({ schema: 'test.list', version: 1, data: ['a', 'b'] })
  })

  it('非 JSON 的旧标量（如 theme 存的裸 dark）→ migrated', () => {
    const codec = makeCodec<string>('theme', (raw) => {
      if (raw === 'dark' || raw === 'light') return raw
      throw new Error('bad theme')
    })
    store.setItem('k.theme', 'dark')
    const r = loadStored('k.theme', codec, 'light')
    expect(r.kind).toBe('migrated')
    if (r.kind === 'migrated') expect(r.data).toBe('dark')
    expect(JSON.parse(store.dump()['k.theme']!)).toEqual({
      schema: 'theme',
      version: 1,
      data: 'dark',
    })
  })

  it('回写使用 codec.serialize 归一化', () => {
    const codec = makeCodec<{ v: number }>(
      'test.ser',
      (raw) => {
        if (!raw || typeof raw !== 'object') throw new Error('bad')
        return { v: (raw as { v?: unknown }).v === 1 ? 1 : 0 }
      },
      (data) => ({ v: data.v, norm: true }),
    )
    store.setItem('k.ser', JSON.stringify({ v: 1 }))
    const r = loadStored('k.ser', codec, { v: 0 })
    expect(r.kind).toBe('migrated')
    expect(JSON.parse(store.dump()['k.ser']!)).toEqual({
      schema: 'test.ser',
      version: 1,
      data: { v: 1, norm: true },
    })
  })

  it('回写遇 quota 失败仍返回 migrated 数据（不 throw）', () => {
    store.setItem('k.quota', JSON.stringify(['x']))
    vi.spyOn(store, 'setItem').mockImplementation(() => {
      const e = new Error('quota')
      e.name = 'QuotaExceededError'
      throw e
    })
    const r = loadStored('k.quota', listCodec(), [])
    expect(r).toEqual({ kind: 'migrated', data: ['x'] })
  })
})

describe('loadStored：v1 信封 → ok（不回写）', () => {
  it('schema 匹配 + version 1 → ok，存储原值不动', () => {
    const raw = JSON.stringify({ schema: 'test.list', version: 1, data: ['a'] })
    store.setItem('k.env', raw)
    const r = loadStored('k.env', listCodec(), [])
    expect(r).toEqual({ kind: 'ok', data: ['a'] })
    expect(store.dump()['k.env']).toBe(raw)
  })

  it('信封 data 原样透传 codec.parse（过滤脏项仍算 ok）', () => {
    store.setItem('k.env2', JSON.stringify({ schema: 'test.list', version: 1, data: ['a', 1] }))
    expect(loadStored('k.env2', listCodec(), [])).toEqual({ kind: 'ok', data: ['a'] })
  })
})

describe('loadStored：未来版本 / schema 不匹配 → invalid（不覆盖）', () => {
  it('version 2（未来版本）→ invalid，原值与键都保持原样（无备份）', () => {
    const raw = JSON.stringify({ schema: 'test.list', version: 2, data: ['future'] })
    store.setItem('k.future', raw)
    const r = loadStored('k.future', listCodec(), [])
    expect(r).toEqual({ kind: 'invalid' })
    expect(store.dump()['k.future']).toBe(raw)
    expect(Object.keys(store.dump())).toEqual(['k.future'])
  })

  it('信封 schema 与 codec 不匹配 → invalid，原值不动', () => {
    const raw = JSON.stringify({ schema: 'other.schema', version: 1, data: [] })
    store.setItem('k.schema', raw)
    expect(loadStored('k.schema', listCodec(), [])).toEqual({ kind: 'invalid' })
    expect(store.dump()['k.schema']).toBe(raw)
  })
})

describe('loadStored：烂 JSON / parse 失败 → invalid（先备份再重置）', () => {
  it('烂 JSON：备份到 `${key}.invalid.<ts>` 后重置原键，backupKey 返回', () => {
    store.setItem('k.broken', '{broken')
    const r = loadStored('k.broken', listCodec(), [])
    expect(r).toEqual({ kind: 'invalid', backupKey: `k.broken.invalid.${TS}` })
    const dump = store.dump()
    expect(dump[`k.broken.invalid.${TS}`]).toBe('{broken')
    expect(dump['k.broken']).toBeUndefined()
  })

  it('信封 v1 但 data 让 codec.parse 抛错 → 同样备份 + 重置', () => {
    const raw = JSON.stringify({ schema: 'test.list', version: 1, data: 'not-array' })
    store.setItem('k.baddata', raw)
    const r = loadStored('k.baddata', listCodec(), [])
    expect(r.kind).toBe('invalid')
    if (r.kind === 'invalid') expect(r.backupKey).toBe(`k.baddata.invalid.${TS}`)
    expect(store.dump()[`k.baddata.invalid.${TS}`]).toBe(raw)
    expect(store.dump()['k.baddata']).toBeUndefined()
  })

  it('备份写失败（quota）仍重置原键，结果无 backupKey', () => {
    store.setItem('k.bkfail', '!!!')
    vi.spyOn(store, 'setItem').mockImplementation(() => {
      throw new Error('quota')
    })
    const r = loadStored('k.bkfail', listCodec(), [])
    expect(r).toEqual({ kind: 'invalid' })
    expect(store.dump()['k.bkfail']).toBeUndefined()
  })
})

describe('saveStored', () => {
  it('写 v1 信封：{schema, version:1, data}', () => {
    expect(saveStored('k.save', 'test.list', ['a'])).toEqual({ ok: true })
    expect(JSON.parse(store.dump()['k.save']!)).toEqual({
      schema: 'test.list',
      version: 1,
      data: ['a'],
    })
  })

  it('quota（QuotaExceededError）→ 记失败不 throw', () => {
    vi.spyOn(store, 'setItem').mockImplementation(() => {
      const e = new Error('quota')
      e.name = 'QuotaExceededError'
      throw e
    })
    const r = saveStored('k.quota', 'test.list', [])
    expect(r.ok).toBe(false)
    expect(typeof r.error).toBe('string')
  })

  it('非 Error 抛出物也归一为字符串错误', () => {
    vi.spyOn(store, 'setItem').mockImplementation(() => {
      throw 'boom'
    })
    expect(saveStored('k.quota2', 'test.list', []).ok).toBe(false)
  })
})

describe('loadValue 便捷层 / removeStored', () => {
  it('ok/migrated → 数据；missing/invalid → fallback', () => {
    expect(loadValue('k.none', listCodec(), ['d'])).toEqual(['d'])
    store.setItem('k.bad', 'zzz')
    expect(loadValue('k.bad', listCodec(), ['d'])).toEqual(['d'])
    store.setItem('k.good', JSON.stringify(['g']))
    expect(loadValue('k.good', listCodec(), ['d'])).toEqual(['g'])
  })

  it('removeStored 清键；无后端静默', () => {
    store.setItem('k.rm', 'x')
    removeStored('k.rm')
    expect(store.dump()['k.rm']).toBeUndefined()
    setStorageBackend(null)
    expect(() => removeStored('k.rm2')).not.toThrow()
  })
})
