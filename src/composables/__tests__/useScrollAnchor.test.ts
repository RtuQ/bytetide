import { describe, it, expect } from 'vitest'
import { anchoredTop } from '../useScrollAnchor'

// 视口锚定补偿（plan-buffer-logging-v1 §2.3）：裁剪 evictedRows 行后，
// 新 scrollTop = clamp(oldTop - evictedRows*itemSize, 0, scrollHeight-clientHeight)
describe('anchoredTop 视口锚定补偿', () => {
  it('正常补偿：上移量 = 被裁行数 × 行高', () => {
    // 旧 scrollTop 1000，裁掉 10 行 × 22px = 220，新几何下回补到 780
    expect(anchoredTop(1000, 10, 22, 5000, 500)).toBe(780)
  })

  it('下钳 0：补偿后为负归 0（正在读的行本身已被淘汰，缓冲头）', () => {
    expect(anchoredTop(100, 50, 22, 5000, 500)).toBe(0)
    expect(anchoredTop(0, 3, 22, 5000, 500)).toBe(0)
  })

  it('上钳 scrollHeight - clientHeight（双向钳制防浏览器越界）', () => {
    // 旧几何 scrollTop 5000 已超新几何最大值 2000-500=1500，钳到 1500
    expect(anchoredTop(5000, 10, 22, 2000, 500)).toBe(1500)
  })
})
