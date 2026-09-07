/**
 * 视口锚定补偿（plan-buffer-logging-v1 §2.3 + 方案 B）：取消跟随阅读时，头部
 * 行集几何变化会让视口内容跟着滑走——头部被裁（滑动窗口淘汰）使内容收缩、
 * 浏览器钳制 scrollTop、视口整体上移；头部回补（上滑翻页补旧行）使内容增长、
 * 视口内容相对下推。在 DOM 更新前记录旧 scrollTop（watcher 默认 pre-flush，
 * DOM 还是旧几何），渲染后按「被裁行数 − 回补行数（仅计通过过滤链、真正渲染
 * 占高的）」等量补偿并双向钳制，保证阅读位置不动。
 * 钳到 0 = 正在读的行本身已被淘汰（缓冲头），大缓冲下极罕见。
 */
export function anchoredTop(
  oldTop: number,
  evictedRows: number,
  prependedRows: number,
  itemSize: number,
  scrollHeight: number,
  clientHeight: number,
): number {
  const max = Math.max(0, scrollHeight - clientHeight)
  const want = oldTop - evictedRows * itemSize + prependedRows * itemSize
  return Math.min(Math.max(want, 0), max)
}
