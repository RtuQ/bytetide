/**
 * 定高虚拟滚动的纯窗口计算（LogScroller 的算法层，无 DOM/Vue 依赖，可单测）。
 *
 * 与 vue-virtual-scroller 的关键差异：渲染窗口按行 key（行 no）锚定，
 * 追加/头部裁剪/回补只移动既有节点的 top，不销毁重建，浏览器文本选区
 * 锚定的 DOM 节点得以存活（选区漂移问题的治本点）。
 */

export interface WinRange {
  /** 渲染窗口起点下标（含），0 ≤ start ≤ end ≤ total */
  start: number
  /** 渲染窗口终点下标（不含） */
  end: number
}

/** 按滚动位置计算渲染窗口：可见区 ± bufferRows 行缓冲。 */
export function computeWindow(
  scrollTop: number,
  viewportH: number,
  total: number,
  itemSize: number,
  bufferRows: number,
): WinRange {
  if (total <= 0) return { start: 0, end: 0 }
  const first = Math.floor(Math.max(scrollTop, 0) / itemSize) - bufferRows
  const start = Math.min(Math.max(first, 0), total)
  const last = Math.ceil((Math.max(scrollTop, 0) + Math.max(viewportH, 0)) / itemSize) + bufferRows
  const end = Math.min(Math.max(last, start + 1), total)
  return { start, end }
}

/**
 * 在按 key 升序排列的数组中二分找第一个 keyOf(item) ≥ key 的下标
 * （找不到即全部 < key 时返回 arr.length）。key 必须同类型可比（日志行 no 为数字）。
 */
export function lowerBoundKey<T>(arr: T[], key: string | number, keyOf: (it: T) => unknown): number {
  let lo = 0
  let hi = arr.length
  while (lo < hi) {
    const mid = (lo + hi) >>> 1
    if ((keyOf(arr[mid]!) as string | number) < key) lo = mid + 1
    else hi = mid
  }
  return lo
}

/**
 * items 数组变化后重锚窗口起点：求「旧窗口起点行」在新数组中的下标。
 * 键序单调时二分命中 → 返回精确新下标（追加/回补/裁剪都只平移，不改窗口内容身份）；
 * 锚行被裁掉或键失配时回退「长度差推算」（尾部追加为 0 偏移、头部裁剪为负偏移），并夹紧到界内。
 */
export function reanchorStart<T>(
  oldItems: T[],
  newItems: T[],
  oldStart: number,
  keyOf: (it: T) => unknown,
): number {
  const anchor = oldItems[oldStart]
  if (anchor != null) {
    const k = keyOf(anchor) as string | number
    const i = lowerBoundKey(newItems, k, keyOf)
    if (i < newItems.length && (keyOf(newItems[i]!) as string | number) === k) return i
  }
  const est = oldStart + (newItems.length - oldItems.length)
  return Math.min(Math.max(est, 0), newItems.length)
}
