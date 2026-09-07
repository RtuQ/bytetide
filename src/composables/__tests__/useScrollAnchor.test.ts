import { describe, it, expect } from 'vitest'
import { anchoredTop } from '../useScrollAnchor'

// 视口锚定补偿（plan-buffer-logging-v1 §2.3 + 方案 B）：
// 新 scrollTop = clamp(oldTop - evictedRows*itemSize + prependedRows*itemSize, 0, scrollHeight-clientHeight)
describe('anchoredTop 视口锚定补偿', () => {
  it('正常补偿：上移量 = 被裁行数 × 行高', () => {
    // 旧 scrollTop 1000，裁掉 10 行 × 22px = 220，新几何下回补到 780
    expect(anchoredTop(1000, 10, 0, 22, 5000, 500)).toBe(780)
  })

  it('下钳 0：补偿后为负归 0（正在读的行本身已被淘汰，缓冲头）', () => {
    expect(anchoredTop(100, 50, 0, 22, 5000, 500)).toBe(0)
    expect(anchoredTop(0, 3, 0, 22, 5000, 500)).toBe(0)
  })

  it('上钳 scrollHeight - clientHeight（双向钳制防浏览器越界）', () => {
    // 旧几何 scrollTop 5000 已超新几何最大值 2000-500=1500，钳到 1500
    expect(anchoredTop(5000, 10, 0, 22, 2000, 500)).toBe(1500)
  })

  it('头部回补（方案 B）：下移量 = 回补行数 × 行高，阅读位置钉死', () => {
    // 上滑回补 20 行插到头部，内容下推 440px，scrollTop 等量增加抵消
    expect(anchoredTop(880, 0, 20, 22, 9000, 500)).toBe(1320)
  })

  it('裁剪与回补同批发生：净变化 = (回补 - 被裁) × 行高', () => {
    // 同 tick 回补 30 行、被裁 10 行：净下推 20 行 = 440px
    expect(anchoredTop(1000, 10, 30, 22, 9000, 500)).toBe(1440)
  })

  it('回补下移也要上钳（回补超出新几何最大可滚范围）', () => {
    expect(anchoredTop(100, 0, 100, 22, 2000, 500)).toBe(1500)
  })
})
