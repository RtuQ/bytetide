import { DEFAULT_PLOT_CONFIG } from '../../types'
import type { PlotConfig } from '../../types'
import type { CenterView, Session } from './model'

/**
 * 视图域纯函数（Task 6）：中心区视图耦合不变式 + 分屏/对比布局。
 * 显式传入状态原地变更，不内部调用 useSessionStore；后端推送（_pushPlot）由
 * 门面编排。
 */

// ===================== 会话级视图耦合 =====================

/**
 * 开关绘图（原 setPlotEnabled 主体）：开启时同时强制 HEX 视图（绘图仅支持 hex
 * 格式接收模式），显式开图自动切到图表视图；关闭时不改动 hexView，图表类视图
 * 回落 log。不变式：centerView !== 'log' ⟹ plot.enabled——启用开关始终看得见效果。
 */
export function applySetPlotEnabled(s: Session, v: boolean): void {
  s.plot = { ...s.plot, enabled: v }
  if (v) {
    s.hexView = true
    if (s.centerView === 'log') s.centerView = 'plot'
  } else if (s.centerView !== 'log') {
    s.centerView = 'log'
  }
}

/**
 * 切换中心区视图模式（原 setCenterView 主体）：进入 split/plot 时若图表未启用
 * 则顺带启用（一次点击即出图）。返回是否发生了需要同步后端绘图配置的变更
 * （= 走了 setPlotEnabled 路径）；同视图幂等返回 false。
 */
export function applySetCenterView(s: Session, view: CenterView): boolean {
  if (s.centerView === view) return false
  s.centerView = view
  if (view !== 'log' && !s.plot.enabled) {
    applySetPlotEnabled(s, true)
    return true
  }
  return false
}

/** 更新绘图解析配置（帧头/帧尾/校验/通道等），enabled 经 applySetPlotEnabled 单独控制 */
export function applyUpdatePlot(s: Session, patch: Partial<PlotConfig>): void {
  s.plot = { ...s.plot, ...patch }
}

/** 采纳 REST 桥写回的绘图文法（bridge-annotations-updated 同源通道）：整包替换，
 *  缺省字段用默认值回填；后端 manager 已持有该配置，无需回推 */
export function applyAdoptBridgePlot(s: Session, cfg: PlotConfig): void {
  s.plot = { ...DEFAULT_PLOT_CONFIG, ...cfg }
}

// ===================== 分屏 / 对比（全局布局态） =====================

export interface LayoutState {
  splitMode: boolean
  compareMode: boolean
  columns: (string | null)[]
}

/** 进入分屏：用已打开会话填充前两列（不足则留空），列数 2~4 */
export function enterSplit(layout: LayoutState, order: readonly string[]): void {
  layout.columns = [order[0] ?? null, order[1] ?? null]
  layout.splitMode = true
}

export function exitSplit(layout: LayoutState): void {
  layout.splitMode = false
  layout.columns = []
}

/** 双会话时间对齐对比（与 splitMode 同级占中心区）：会话不足 2 个时拒绝进入
 *  （退出不受限，供关闭会话后的自动退出兜底） */
export function toggleCompareMode(layout: LayoutState, sessionCount: number): void {
  if (!layout.compareMode && sessionCount < 2) return
  layout.compareMode = !layout.compareMode
}

/** 关闭会话后兜底：对比依赖 ≥2 会话，关到只剩一个时自动退出对比态 */
export function autoExitCompareAfterClose(layout: LayoutState, sessionCount: number): void {
  if (layout.compareMode && sessionCount < 2) layout.compareMode = false
}

/** 设置某列绑定的会话；若该会话已在别列，则两列互换（避免同会话出现两次） */
export function setColumnSessionIn(
  layout: LayoutState,
  i: number,
  id: string | null,
): void {
  if (i < 0 || i >= layout.columns.length) return
  const cols = [...layout.columns]
  if (id) {
    const j = cols.findIndex((c, idx) => idx !== i && c === id)
    if (j >= 0) cols[j] = cols[i]
  }
  cols[i] = id
  layout.columns = cols
}

/** 增加一列：优先填充未占用的已打开会话，最多 4 列 */
export function addColumnTo(layout: LayoutState, order: readonly string[]): void {
  if (layout.columns.length >= 4) return
  const used = new Set(layout.columns.filter((c): c is string => !!c))
  const next = order.find((id) => !used.has(id)) ?? null
  layout.columns = [...layout.columns, next]
}

/** 删除一列：至少保留 2 列 */
export function removeColumnFrom(layout: LayoutState, i: number): void {
  if (layout.columns.length <= 2) return
  layout.columns = layout.columns.filter((_, idx) => idx !== i)
}

// ===================== 书签 / 跳转 =====================

/** 切换书签：存在则移除，否则按行号升序插入 */
export function toggleBookmarkIn(s: Session, no: number): void {
  const i = s.bookmarks.indexOf(no)
  if (i >= 0) s.bookmarks.splice(i, 1)
  else {
    let at = s.bookmarks.length
    for (let k = 0; k < s.bookmarks.length; k++) {
      if (s.bookmarks[k]! > no) {
        at = k
        break
      }
    }
    s.bookmarks.splice(at, 0, no)
  }
}

export function removeBookmarkIn(s: Session, no: number): void {
  const i = s.bookmarks.indexOf(no)
  if (i >= 0) s.bookmarks.splice(i, 1)
}

export function requestJumpIn(s: Session, no: number): void {
  s.jump = { no, token: Date.now() }
}
