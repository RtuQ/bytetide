import { describe, it, expect } from 'vitest'
import { createFramerState, framerFeed, normalizeFramingParts } from '../framer'
import { computeCrc, crcSum8, crcXor8, crc16Generic, crc32 } from '../crc'
import type { NormalizedFraming } from '../../types/parser'

// ---- 工具 ----

const bytes = (arr: number[]) => new Uint8Array(arr)
/** 字符串 → UTF-8 字节（展开字符串进 Uint8Array 会把字符转成 NaN→0，必须走 TextEncoder） */
const strBytes = (s: string) => new TextEncoder().encode(s)

/** 温控示例协议：sync AA 55，类型@2，长度域 u8@3（= payload+2），payload，CRC16-modbus(tail:2, little) */
function mkTc(type: number, payload: number[], badCrc = false): number[] {
  const body = [0xaa, 0x55, type, payload.length + 2, ...payload]
  let crc = computeCrc('crc16-modbus', bytes(body))
  if (badCrc) crc = (crc + 1) & 0xffff
  return [...body, crc & 0xff, (crc >> 8) & 0xff]
}

function mkFraming(over: {
  sync?: string | null
  length: NormalizedFraming['length']
  crc?: NormalizedFraming['crc']
  maxSize?: number
  source?: 'binary' | 'ascii-hex'
}): NormalizedFraming {
  const syncRaw = over.sync === null ? '' : (over.sync ?? 'AA 55')
  return {
    source: over.source ?? 'binary',
    sync: new Uint8Array(syncRaw ? [0xaa, 0x55] : []),
    length: over.length,
    crc: over.crc ?? null,
    maxSize: over.maxSize ?? 4096,
  }
}

const TC_FRAMING = mkFraming({
  length: { kind: 'field', at: 3, fmt: 'u8', endian: 'little', add: 4 },
  crc: { algo: 'crc16-modbus', n: 2, endian: 'little' },
  maxSize: 64,
})

function feedAll(cfg: NormalizedFraming, chunks: (number[] | Uint8Array)[]) {
  const st = createFramerState()
  const out = { frames: [] as ReturnType<typeof framerFeed>['frames'], warnings: 0 }
  for (const c of chunks) {
    const r = framerFeed(st, cfg, c instanceof Uint8Array ? c : bytes(c))
    out.frames.push(...r.frames)
    out.warnings += r.warnings
  }
  return { ...out, state: st }
}

// ---- CRC 算法集（reveng catalog "123456789" 校验值为外部真值） ----

describe('CRC 算法集（reveng catalog check 值）', () => {
  const V = bytes([0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39]) // "123456789"

  it('sum8 = 0xDD / xor8 = 0x31', () => {
    expect(crcSum8(V)).toBe(0xdd)
    expect(crcXor8(V)).toBe(0x31)
  })
  it('crc16-modbus = 0x4B37', () => {
    expect(crc16Generic(V, 0x8005, 0xffff, true, true, 0x0000)).toBe(0x4b37)
  })
  it('crc16-ccitt-false = 0x29B1', () => {
    expect(crc16Generic(V, 0x1021, 0xffff, false, false, 0x0000)).toBe(0x29b1)
  })
  it('crc16-xmodem = 0x31C3', () => {
    expect(crc16Generic(V, 0x1021, 0x0000, false, false, 0x0000)).toBe(0x31c3)
  })
  it('crc16-kermit = 0x2189', () => {
    expect(crc16Generic(V, 0x1021, 0x0000, true, true, 0x0000)).toBe(0x2189)
  })
  it('crc32 = 0xCBF43926', () => {
    expect(crc32(V)).toBe(0xcbf43926)
  })
  it('computeCrc 分发覆盖 7 种算法', () => {
    expect(computeCrc('sum8', V)).toBe(0xdd)
    expect(computeCrc('xor8', V)).toBe(0x31)
    expect(computeCrc('crc16-modbus', V)).toBe(0x4b37)
    expect(computeCrc('crc16-ccitt-false', V)).toBe(0x29b1)
    expect(computeCrc('crc16-xmodem', V)).toBe(0x31c3)
    expect(computeCrc('crc16-kermit', V)).toBe(0x2189)
    expect(computeCrc('crc32', V)).toBe(0xcbf43926)
  })
})

// ---- 切帧：sync + 长度域 + CRC（温控协议形态） ----

