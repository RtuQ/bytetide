import { describe, it, expect } from 'vitest'
import { computeGaps } from '../useLineStats'
import type { LogLine } from '../../types'

function mkLine(dir: 'rx' | 'tx', epoch: number): LogLine {
  return { no: 0, ts: '', dir, text: 'x', bytes: null, epochMillis: epoch }
}

describe('computeGaps 尾部窗口', () => {
  it('统计仅覆盖最近 5000 行', () => {
    // 6000 行 RX 间隔 1ms，末尾 3 行间隔 100ms。
    // 尾部 5000 行窗口：窗内 5000 个 RX，首行不与窗外行配对 -> 4999 对间隔
    const lines: LogLine[] = []
    let e = 0
    for (let i = 0; i < 6000; i++) lines.push(mkLine('rx', (e += 1)))
    for (let i = 0; i < 3; i++) lines.push(mkLine('rx', (e += 100)))
    const { stats, samples } = computeGaps(lines)
    expect(stats.count).toBe(4999)
    expect(stats.min).toBe(1)
    expect(stats.max).toBe(100)
    expect(samples.length).toBeLessThanOrEqual(200)
  })

  it('TX 行不参与间隔，RX 链跨 TX 延续', () => {
    const lines = [
      mkLine('rx', 1000),
      mkLine('tx', 2000),
      mkLine('rx', 2500),
      mkLine('rx', 2600),
    ]
    const { stats } = computeGaps(lines)
    // 仅 RX 口径：rx(1000)->rx(2500)->rx(2600)，TX 只是被跳过
    expect(stats.count).toBe(2)
    expect(stats.avg).toBe(800)
    expect(stats.max).toBe(1500)
  })

  it('空输入与乱序 epoch 返回零统计', () => {
    expect(computeGaps([]).stats.count).toBe(0)
    const onlyTx = [mkLine('tx', 1), mkLine('tx', 2)]
    expect(computeGaps(onlyTx).stats.count).toBe(0)
    const backwards = [mkLine('rx', 3000), mkLine('rx', 1000)]
    expect(computeGaps(backwards).stats.count).toBe(0)
  })
})
