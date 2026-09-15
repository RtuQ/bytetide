import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest'
import { useNotificationPrefs, _resetNotificationPrefsForTest } from '../useNotificationPrefs'

// node 环境无 localStorage：注入内存 stub（同 useLayoutPrefs 测试模式）
const mem: Record<string, string> = {}
beforeAll(() => {
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => mem[k] ?? null,
    setItem: (k: string, v: string) => {
      mem[k] = v
    },
    removeItem: (k: string) => {
      delete mem[k]
    },
  })
})

beforeEach(() => {
  for (const k of Object.keys(mem)) delete mem[k]
  _resetNotificationPrefsForTest()
})

describe('useNotificationPrefs', () => {
  it('默认开；写关回写 v1 信封', () => {
    const n = useNotificationPrefs()
    expect(n.prefs.enabled).toBe(true)
    n.setEnabled(false)
    expect(n.prefs.enabled).toBe(false)
    expect(JSON.parse(mem['serialtool.notifications'] as string)).toEqual({
      schema: 'notifications',
      version: 1,
      data: { enabled: false },
    })
  })

  it('从 localStorage 恢复上次偏好；再开回写 true', () => {
    mem['serialtool.notifications'] = JSON.stringify({
      schema: 'notifications',
      version: 1,
      data: { enabled: false },
    })
    const n = useNotificationPrefs()
    expect(n.prefs.enabled).toBe(false)
    n.setEnabled(true)
    expect(JSON.parse(mem['serialtool.notifications'] as string).data).toEqual({ enabled: true })
  })

  it('坏 JSON / 非对象值 / 非布尔字段容错回默认开', () => {
    mem['serialtool.notifications'] = '{broken'
    expect(useNotificationPrefs().prefs.enabled).toBe(true)

    _resetNotificationPrefsForTest()
    mem['serialtool.notifications'] = JSON.stringify({ schema: 'notifications', version: 1, data: [1] })
    expect(useNotificationPrefs().prefs.enabled).toBe(true)

    _resetNotificationPrefsForTest()
    mem['serialtool.notifications'] = JSON.stringify({
      schema: 'notifications',
      version: 1,
      data: { enabled: 'yes' },
    })
    expect(useNotificationPrefs().prefs.enabled).toBe(false)
  })

  it('缺失 enabled 字段回落默认开（增量字段向后兼容）', () => {
    mem['serialtool.notifications'] = JSON.stringify({ schema: 'notifications', version: 1, data: {} })
    expect(useNotificationPrefs().prefs.enabled).toBe(true)
  })

  it('跨实例共享同一状态（模块级单例）', () => {
    useNotificationPrefs().setEnabled(false)
    expect(useNotificationPrefs().prefs.enabled).toBe(false)
  })
})
