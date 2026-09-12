import { reactive } from 'vue'
import { makeCodec } from '../persistence/schema'
import { loadValue, saveStored } from '../persistence/storage'

/** 布局类 UI 偏好（localStorage 持久化）：侧栏面板开合 / 日志↔图表分屏高度比 /
 *  底部 dock 状态 / 侧栏宽度收起。纯函数（钳制/解析）与副作用分离，存储统一走
 *  src/persistence（v1 信封 + 旧裸 JSON 自动迁移），vitest node 环境注入
 *  localStorage stub 或 setStorageBackend 即可直测。 */

export type DockTab = 'decode' | 'alerts' | 'monitor'
export interface DockPrefs {
  height: number
  collapsed: boolean
  tab: DockTab
}

const PANELS_KEY = 'serialtool.panels'
const SPLIT_KEY = 'serialtool.centerSplit'
const DOCK_KEY = 'serialtool.dock'
const SIDEBAR_KEY = 'serialtool.sidebar'

/** 分屏高度比（日志占比，百分比）钳制范围 */
export const SPLIT_MIN = 20
export const SPLIT_MAX = 80
export const SPLIT_DEFAULT = 55
/** dock 展开高度下限（px）；上限按视口 55% 动态钳制 */
export const DOCK_MIN = 120
export const DOCK_DEFAULT = 180
/** 侧栏宽度钳制范围（px） */
export const SIDEBAR_MIN = 240
export const SIDEBAR_MAX = 560
export const SIDEBAR_DEFAULT = 312

export function clampSplit(pct: number): number {
  if (!Number.isFinite(pct)) return SPLIT_DEFAULT
  return Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, Math.round(pct)))
}

export function clampDockHeight(h: number, viewportH: number): number {
  const max = Math.max(DOCK_MIN, Math.floor(viewportH * 0.55))
  if (!Number.isFinite(h)) return DOCK_DEFAULT
  return Math.min(max, Math.max(DOCK_MIN, Math.round(h)))
}

// ---- 信封 codecs（schema 名 = 键去掉 serialtool. 前缀，与 migrations 注册表一致） ----

const panelsCodec = makeCodec<Record<string, boolean>>(
  'panels',
  (raw) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid panels')
    const out: Record<string, boolean> = {}
    for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
      if (typeof v === 'boolean') out[k] = v
    }
    return out
  },
)

const splitCodec = makeCodec<number>('centerSplit', (raw) => {
  if (typeof raw !== 'number' || !Number.isFinite(raw)) throw new Error('invalid centerSplit')
  return raw
})

const dockCodec = makeCodec<Partial<DockPrefs>>(
  'dock',
  (raw) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid dock')
    return raw as Partial<DockPrefs> // 字段级校验在 loadDockPrefs（tab 白名单/钳制）
  },
)

export interface SidebarPrefs {
  width: number
  collapsed: boolean
}

const sidebarCodec = makeCodec<SidebarPrefs>(
  'sidebar',
  (raw) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid sidebar')
    const v = raw as { width?: unknown; collapsed?: unknown }
    const w = Number(v.width)
    return {
      width: Number.isFinite(w) && w > 0 ? Math.min(w, SIDEBAR_MAX) : SIDEBAR_DEFAULT,
      collapsed: v.collapsed === true,
    }
  },
  // 只存钳制后的宽度（防越界值长期滞留 localStorage）
  (p) => ({
    width: Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, Math.round(p.width))),
    collapsed: p.collapsed,
  }),
)

// ---- 面板开合：模块级单例（App.vue 多个 <details> 共享），默认全收起 ----
const panels = reactive<Record<string, boolean>>({})
let panelsLoaded = false
function ensurePanelsLoaded() {
  if (panelsLoaded) return
  panelsLoaded = true
  const v = loadValue(PANELS_KEY, panelsCodec, {})
  for (const [k, open] of Object.entries(v)) {
    if (open === true) panels[k] = true
  }
}

export function usePanelState() {
  ensurePanelsLoaded()
  return {
    /** 未记录的面板一律视为收起（默认全收起约定不变） */
    isOpen: (id: string): boolean => panels[id] === true,
    setOpen: (id: string, open: boolean) => {
      if (panels[id] === open) return
      panels[id] = open
      saveStored(PANELS_KEY, panelsCodec.schema, { ...panels })
    },
  }
}

// ---- 分屏高度比（日志区占比 %） ----
export function loadCenterSplit(): number {
  const v = loadValue(SPLIT_KEY, splitCodec, SPLIT_DEFAULT)
  return clampSplit(typeof v === 'number' ? v : SPLIT_DEFAULT)
}
export function saveCenterSplit(pct: number) {
  saveStored(SPLIT_KEY, splitCodec.schema, clampSplit(pct))
}

// ---- 底部 dock ----
export function loadDockPrefs(viewportH: number): DockPrefs {
  const o = loadValue(DOCK_KEY, dockCodec, {} as Partial<DockPrefs>)
  return {
    height: clampDockHeight(
      typeof o.height === 'number' ? o.height : DOCK_DEFAULT,
      viewportH,
    ),
    // 首次打开时优先把空 Dock 收起，给主工作区让出空间；显式 false 仍尊重用户偏好。
    collapsed: o.collapsed !== false,
    tab: o.tab === 'alerts' || o.tab === 'monitor' ? o.tab : 'decode',
  }
}
export function saveDockPrefs(p: DockPrefs) {
  saveStored(DOCK_KEY, dockCodec.schema, p)
}

// ---- 侧栏（宽度 + 收起；App.vue 调用，持久化逻辑收拢在本模块） ----
export function loadSidebarPrefs(): SidebarPrefs {
  return loadValue(SIDEBAR_KEY, sidebarCodec, { width: SIDEBAR_DEFAULT, collapsed: false })
}
export function saveSidebarPrefs(p: SidebarPrefs) {
  saveStored(SIDEBAR_KEY, sidebarCodec.schema, p)
}

/** 测试辅助：重置模块级面板单例 */
export function _resetForTest() {
  panelsLoaded = false
  for (const k of Object.keys(panels)) delete panels[k]
}
