/**
 * 校验计算器纯函数：输入（HEX/ASCII 文本）→ 七种校验算法并排结果。
 * 算法复用 parser/crc.ts（与解析引擎/Worker 内嵌同一实现，参数表见 parser-spec.md）；
 * HEX 解析语义与后端 decode_hex 一致（剔除非 hex 字符后按字节对切分）。
 */
import { computeCrc } from '../parser/crc'

export interface ChecksumRow {
  algo: BytetideParser.CrcAlgo
  /** 结果字节数（8 位=1，16 位=2，32 位=4） */
  bytes: number
  /** 按所选端序格式化后的 hex 串（大写、空格分隔） */
  hexBytes: string
}

const ALGOS: BytetideParser.CrcAlgo[] = [
  'sum8',
  'xor8',
  'crc16-modbus',
  'crc16-ccitt-false',
  'crc16-xmodem',
  'crc16-kermit',
  'crc32',
]

const BYTE_LEN: Record<BytetideParser.CrcAlgo, number> = {
  sum8: 1,
  xor8: 1,
  'crc16-modbus': 2,
  'crc16-ccitt-false': 2,
  'crc16-xmodem': 2,
  'crc16-kermit': 2,
  crc32: 4,
}

/** 输入文本→字节：ascii 走 UTF-8 编码；hex 剔除 0x 前缀与非 hex 字符后按对切分（容忍空白/逗号等分隔） */
export function parseSendBytes(input: string, mode: 'hex' | 'ascii'): Uint8Array {
  if (mode === 'ascii') return new TextEncoder().encode(input)
  const cleaned = input.replace(/0x/gi, '').replace(/[^0-9a-fA-F]/g, '')
  const n = Math.floor(cleaned.length / 2)
  const out = new Uint8Array(n)
  for (let i = 0; i < n; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16)
  return out
}

/** 数值→定长 hex 字节串：默认大端（高位在前），little 反转字节顺序 */
export function valueBytesHex(v: number, bytes: number, endian: 'be' | 'le'): string {
  const parts: string[] = []
  for (let i = 0; i < bytes; i++) {
    parts.push(((v >>> (8 * (bytes - 1 - i))) & 0xff).toString(16).toUpperCase().padStart(2, '0'))
  }
  if (endian === 'le') parts.reverse()
  return parts.join(' ')
}

/** 七算法并排计算；空输入返回空数组（调用方显示空态） */
export function checksumResults(
  input: string,
  mode: 'hex' | 'ascii',
  endian: 'be' | 'le',
): ChecksumRow[] {
  const bytes = parseSendBytes(input, mode)
  if (!bytes.length) return []
  return ALGOS.map((algo) => {
    const n = BYTE_LEN[algo]
    return { algo, bytes: n, hexBytes: valueBytesHex(computeCrc(algo, bytes), n, endian) }
  })
}
