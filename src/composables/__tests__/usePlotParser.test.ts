import { describe, it, expect } from 'vitest'
import { parseHexField, computeChecksum, parseValue, parseFrames, toHex } from '../usePlotParser'
import plotFixture from '../../../testdata/protocol/plot-v1.json'
import type { Dir, LogLine, PlotBytes, PlotChecksum, PlotConfig, PlotEndian } from '../../types'

/**
 * 绘图文法测试（消费共享黄金样本 testdata/protocol/plot-v1.json）：
 * 向量与期望值以 fixture 为准（Rust parse_frames 侧同源消费），本测试断言
 * TS 实现（usePlotParser）与样本一致——实现漂移即红，防跨语言语义漂移。
 */

interface PlotFixture {
  hexFields: { valid: { raw: string; bytes: number[] }[]; noPairs: string[] }
  checksumVectors: Record<'sum' | 'xor' | 'none', { bytes: number[]; expected: number }>
  valueBoundaries: {
    cases: {
      bytes: number[]
      offset: number
      len: number
      endian: string
      signed: boolean
      expected: number
    }[]
  }
  plotCases: {
    groups: {
      name: string
      config: Record<string, unknown>
      cases: {
        name: string
        lines: { dir: string; text: string; bytes: number[] | null; epochMillis: number }[]
        expected: {
          frameCount: number
          points: { values: number[]; rawHex: string; epochMillis: number }[]
          lastError: string
        }
      }[]
    }[]
  }
}

const fx = plotFixture as unknown as PlotFixture

let lineSeq = 0
function mkLine(dir: Dir, text: string, bytes: number[] | null, epochMillis: number): LogLine {
  lineSeq++
  return { no: lineSeq, ts: '00:00:00.000', dir, text, bytes, epochMillis }
}

function feedLines(lines: PlotFixture['plotCases']['groups'][number]['cases'][number]['lines']): LogLine[] {
  lineSeq = 0
  return lines.map((l) => mkLine(l.dir as Dir, l.text, l.bytes, l.epochMillis))
}

describe('parseHexField（fixture: hexFields）', () => {
  it('合法输入按对抽取、容忍分隔符', () => {
    for (const c of fx.hexFields.valid) {
      expect(parseHexField(c.raw)).toEqual(c.bytes)
    }
  })
  it('无字节对输入返回空数组', () => {
    for (const raw of fx.hexFields.noPairs) {
      expect(parseHexField(raw)).toEqual([])
    }
  })
})

describe('computeChecksum（fixture: checksumVectors）', () => {
  for (const method of ['sum', 'xor', 'none'] as const) {
    it(`${method} 向量`, () => {
      const v = fx.checksumVectors[method]
      expect(computeChecksum(new Uint8Array(v.bytes), method as PlotChecksum)).toBe(v.expected)
    })
  }
})

describe('parseValue（fixture: valueBoundaries 大小端/有符号边界）', () => {
  it('全部边界向量', () => {
    for (const c of fx.valueBoundaries.cases) {
      expect(
        parseValue(
          new Uint8Array(c.bytes),
          c.offset,
          c.len as PlotBytes,
          c.endian as PlotEndian,
          c.signed,
        ),
      ).toBe(c.expected)
    }
  })
})

describe('parseFrames（fixture: plotCases 黄金帧）', () => {
  for (const group of fx.plotCases.groups) {
    for (const tc of group.cases) {
      it(`${group.name} / ${tc.name}`, () => {
        const r = parseFrames(group.config as unknown as PlotConfig, feedLines(tc.lines))
        expect(r.frameCount).toBe(tc.expected.frameCount)
        expect(r.lastError).toBe(tc.expected.lastError)
        expect(r.points).toHaveLength(tc.expected.points.length)
        tc.expected.points.forEach((p, i) => {
          const got = r.points[i]!
          expect(got.values).toEqual(p.values)
          expect(got.rawHex).toBe(p.rawHex)
          expect(got.epochMillis).toBe(p.epochMillis)
        })
      })
    }
  }
})

describe('toHex', () => {
  it('renders uppercase space-separated', () => {
    expect(toHex(new Uint8Array([0xaa, 0x05]))).toBe('AA 05')
    expect(toHex(new Uint8Array([]))).toBe('')
  })
})
