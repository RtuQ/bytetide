/**
 * 日志行 → 字节的三态还原（解析引擎与绘图解析共用）：
 * 1. ascii-hex：从行文本抽取十六进制字节对（设备发 "01 00 ..." 文本）
 * 2. binary 且后端携带原始字节：直接用（含 0x80+ 孤立字节，TextEncoder 无法还原）
 * 3. 否则 TextEncoder 编码行文本（有效 UTF-8 场景）
 */
import type { LogLine, PlotSource } from '../types'

const HEX_PAIR = /[0-9a-fA-F]{2}/g

export function lineBytes(line: LogLine, source: PlotSource): Uint8Array {
  if (source === 'ascii-hex') {
    const out: number[] = []
    HEX_PAIR.lastIndex = 0
    let m: RegExpExecArray | null
    while ((m = HEX_PAIR.exec(line.text)) !== null) out.push(parseInt(m[0], 16))
    return new Uint8Array(out)
  }
  if (line.bytes && line.bytes.length > 0) return new Uint8Array(line.bytes)
  return new TextEncoder().encode(line.text)
}
