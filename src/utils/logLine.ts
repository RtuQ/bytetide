import type { LogLine } from '../types'

// 模块级单例：TextEncoder 构造有开销，禁止每行/每批 new（encode 本身无状态可复用）
const encoder = new TextEncoder()

/** 日志行真实字节数：优先原始字节（LogLine.bytes 是后端仅在该行含非法 UTF-8 时
 *  附带的原始字节数组），无 bytes 才回退文本 UTF-8 编码长度——二进制帧行禁止把
 *  lossy 文本（U+FFFD）再编码当字节用（与 HEX 渲染语义一致）。 */
export function byteLength(line: Pick<LogLine, 'bytes' | 'text'>): number {
  return line.bytes?.length ?? encoder.encode(line.text).length
}
