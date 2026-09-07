/**
 * 视口锚定补偿（plan-buffer-logging-v1 §2.3）：取消跟随阅读时，头部行被滑动
 * 窗口裁剪会让内容高度收缩、浏览器钳制 scrollTop，视口整体上移。
 * 在 DOM 更新前记录旧 scrollTop（watcher 默认 pre-flush，DOM 还是旧几何），
 * 渲染后按被裁行数等量回补并双向钳制，保证阅读位置不动。
 * 钳到 0 = 正在读的行本身已被淘汰（缓冲头），大缓冲下极罕见。
 */
export function anchoredTop(
  oldTop: number,
  evictedRows: number,
  itemSize: number,
  scrollHeight: number,
  clientHeight: number,
): number {
  const max = Math.max(0, scrollHeight - clientHeight)
  const want = oldTop - evictedRows * itemSize
  return Math.min(Math.max(want, 0), max)
}
