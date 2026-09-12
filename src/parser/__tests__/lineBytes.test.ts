import { describe, it, expect } from 'vitest'
import { lineBytes } from '../lineBytes'
import plotFixture from '../../../testdata/protocol/plot-v1.json'
import type { LogLine, PlotSource } from '../../types'

/**
 * 行字节三态还原测试（消费共享黄金样本 plot-v1.json 的 lineBytesSamples）：
 * 样本同时被 Rust 侧 line_bytes 的测试消费——两侧任一实现漂移即红。
 */

interface PlotFixture {
  lineBytesSamples: {
    cases: {
      name: string
      source: string
      line: { text: string; bytes: number[] | null }
      expectedBytes: number[]
    }[]
  }
}

const fx = (plotFixture as unknown as PlotFixture).lineBytesSamples

describe('lineBytes 三态还原（fixture: lineBytesSamples）', () => {
  it('全部黄金样本', () => {
    for (const c of fx.cases) {
      const line: LogLine = {
        no: 1,
        ts: '00:00:00.000',
        dir: 'rx',
        text: c.line.text,
        bytes: c.line.bytes,
        epochMillis: 0,
      }
      expect(Array.from(lineBytes(line, c.source as PlotSource)), c.name).toEqual(c.expectedBytes)
    }
  })

  it('返回副本：改写不影响原数组（防外部串改原始字节）', () => {
    const raw = [0x80, 0xc3, 0x28]
    const b = lineBytes(
      { no: 1, ts: '', dir: 'rx', text: '???', bytes: raw, epochMillis: 0 },
      'binary',
    )
    b[0] = 0xff
    expect(raw[0]).toBe(0x80)
  })
})
