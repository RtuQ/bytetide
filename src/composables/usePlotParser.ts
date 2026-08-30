import type { LogLine, PlotBytes, PlotChecksum, PlotConfig, PlotEndian, PlotPoint, PlotSource } from '../types'

const textEncoder = new TextEncoder()
const HEX_PAIR = /[0-9a-fA-F]{2}/g

/** 把配置里的 hex 字段（如 "01 00" / "0100" / "01,00"）解析为字节数组 */
export function parseHexField(s: string): number[] {
  const out: number[] = []
  HEX_PAIR.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = HEX_PAIR.exec(s)) !== null) out.push(parseInt(m[0], 16))
  return out
}

/** 取一行 RX 的字节表示。
 *  binary：优先用后端携带的原始字节（含 0x80+ 孤立字节），否则 TextEncoder 恢复有效 UTF-8 字节。
 *  ascii-hex：从 .text 抽取十六进制字节对（设备发 "01 00 ..." 文本）。 */
export function lineBytes(line: LogLine, source: PlotSource): Uint8Array {
  if (source === 'ascii-hex') {
    const out: number[] = []
    HEX_PAIR.lastIndex = 0
    let m: RegExpExecArray | null
    while ((m = HEX_PAIR.exec(line.text)) !== null) out.push(parseInt(m[0], 16))
    return new Uint8Array(out)
  }
  if (line.bytes && line.bytes.length > 0) return new Uint8Array(line.bytes)
  return textEncoder.encode(line.text)
}

/** 计算校验和：sum 取低 8 位，xor 为逐字节异或，none 恒 0（不校验时调用方不应进入校验分支） */
export function computeChecksum(data: Uint8Array, method: PlotChecksum): number {
  if (method === 'sum') {
    let s = 0
    for (let i = 0; i < data.length; i++) s += data[i]
    return s & 0xff
  }
  if (method === 'xor') {
    let x = 0
    for (let i = 0; i < data.length; i++) x ^= data[i]
    return x
  }
  return 0
}

/** 按端序拼装 len 字节整数；用乘法避免 JS 位运算的 32 位截断，支持 1/2/4 字节有/无符号 */
export function parseValue(
  bytes: Uint8Array,
  off: number,
  len: PlotBytes,
  endian: PlotEndian,
  signed: boolean,
): number {
  let v = 0
  if (endian === 'big') {
    for (let k = 0; k < len; k++) v = v * 256 + bytes[off + k]
  } else {
    for (let k = 0; k < len; k++) v = v * 256 + bytes[off + len - 1 - k]
  }
  if (signed) {
    const max = 256 ** len
    if (v >= max / 2) v = v - max
  }
  return v
}

/** 字节数组转大写 hex 串（空格分隔），用于提示展示原始帧 */
export function toHex(bytes: Uint8Array): string {
  let out = ''
  for (let i = 0; i < bytes.length; i++) {
    out += (i ? ' ' : '') + bytes[i].toString(16).padStart(2, '0').toUpperCase()
  }
  return out
}

interface LineRange {
  off0: number
  off1: number
  epochMillis: number
  ts: string
}

/** 二分查找包含某字节偏移的行区间 */
function lineAt(ranges: LineRange[], off: number): LineRange | null {
  let lo = 0
  let hi = ranges.length - 1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const r = ranges[mid]
    if (off < r.off0) hi = mid - 1
    else if (off >= r.off1) lo = mid + 1
    else return r
  }
  return null
}

export interface ParseResult {
  points: PlotPoint[]
  frameCount: number
  lastError: string
}

/** 拼接 RX 行字节为一流，按帧头/帧尾/校验切帧，解析多通道点。
 *  帧布局：[HEADER][DATA(channels×bytesPerChannel)][CHECKSUM?(0/1B)][TAIL?] */
