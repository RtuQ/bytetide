import { describe, it, expect, beforeEach, beforeAll, vi } from 'vitest'
import {
  usePanelState,
  loadCenterSplit,
  saveCenterSplit,
  loadDockPrefs,
  saveDockPrefs,
  clampSplit,
  clampDockHeight,
  _resetForTest,
} from '../useLayoutPrefs'

// node 环境无 localStorage：注入内存 stub
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
  _resetForTest()
})

describe('clampSplit / clampDockHeight 纯函数', () => {
  it('分屏比钳制到 20–80 且取整', () => {
    expect(clampSplit(10)).toBe(20)
    expect(clampSplit(90)).toBe(80)
    expect(clampSplit(55.6)).toBe(56)
    expect(clampSplit(NaN)).toBe(55)
  })

  it('dock 高度：下限 120，上限为视口 55%', () => {
    expect(clampDockHeight(50, 900)).toBe(120)
    expect(clampDockHeight(180, 900)).toBe(180)
    expect(clampDockHeight(600, 900)).toBe(495)
    // 极小视口：上限不会低于下限
    expect(clampDockHeight(180, 100)).toBe(120)
  })
})

describe('面板开合持久化', () => {
  it('默认全收起；开合回写 localStorage', () => {
    const p = usePanelState()
    expect(p.isOpen('search')).toBe(false)
    p.setOpen('search', true)
    expect(p.isOpen('search')).toBe(true)
    expect(JSON.parse(mem['serialtool.panels'])).toEqual({ search: true })
    p.setOpen('search', false)
    expect(p.isOpen('search')).toBe(false)
  })

  it('坏 JSON 容错回默认', () => {
    mem['serialtool.panels'] = '{broken'
    const p = usePanelState()
    expect(p.isOpen('any')).toBe(false)
  })

  it('跨实例共享同一状态（模块级单例）', () => {
    usePanelState().setOpen('keywords', true)
    expect(usePanelState().isOpen('keywords')).toBe(true)
  })
})

describe('分屏比 / dock 状态持久化', () => {
  it('split 存取往返并钳制', () => {
    expect(loadCenterSplit()).toBe(55) // 无记录用默认
    saveCenterSplit(70)
    expect(loadCenterSplit()).toBe(70)
    saveCenterSplit(500)
    expect(loadCenterSplit()).toBe(80)
    mem['serialtool.centerSplit'] = 'not-json'
    expect(loadCenterSplit()).toBe(55)
  })

  it('dock 存取往返：高度按视口钳制、tab 白名单、collapsed 仅接受 true', () => {
    saveDockPrefs({ height: 200, collapsed: true, tab: 'alerts' })
    expect(loadDockPrefs(900)).toEqual({ height: 200, collapsed: true, tab: 'alerts' })
    // 视口变小则高度被压回上限
    expect(loadDockPrefs(300).height).toBe(165)
    // 坏数据全回默认
    mem['serialtool.dock'] = '[1,2]'
    expect(loadDockPrefs(900)).toEqual({ height: 180, collapsed: false, tab: 'decode' })
    // 非法 tab 回 decode
    mem['serialtool.dock'] = JSON.stringify({ height: 150, tab: 'hax' })
    expect(loadDockPrefs(900).tab).toBe('decode')
  })
})
