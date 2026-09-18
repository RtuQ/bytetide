import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { setStorageBackend, type StorageLike } from '../../persistence/storage'
import { zhCN } from '../messages/zh-CN'
import { en } from '../messages/en'
import { locale, t, tDynamic, setLocale, detectSystemLocale, _resetLocaleForTest } from '../index'

function memStorage(): StorageLike & { dump: () => Map<string, string> } {
  const m = new Map<string, string>()
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
    dump: () => m,
  }
}

let store: ReturnType<typeof memStorage>

beforeEach(() => {
  store = memStorage()
  setStorageBackend(store)
  _resetLocaleForTest('zh-CN')
})
afterEach(() => setStorageBackend(undefined))

describe('词典键集一致性', () => {
  it('en 键集与 zh 完全一致（无缺无多余）', () => {
    const zhKeys = Object.keys(zhCN)
    const enKeys = Object.keys(en)
    expect(enKeys.filter((k) => !(k in zhCN))).toEqual([])
    expect(zhKeys.filter((k) => !(k in en))).toEqual([])
  })

  it('两侧词条均非空白', () => {
    for (const k of Object.keys(zhCN)) expect((en as Record<string, string>)[k].trim()).not.toBe('')
    for (const v of Object.values(en)) expect(v.trim()).not.toBe('')
  })
})

describe('detectSystemLocale', () => {
  it('en* 命中即英文，其余中文，按偏好顺序取首个命中', () => {
    expect(detectSystemLocale(['en-US'])).toBe('en')
    expect(detectSystemLocale(['EN'])).toBe('en')
    expect(detectSystemLocale(['zh-CN', 'en-GB'])).toBe('zh-CN')
    expect(detectSystemLocale(['fr', 'en-GB'])).toBe('en')
    expect(detectSystemLocale([])).toBe('zh-CN')
  })
})

describe('t 与语言切换', () => {
  it('默认中文词条', () => {
    expect(locale.value).toBe('zh-CN')
    expect(t('app.title')).toBe('ByteTide · 字节潮')
  })

  it('setLocale 后词条切换且即时生效', () => {
    setLocale('en')
    expect(locale.value).toBe('en')
    expect(t('app.title')).toBe('ByteTide')
  })

  it('{name} 插值：命中替换、未命中保留占位', () => {
    expect(t('app.selftest.interp', { v: 42 })).toBe('值=42')
    expect(t('app.selftest.interp')).toBe('值={v}')
    setLocale('en')
    expect(t('app.selftest.interp', { v: 'x' })).toBe('value=x')
  })
})

describe('持久化', () => {
  it('setLocale 写 v1 信封（schema locale）', () => {
    setLocale('en')
    const raw = store.dump().get('serialtool.locale')
    expect(raw).toBeDefined()
    expect(JSON.parse(raw as string)).toEqual({ schema: 'locale', version: 1, data: 'en' })
  })

  it('_resetLocaleForTest() 清持久化后回系统判定', () => {
    setLocale('en')
    _resetLocaleForTest('zh-CN')
    expect(locale.value).toBe('zh-CN')
    expect(store.dump().has('serialtool.locale')).toBe(true) // setLocale 仍落盘
  })
})

describe('tDynamic 动态键', () => {
  it('命中词条翻译，未命中回退 fallback', () => {
    expect(tDynamic('errors.generic', 'fallback')).toBe('操作失败')
    expect(tDynamic('errors.not_a_code', 'fallback')).toBe('fallback')
    setLocale('en')
    expect(tDynamic('errors.generic', 'fallback')).toBe('Operation failed')
  })
})