describe('framer：sync + 长度域 + CRC', () => {
  it('单帧一次吐出，crcOk=true', () => {
    const f = mkTc(0x01, [0x00, 0xfa, 87])
    const r = feedAll(TC_FRAMING, [f])
    expect(r.frames).toHaveLength(1)
    expect(r.frames[0]!.crcOk).toBe(true)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(f)
    expect(r.warnings).toBe(0)
  })

  it('粘包：一个 chunk 两帧', () => {
    const a = mkTc(0x01, [0x00, 0xfa])
    const b = mkTc(0x02, [0x02, 0x02, 0x00])
    const r = feedAll(TC_FRAMING, [[...a, ...b]])
    expect(r.frames).toHaveLength(2)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(a)
    expect(Array.from(r.frames[1]!.bytes)).toEqual(b)
  })

  it('断帧/跨行帧：分多次 feed 逐步到齐', () => {
    const f = mkTc(0x01, [0x00, 0xfa, 87])
    const r = feedAll(TC_FRAMING, [f.slice(0, 3), f.slice(3, 6), f.slice(6)])
    expect(r.frames).toHaveLength(1)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(f)
  })

  it('垃圾前缀被丢弃，帧照常解析', () => {
    const f = mkTc(0x81, [])
    const r = feedAll(TC_FRAMING, [[0x00, 0x99, 0x12], f])
    expect(r.frames).toHaveLength(1)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(f)
  })

  it('sync 跨批截断：前一批只到 AA，下一批补 55 后成帧', () => {
    const f = mkTc(0x01, [0x01, 0x00])
    const r = feedAll(TC_FRAMING, [[0xaa], f])
    expect(r.frames).toHaveLength(1)
    // 前导的孤立 AA 已作为 sync 前缀被消费，不产生重复帧
    expect(Array.from(r.frames[0]!.bytes)).toEqual(f)
  })

  it('坏长度值（超限）有 sync：滑窗重同步，后续好帧正常，无脏帧', () => {
    const bad = [0xaa, 0x55, 0x01, 0xff, 0x11, 0x22, 0x33] // 长度域 0xFF+4 > maxSize 64
    const good = mkTc(0x02, [0x01, 0x00, 0x00])
    const r = feedAll(TC_FRAMING, [[...bad, ...good]])
    expect(r.frames).toHaveLength(1)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(good)
    expect(r.frames[0]!.crcOk).toBe(true)
  })

  it('CRC 失败帧有 sync：帧以 crcOk=false 吐出（计入警告不进翻译）且不脏流', () => {
    const bad = mkTc(0x01, [0x00, 0xfa], true)
    const good = mkTc(0x02, [0x02, 0x02, 0x00])
    const r = feedAll(TC_FRAMING, [[...bad, ...good]])
    expect(r.frames).toHaveLength(2)
    expect(r.frames[0]!.crcOk).toBe(false)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(bad)
    expect(r.frames[1]!.crcOk).toBe(true)
    expect(Array.from(r.frames[1]!.bytes)).toEqual(good)
  })
})

// ---- 切帧：无 sync ----

describe('framer：无 sync', () => {
  it('定长无 sync：按 N 字节顺序切', () => {
    const cfg = mkFraming({ sync: null, length: { kind: 'fixed', value: 4 } })
    const r = feedAll(cfg, [[1, 2, 3, 4, 5, 6, 7, 8, 9]])
    expect(r.frames.map((f) => Array.from(f.bytes))).toEqual([
      [1, 2, 3, 4],
      [5, 6, 7, 8],
    ])
    expect(r.state.buf.length).toBe(1) // 尾部不足一帧的等待
  })

  it('定长 + sync：value 含 sync 在内的总帧长', () => {
    const cfg = mkFraming({ length: { kind: 'fixed', value: 5 } })
    const r = feedAll(cfg, [[0xaa, 0x55, 1, 2, 3, 0xaa, 0x55, 4, 5, 6]])
    expect(r.frames).toHaveLength(2)
    expect(Array.from(r.frames[1]!.bytes)).toEqual([0xaa, 0x55, 4, 5, 6])
  })

  it('长度域大小端：u16@0 大端 vs 小端', () => {
    const big = mkFraming({
      sync: null,
      length: { kind: 'field', at: 0, fmt: 'u16', endian: 'big', add: 2 },
    })
    const rBig = feedAll(big, [[0x00, 0x02, 0xaa, 0x55]])
    expect(Array.from(rBig.frames[0]!.bytes)).toEqual([0x00, 0x02, 0xaa, 0x55])
    const little = mkFraming({
      sync: null,
      length: { kind: 'field', at: 0, fmt: 'u16', endian: 'little', add: 2 },
    })
    const rLittle = feedAll(little, [[0x02, 0x00, 0xaa, 0x55]])
    expect(Array.from(rLittle.frames[0]!.bytes)).toEqual([0x02, 0x00, 0xaa, 0x55])
  })

  it('长度域坏值无 sync：复位缓冲计警告（坏长度下无法可靠重同步）', () => {
    const cfg = mkFraming({
      sync: null,
      length: { kind: 'field', at: 0, fmt: 'u8', endian: 'little', add: 2 },
      maxSize: 16,
    })
    const r = feedAll(cfg, [[0xff, 0x01, 0x02, 0x03]])
    expect(r.frames).toHaveLength(0)
    expect(r.warnings).toBe(1)
    expect(r.state.buf.length).toBe(0)
  })

  it('CRC 失败帧无 sync：消费整帧继续（帧边界自洽）', () => {
    const cfg = mkFraming({
      sync: null,
      length: { kind: 'field', at: 0, fmt: 'u8', endian: 'little', add: 1 },
      crc: { algo: 'sum8', n: 1, endian: 'little' },
    })
    // 帧 = [len, payload..., sum]；len=2 → 总长 3；覆盖范围 = 去掉 CRC 自身 = [0x02, 0x10]
    const good = [0x02, 0x10, 0x12]
    const bad = [0x02, 0x7f, 0x00] // 错误校验（sum(0x02,0x7f)=0x81 ≠ 0x00）
    const r = feedAll(cfg, [[...bad, ...good]])
    expect(r.frames).toHaveLength(2)
    expect(r.frames[0]!.crcOk).toBe(false)
    expect(Array.from(r.frames[0]!.bytes)).toEqual(bad)
    expect(r.frames[1]!.crcOk).toBe(true)
    expect(Array.from(r.frames[1]!.bytes)).toEqual(good)
  })
})

