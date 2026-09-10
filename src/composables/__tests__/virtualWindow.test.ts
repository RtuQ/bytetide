import { describe, expect, it } from 'vitest'
import { computeWindow, lowerBoundKey, reanchorStart } from '../virtualWindow'

const keyOf = (n: number) => n

describe('computeWindow', () => {
  it('空列表返回空窗口', () => {
    expect(computeWindow(0, 220, 0, 22, 9)).toEqual({ start: 0, end: 0 })
  })

  it('顶部：可见区加上下缓冲，不越界', () => {
    // scrollTop=0，视口 10 行，缓冲 9 → 上取 0，下取 10+9
    expect(computeWindow(0, 220, 1000, 22, 9)).toEqual({ start: 0, end: 19 })
  })

  it('中部：两侧各留缓冲', () => {
    // scrollTop=1100 → 可见首行 50，缓冲 9 → start=41；可见末行 59，end=59+9+1=69
    expect(computeWindow(1100, 220, 1000, 22, 9)).toEqual({ start: 41, end: 69 })
  })

  it('底部：终点夹到 total', () => {
    // 最大滚动位置 = 1000*22 - 220
    const w = computeWindow(1000 * 22 - 220, 220, 1000, 22, 9)
    expect(w.start).toBe(990 - 9)
    expect(w.end).toBe(1000)
  })

  it('负 scrollTop 视为 0；零高视口仍至少渲染 1 行', () => {
    expect(computeWindow(-50, 220, 1000, 22, 0).start).toBe(0)
    expect(computeWindow(500, 0, 1000, 22, 0).end).toBe(23)
  })
})

describe('lowerBoundKey', () => {
  it('升序二分：命中/越界/空数组', () => {
    expect(lowerBoundKey([10, 20, 30], 20, keyOf)).toBe(1)
    expect(lowerBoundKey([10, 20, 30], 5, keyOf)).toBe(0)
    expect(lowerBoundKey([10, 20, 30], 40, keyOf)).toBe(3)
    expect(lowerBoundKey([], 1, keyOf)).toBe(0)
  })
})

describe('reanchorStart', () => {
  const seq = (from: number, count: number) => Array.from({ length: count }, (_, i) => from + i)

  it('尾部追加：锚行下标不变（选区所在 DOM 不动）', () => {
    const old = seq(0, 100)
    expect(reanchorStart(old, seq(0, 150), 40, keyOf)).toBe(40)
  })

  it('头部回补 K 行：锚行下标平移 +K，窗口内容身份不变', () => {
    const old = seq(100, 100)
    expect(reanchorStart(old, seq(50, 150), 40, keyOf)).toBe(90)
  })

  it('头部裁剪未伤锚行：平移 -K', () => {
    const old = seq(100, 100)
    expect(reanchorStart(old, seq(130, 70), 40, keyOf)).toBe(10)
  })

  it('锚行被裁掉：回退长度差推算并夹紧到 0', () => {
    const old = seq(100, 100)
    // 锚行 no=102 在新数组已不存在（全部 ≥130），回退 40-30=10 —— 也在界内
    expect(reanchorStart(old, seq(130, 70), 2, keyOf)).toBe(0)
  })

  it('锚行不在新数组（尾部收缩/异源替换）：回退长度差推算，负值夹 0', () => {
    const old = seq(0, 100)
    expect(reanchorStart(old, seq(0, 10), 50, keyOf)).toBe(0)
  })

  it('键失配（无序/异源替换）：回退长度差推算', () => {
    // 二分落点 key 不等于锚 key → 走回退分支：3+(5-4)=4
    const old = [5, 3, 9, 1] // 故意无序
    expect(reanchorStart(old, [5, 3, 9, 1, 7], 3, keyOf)).toBe(4)
  })
})
