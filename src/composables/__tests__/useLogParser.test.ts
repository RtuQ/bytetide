import { describe, it, expect } from 'vitest'
import { parseTsToMs, parseLogFile } from '../useLogParser'
import type { RawLogLine } from '../../types'

describe('parseTsToMs', () => {
  it('parses HH:MM:SS.mmm', () => {
    expect(parseTsToMs('14:30:25.123')).toBe(52_225_123)
  })
  it('parses without milliseconds (-> .000)', () => {
    expect(parseTsToMs('14:30:25')).toBe(52_225_000)
  })
  it('pads fractional .5 to 500ms', () => {
    // 1*3600000 + 2*60000 + 3*1000 + 500（分秒须两位，与真实日志格式一致）
    expect(parseTsToMs('01:02:03.5')).toBe(3_723_500)
  })
  it('rejects out-of-range hour', () => {
    expect(parseTsToMs('25:00:00.000')).toBe(-1)
  })
  it('rejects out-of-range minute', () => {
    expect(parseTsToMs('14:60:00.000')).toBe(-1)
  })
  it('rejects garbage / empty', () => {
    expect(parseTsToMs('abc')).toBe(-1)
    expect(parseTsToMs('')).toBe(-1)
  })
})

describe('parseLogFile', () => {
  it('splits only on first two tabs (text may contain tabs)', () => {
    const r = parseLogFile('14:30:25.123\tRX\thello\tworld')
    expect(r.total).toBe(1)
    expect(r.errors).toBe(0)
    expect(r.lines[0]).toEqual({
      ts: '14:30:25.123',
      dir: 'rx',
      text: 'hello\tworld',
      bytes: null,
      epochMillis: 52_225_123,
    } satisfies RawLogLine)
  })

  it('normalizes dir (TX->tx, anything else->rx) and keeps bytes null', () => {
    const content = ['00:00:00.000\ttx\tA', '00:00:00.000\tXYZ\tB', '00:00:00.000\t TX \tC'].join('\n')
    const r = parseLogFile(content)
    expect(r.lines.map((l) => l.dir)).toEqual(['tx', 'rx', 'tx'])
    expect(r.lines.every((l) => l.bytes === null)).toBe(true)
  })

  it('falls back epochMillis to seq when ts unparseable', () => {
    // line0: bad ts -> epoch = seq 0；line1: real ts .005 -> epoch = 5（非 seq 1）
    const r = parseLogFile('badts\tRX\tfoo\n00:00:00.005\tRX\tbar')
    expect(r.lines[0].ts).toBe('badts')
    expect(r.lines[0].epochMillis).toBe(0) // seq 回退
    expect(r.lines[1].epochMillis).toBe(5) // 真实 ms
  })

  it('skips empty lines and counts no-tab lines as errors', () => {
    const r = parseLogFile('00:00:00.000\tRX\ta\n\nnotabline\n00:00:00.000\tRX\tb')
    expect(r.total).toBe(2) // 空行 + 无 tab 行不计入
    expect(r.errors).toBe(1) // 仅 notabline
  })

  it('caps to 50000 keeping the last', () => {
    const one = '00:00:00.000\tRX\tx'
    const r = parseLogFile(Array(50_001).fill(one).join('\n'))
    expect(r.total).toBe(50_001)
    expect(r.lines.length).toBe(50_000)
  })
})
