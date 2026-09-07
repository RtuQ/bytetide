/**
 * HEX 视图行渲染（plan 无文档、bugfix：二进制行 HEX 显示错误）。
 *
 * 根因：后端 ring 的 text 经 `String::from_utf8_lossy`，二进制帧里 0x80+ 的
 * 字节全部变成 U+FFFD（EF BF BD），把 text 再按 UTF-8 编码得到的十六进制
 * 已不是设备原始字节。后端 `make_rx_line` 对非法 UTF-8 行附带了原始字节
 * （`LogLine.bytes`），渲染时必须优先使用；纯文本行（无 bytes）才回退到
 * text 的 UTF-8 编码。
 */
export function lineHexDump(text: string, rawBytes?: number[] | null, cap = 512): string {
  const bytes: Uint8Array =
    rawBytes && rawBytes.length > 0 ? Uint8Array.from(rawBytes) : new TextEncoder().encode(text)
  const n = Math.min(bytes.length, cap)
  let out = ''
  for (let i = 0; i < n; i++) out += bytes[i]!.toString(16).padStart(2, '0').toUpperCase() + ' '
  return out.trim() + (bytes.length > n ? ' …' : '')
}

/** HEX 视图估宽用的字节长度：有原始字节按其长度，否则按文本字符数（ANSI 行由调用方先剥离）。 */
export function lineHexLen(text: string, rawBytes?: number[] | null): number {
  return rawBytes && rawBytes.length > 0 ? rawBytes.length : text.length
}