export function parseFrames(config: PlotConfig, rxLines: LogLine[]): ParseResult {
  const channels = Math.max(1, Math.floor(config.channels) || 1)
  const bytesPerChannel = (config.bytesPerChannel || 2) as PlotBytes
  const dataLen = channels * bytesPerChannel
  const checksumLen = config.checksum === 'none' ? 0 : 1

  const head = parseHexField(config.frameHead)
  const tail = parseHexField(config.frameTail)
  const tailLen = tail.length
  const frameLen = dataLen + checksumLen + tailLen

  if (head.length === 0 && tailLen === 0) {
    return { points: [], frameCount: 0, lastError: '需设置帧头或帧尾' }
  }
  if (frameLen <= 0) {
    return { points: [], frameCount: 0, lastError: '通道/字节配置无效' }
  }

  // 拼接字节流并记录每行字节区间 -> 行时间戳
  const chunks: Uint8Array[] = []
  const ranges: LineRange[] = []
  let total = 0
  for (const ln of rxLines) {
    if (ln.dir !== 'rx') continue
    const b = lineBytes(ln, config.source)
    if (b.length === 0) continue
    chunks.push(b)
    ranges.push({ off0: total, off1: total + b.length, epochMillis: ln.epochMillis, ts: ln.ts })
    total += b.length
  }
  if (total < frameLen + head.length) {
    return { points: [], frameCount: 0, lastError: '' }
  }
  const bytes = new Uint8Array(total)
  let p = 0
  for (const b of chunks) {
    bytes.set(b, p)
    p += b.length
  }

  const points: PlotPoint[] = []
  let idx = 0

  const pushPoint = (frameStart: number, frameEnd: number) => {
    idx += 1
    const values: number[] = []
    const dataStart = frameStart + head.length
    for (let ch = 0; ch < channels; ch++) {
      values.push(parseValue(bytes, dataStart + ch * bytesPerChannel, bytesPerChannel, config.endian, config.signed))
    }
    const ln = lineAt(ranges, frameEnd - 1)
    points.push({
      idx,
      values,
      epochMillis: ln ? ln.epochMillis : 0,
      ts: ln ? ln.ts : '',
      rawHex: toHex(bytes.subarray(frameStart, frameEnd)),
    })
  }

  if (head.length > 0) {
    // 帧头优先扫描
    const need = head.length + frameLen
    let i = 0
    while (i + need <= total) {
      let ok = true
      for (let k = 0; k < head.length; k++) {
        if (bytes[i + k] !== head[k]) {
          ok = false
          break
        }
      }
      if (!ok) {
        i += 1
        continue
      }
      const dataStart = i + head.length
      const frameEnd = dataStart + frameLen
      if (tailLen > 0) {
        let t = true
        for (let k = 0; k < tailLen; k++) {
          if (bytes[frameEnd - tailLen + k] !== tail[k]) {
            t = false
            break
          }
        }
        if (!t) {
          i += 1
          continue
        }
      }
      if (checksumLen > 0) {
        const expect = computeChecksum(bytes.subarray(dataStart, dataStart + dataLen), config.checksum)
        if (bytes[dataStart + dataLen] !== expect) {
          i += 1
          continue
        }
      }
      pushPoint(i, frameEnd)
      i = frameEnd
    }
  } else {
    // 无帧头：按帧尾反向定位
    let i = 0
    while (i + tailLen <= total) {
      let t = true
      for (let k = 0; k < tailLen; k++) {
        if (bytes[i + k] !== tail[k]) {
          t = false
          break
        }
      }
      if (!t) {
        i += 1
        continue
      }
      const frameEnd = i + tailLen
      const frameStart = frameEnd - frameLen
      if (frameStart < 0) {
        i += 1
        continue
      }
      const dataStart = frameStart + 0 // 无头
      if (checksumLen > 0) {
        const expect = computeChecksum(bytes.subarray(dataStart, dataStart + dataLen), config.checksum)
        if (bytes[dataStart + dataLen] !== expect) {
          i += 1
          continue
        }
      }
      pushPoint(frameStart, frameEnd)
      i = frameEnd
    }
  }

  const frameCount = idx
  let visible = points
  if (points.length > config.maxPoints) {
    visible = points.slice(points.length - config.maxPoints)
  }
  return { points: visible, frameCount, lastError: '' }
}
