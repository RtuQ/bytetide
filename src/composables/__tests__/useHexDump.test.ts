import { describe, it, expect } from 'vitest'
import { lineHexDump, lineHexLen } from '../useHexDump'

describe('lineHexDump', () => {
  it('纯文本行按 UTF-8 编码转大写十六进制', () => {
    expect(lineHexDump('AB')).toBe('41 42')
    expect(lineHexDump('温')).toBe('E6 B8 A9') // 中文占 3 字节
    expect(lineHexDump('')).toBe('')
  })

  it('有原始字节时优先使用（lossy 文本的 U+FFFD 不再参与编码）', () => {
    // 设备帧 AA 55 01 A4：lossy 后 text 变 "\uFFFD\uFFFD\x01\uFFFD"，
    // 旧实现按 text 编码会得到 "EF BF BD EF BF BD 01 EF BF BD"
    const raw = [0xaa, 0x55, 0x01, 0xa4]
    expect(lineHexDump('\uFFFD\uFFFD\x01\uFFFD', raw)).toBe('AA 55 01 A4')
  })

  it('空原始字节数组视同无字节，回退 text 编码', () => {
    expect(lineHexDump('AB', [])).toBe('41 42')
    expect(lineHexDump('AB', null)).toBe('41 42')
  })

  it('超过封顶字节数截断并以省略号结尾', () => {
    const raw = Array.from({ length: 600 }, (_, i) => i % 256)
    const out = lineHexDump('x', raw)
    expect(out.endsWith('…')).toBe(true)
    expect(out.replace(' …', '').split(' ')).toHaveLength(512)
  })

  it('恰好等于封顶字节数时不加省略号', () => {
    const raw = [0x01, 0x02]
    expect(lineHexDump('x', raw, 2)).toBe('01 02')
  })
})

describe('lineHexLen', () => {
  it('有原始字节按字节长度估宽，否则按文本长度', () => {
    expect(lineHexLen('\uFFFD\uFFFD', [1, 2, 3])).toBe(3)
    expect(lineHexLen('abc')).toBe(3)
    expect(lineHexLen('abc', null)).toBe(3)
  })
})
