import { describe, it, expect } from 'vitest'
import {
  PULL_FLOOR_MS,
  PULL_CEIL_MS,
  PULL_GRACE_TICKS,
  initialPacing,
  nextPullInterval,
} from '../pullPacing'

const DATA = { got: 3, capped: false }
const EMPTY = { got: 0, capped: false }
const CAPPED = { got: 120_000, capped: true }

describe('nextPullInterval（自适应拉取控制律）', () => {
  it('新条目起始为天花板：空闲节奏与旧固定 200ms 持平（零回归）', () => {
    const st = initialPacing()
    expect(st.intervalMs).toBe(PULL_CEIL_MS)
    expect(st.idleStreak).toBe(0)
  })

  it('拉到新行立即贴地板并清零空拉连击（爬升中突来数据一步回地板）', () => {
    const st = initialPacing()
    expect(nextPullInterval(st, DATA)).toBe(PULL_FLOOR_MS)
    expect(st.intervalMs).toBe(PULL_FLOOR_MS)
    expect(st.idleStreak).toBe(0)

    const st2 = initialPacing()
    for (let i = 0; i < 10; i++) nextPullInterval(st2, EMPTY)
    expect(st2.intervalMs).toBe(PULL_CEIL_MS)
    expect(nextPullInterval(st2, DATA)).toBe(PULL_FLOOR_MS)
  })

  it('翻页拉满仍落后（capped）维持地板不退缩', () => {
    const st = { intervalMs: PULL_FLOOR_MS, idleStreak: 0 }
    expect(nextPullInterval(st, CAPPED)).toBe(PULL_FLOOR_MS)
    expect(st.idleStreak).toBe(0)
  })

  it('宽限期内空拍维持当前间隔：容忍地板采样下的行间空隙', () => {
    const st = { intervalMs: PULL_FLOOR_MS, idleStreak: 0 }
    for (let i = 1; i <= PULL_GRACE_TICKS; i++) {
      expect(nextPullInterval(st, EMPTY)).toBe(PULL_FLOOR_MS)
      expect(st.idleStreak).toBe(i)
    }
  })

  it('宽限期后乘性爬升并钳制在天花板：25→45→81→146→200', () => {
    const st = { intervalMs: PULL_FLOOR_MS, idleStreak: PULL_GRACE_TICKS }
    const seq: number[] = []
    for (let i = 0; i < 8; i++) seq.push(nextPullInterval(st, EMPTY))
    expect(seq).toEqual([
      45,
      81,
      146,
      PULL_CEIL_MS,
      PULL_CEIL_MS,
      PULL_CEIL_MS,
      PULL_CEIL_MS,
      PULL_CEIL_MS,
    ])
  })

  it('无数据的新会话从天花板空拍不会误贴地板', () => {
    const st = initialPacing()
    for (let i = 0; i < 10; i++) nextPullInterval(st, EMPTY)
    expect(st.intervalMs).toBe(PULL_CEIL_MS)
  })

  it('突发-间隙交替流：宽限内空拍不横跳，数据拍即回地板', () => {
    const st = initialPacing()
    nextPullInterval(st, DATA)
    for (let i = 0; i < PULL_GRACE_TICKS; i++) nextPullInterval(st, EMPTY)
    expect(st.intervalMs).toBe(PULL_FLOOR_MS)
    nextPullInterval(st, DATA)
    expect(st.intervalMs).toBe(PULL_FLOOR_MS)
  })
})
