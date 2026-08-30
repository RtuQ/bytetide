import { describe, it, expect } from 'vitest'
import { parseHexField, computeChecksum, parseValue, parseFrames, toHex } from '../usePlotParser'
import type { Dir, LogLine, PlotConfig } from '../../types'

function mkLine(no: number, dir: Dir, text: string, bytes: number[] | null, epochMillis: number): LogLine {
  return { no, ts: '00:00:00.000', dir, text, bytes, epochMillis }
}

function mkPlot(over: Partial<PlotConfig> = {}): PlotConfig {
  return {
    enabled: false,
    source: 'binary',
    frameHead: '',
    frameTail: '',
    checksum: 'none',
    channels: 2,
    bytesPerChannel: 2,
    endian: 'big',
    signed: false,
    maxPoints: 2000,
    ...over,
  }
}

describe('parseHexField', () => {
  it('extracts hex pairs ignoring separators', () => {
    expect(parseHexField('01 00')).toEqual([0x01, 0x00])
    expect(parseHexField('0100')).toEqual([0x01, 0x00])
    expect(parseHexField('01,00')).toEqual([0x01, 0x00])
  })
  it('returns empty when no pairs match', () => {
    expect(parseHexField('')).toEqual([])
    expect(parseHexField('zz')).toEqual([])
  })
})

describe('computeChecksum', () => {
  it('sum takes the low 8 bits', () => {
    expect(computeChecksum(new Uint8Array([0x01, 0x02, 0xff]), 'sum')).toBe(2) // 258 & 0xff
  })
  it('xor folds all bytes', () => {
    expect(computeChecksum(new Uint8Array([0xaa, 0x55]), 'xor')).toBe(0xff)
  })
  it('none returns 0', () => {
    expect(computeChecksum(new Uint8Array([0x01]), 'none')).toBe(0)
  })
})

describe('parseValue', () => {
  it('big/little endian 2 bytes', () => {
    expect(parseValue(new Uint8Array([0x01, 0x00]), 0, 2, 'big', false)).toBe(256)
    expect(parseValue(new Uint8Array([0x01, 0x00]), 0, 2, 'little', false)).toBe(1)
  })
  it('signed 1-byte boundary (-128 / 127)', () => {
    expect(parseValue(new Uint8Array([0x80]), 0, 1, 'big', true)).toBe(-128)
    expect(parseValue(new Uint8Array([0x7f]), 0, 1, 'big', true)).toBe(127)
  })
  it('signed 2-byte boundary (-32768)', () => {
    expect(parseValue(new Uint8Array([0x80, 0x00]), 0, 2, 'big', true)).toBe(-32768)
  })
  it('4-byte without 32-bit truncation', () => {
    expect(parseValue(new Uint8Array([0x00, 0x00, 0x01, 0x00]), 0, 4, 'big', false)).toBe(256)
  })
})

describe('parseFrames', () => {
  it('golden 1: binary head AA55, 2ch x 2B big unsigned -> [256,512]', () => {
    const cfg = mkPlot({ frameHead: 'AA55', channels: 2, bytesPerChannel: 2, endian: 'big' })
    const r = parseFrames(cfg, [mkLine(1, 'rx', '', [0xaa, 0x55, 0x01, 0x00, 0x02, 0x00], 1000)])
    expect(r.frameCount).toBe(1)
    expect(r.points[0].values).toEqual([256, 512])
    expect(r.points[0].rawHex).toBe('AA 55 01 00 02 00')
    expect(r.lastError).toBe('')
  })

  it('golden 2: binary head FF, 1ch x 1B signed + sum -> [-128]; bad cs -> 0 frames', () => {
    const cfg = mkPlot({
      frameHead: 'FF',
      checksum: 'sum',
      channels: 1,
      bytesPerChannel: 1,
      endian: 'big',
      signed: true,
    })
    const ok = parseFrames(cfg, [mkLine(1, 'rx', '', [0xff, 0x80, 0x80], 1000)])
    expect(ok.frameCount).toBe(1)
    expect(ok.points[0].values).toEqual([-128])
    expect(ok.points[0].rawHex).toBe('FF 80 80')
    // 坏校验：前端静默跳过（不像后端会记 lastError），结果 0 帧
    const bad = parseFrames(cfg, [mkLine(2, 'rx', '', [0xff, 0x80, 0x00], 2000)])
    expect(bad.frameCount).toBe(0)
    expect(bad.points.length).toBe(0)
  })

  it('golden 3: ascii-hex tail 0D, 2ch x 1B little -> [170,85],[187,102]', () => {
    const cfg = mkPlot({
      source: 'ascii-hex',
      frameTail: '0D',
      channels: 2,
      bytesPerChannel: 1,
      endian: 'little',
    })
    const r = parseFrames(cfg, [mkLine(7, 'rx', 'AA550DBB660D', null, 3000)])
    expect(r.frameCount).toBe(2)
    expect(r.points[0].values).toEqual([170, 85])
    expect(r.points[0].rawHex).toBe('AA 55 0D')
    expect(r.points[1].values).toEqual([187, 102])
    expect(r.points[1].rawHex).toBe('BB 66 0D')
  })

  it('head mismatch advances one byte', () => {
    const cfg = mkPlot({ frameHead: 'AA55', channels: 2, bytesPerChannel: 2, endian: 'big' })
    const r = parseFrames(cfg, [mkLine(1, 'rx', '', [0x00, 0xaa, 0x55, 0x01, 0x00, 0x02, 0x00], 1000)])
    expect(r.frameCount).toBe(1)
    expect(r.points[0].rawHex).toBe('AA 55 01 00 02 00')
  })

  it('maxPoints keeps the last N points (frameCount counts all)', () => {
    const cfg = mkPlot({ frameHead: 'AA', channels: 1, bytesPerChannel: 1, endian: 'big', maxPoints: 2 })
    const r = parseFrames(cfg, [mkLine(1, 'rx', '', [0xaa, 0x01, 0xaa, 0x02, 0xaa, 0x03], 0)])
    expect(r.frameCount).toBe(3)
    expect(r.points.length).toBe(2)
    expect(r.points[0].values).toEqual([2])
    expect(r.points[1].values).toEqual([3])
  })
})

describe('toHex', () => {
  it('renders uppercase space-separated', () => {
    expect(toHex(new Uint8Array([0xaa, 0x05]))).toBe('AA 05')
    expect(toHex(new Uint8Array([]))).toBe('')
  })
})