// ---- 切帧：until / line ----

describe('framer：until / line', () => {
  it('until：以 0D 0A 结尾切帧，含分隔符本身', () => {
    const cfg = mkFraming({ sync: null, length: { kind: 'until', tail: new Uint8Array([0x0d, 0x0a]) } })
    const r = feedAll(cfg, [[0xaa, 0x01, 0x0d, 0x0a, 0xbb, 0x02, 0x0d, 0x0a]])
    expect(r.frames.map((f) => Array.from(f.bytes))).toEqual([
      [0xaa, 0x01, 0x0d, 0x0a],
      [0xbb, 0x02, 0x0d, 0x0a],
    ])
  })

  it('line：整行一帧，容忍 CRLF（帧不含行尾）', () => {
    const cfg = mkFraming({ sync: null, length: { kind: 'line' } })
    const r = feedAll(cfg, [strBytes('TEMP=25.5\r\n'), strBytes('OK\n')])
    expect(Array.from(r.frames[0]!.bytes)).toEqual(Array.from(strBytes('TEMP=25.5')))
    expect(Array.from(r.frames[1]!.bytes)).toEqual(Array.from(strBytes('OK')))
  })

  it('line + CRC：tail:N 对行内容（不含行尾）', () => {
    const cfg = mkFraming({
      sync: null,
      length: { kind: 'line' },
      crc: { algo: 'xor8', n: 1, endian: 'little' },
    })
    const line = strBytes('AB') // xor('A','B') = 0x03
    const r = feedAll(cfg, [new Uint8Array([...line, 0x03, 0x0a])])
    expect(r.frames).toHaveLength(1)
    expect(r.frames[0]!.crcOk).toBe(true)
  })

  it('until 超限（maxSize 内无分隔符）：复位缓冲计警告', () => {
    const cfg = mkFraming({
      sync: null,
      length: { kind: 'until', tail: new Uint8Array([0x0d, 0x0a]) },
      maxSize: 8,
    })
    const r = feedAll(cfg, [[1, 2, 3, 4, 5, 6, 7, 8, 9]])
    expect(r.frames).toHaveLength(0)
    expect(r.warnings).toBe(1)
  })
})

// ---- 隔离与归一化 ----

describe('framer：状态隔离与 normalizeFramingParts', () => {
  it('两个状态互不干扰（(session,dir) 隔离的根基）', () => {
    const stA = createFramerState()
    const stB = createFramerState()
    const f1 = mkTc(0x01, [0x00, 0xfa])
    const f2 = mkTc(0x02, [0x02, 0x02])
    const r1 = framerFeed(stA, TC_FRAMING, bytes(f1.slice(0, 4)))
    const r2 = framerFeed(stB, TC_FRAMING, bytes(f2))
    expect(r1.frames).toHaveLength(0)
    expect(r2.frames).toHaveLength(1)
    const r1b = framerFeed(stA, TC_FRAMING, bytes(f1.slice(4)))
    expect(r1b.frames).toHaveLength(1)
  })

  it('normalizeFramingParts：hex 串转字节 / crc tail:N / 缺省值', () => {
    const n = normalizeFramingParts({
      source: 'binary',
      sync: 'AA 55',
      length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
      crc: { algo: 'crc16-ccitt-false', at: 'tail:2', endian: 'big' },
      maxSize: 512,
    })
    expect(Array.from(n.sync)).toEqual([0xaa, 0x55])
    expect(n.crc).toEqual({ algo: 'crc16-ccitt-false', n: 2, endian: 'big' })
    expect(n.maxSize).toBe(512)
    const dflt = normalizeFramingParts({
      source: 'ascii-hex',
      length: { kind: 'line' },
    })
    expect(dflt.sync.length).toBe(0)
    expect(dflt.crc).toBeNull()
    expect(dflt.maxSize).toBe(4096)
  })
})
