import { reactive } from 'vue'

/** 布局类 UI 偏好（localStorage 持久化）：侧栏面板开合 / 日志↔图表分屏高度比 / 底部 dock 状态。
 *  纯函数（钳制/解析）与副作用分离，vitest node 环境注入 localStorage stub 即可直测。 */

export type DockTab = 'decode' | 'alerts' | 'monitor'
export interface DockPrefs {
  height: number
  collapsed: boolean
  tab: DockTab
}

const PANELS_KEY = 'serialtool.panels'
const SPLIT_KEY = 'serialtool.centerSplit'
const DOCK_KEY = 'serialtool.dock'

/** 分屏高度比（日志占比，百分比）钳制范围 */
export const SPLIT_MIN = 20
export const SPLIT_MAX = 80
export const SPLIT_DEFAULT = 55
/** dock 展开高度下限（px）；上限按视口 55% 动态钳制 */
export const DOCK_MIN = 120
export const DOCK_DEFAULT = 180

export function clampSplit(pct: number): number {
  if (!Number.isFinite(pct)) return SPLIT_DEFAULT
  return Math.min(SPLIT_MAX, Math.max(SPLIT_MIN, Math.round(pct)))
}

export function clampDockHeight(h: number, viewportH: number): number {
  const max = Math.max(DOCK_MIN, Math.floor(viewportH * 0.55))
  if (!Number.isFinite(h)) return DOCK_DEFAULT
  return Math.min(max, Math.max(DOCK_MIN, Math.round(h)))
}

function readJSON(key: string): unknown {
  try {
    const raw = localStorage.getItem(key)
    return raw ? JSON.parse(raw) : null
  } catch {
    return null
  }
}
function writeJSON(key: string, v: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(v))
  } catch {
    /* ignore */
  }
}

// ---- 面板开合：模块级单例（App.vue 多个 <details> 共享），默认全收起 ----
const panels = reactive<Record<string, boolean>>({})
let panelsLoaded = false
function ensurePanelsLoaded() {
  if (panelsLoaded) return
  panelsLoaded = true
  const v = readJSON(PANELS_KEY)
  if (v && typeof v === 'object') {
    for (const [k, open] of Object.entries(v as Record<string, unknown>)) {
      if (open === true) panels[k] = true
    }
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
      writeJSON(PANELS_KEY, { ...panels })
    },
  }
}

// ---- 分屏高度比（日志区占比 %） ----
export function loadCenterSplit(): number {
  const v = readJSON(SPLIT_KEY)
  return clampSplit(typeof v === 'number' ? v : SPLIT_DEFAULT)
}
export function saveCenterSplit(pct: number) {
  writeJSON(SPLIT_KEY, clampSplit(pct))
}

// ---- 底部 dock ----
export function loadDockPrefs(viewportH: number): DockPrefs {
  const v = readJSON(DOCK_KEY)
  const o = (v && typeof v === 'object' ? v : {}) as Partial<DockPrefs>
  return {
    height: clampDockHeight(
      typeof o.height === 'number' ? o.height : DOCK_DEFAULT,
      viewportH,
    ),
    collapsed: o.collapsed === true,
    tab: o.tab === 'alerts' || o.tab === 'monitor' ? o.tab : 'decode',
  }
}
export function saveDockPrefs(p: DockPrefs) {
  writeJSON(DOCK_KEY, p)
}

/** 测试辅助：重置模块级面板单例 */
export function _resetForTest() {
  panelsLoaded = false
  for (const k of Object.keys(panels)) delete panels[k]
}
