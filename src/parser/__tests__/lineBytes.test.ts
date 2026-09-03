import { describe, it, expect } from 'vitest'
import { lineBytes } from '../lineBytes'
import type { LogLine } from '../../types'

function mkLine(text: string, bytes: number[] | null): LogLine {
  return { no: 1, ts: '00:00:00.000', dir: 'rx', text, bytes, epochMillis: 0 }
}

describe('lineBytes 三态还原', () => {
  it('ascii-hex：抽取文本中的十六进制字节对（忽略分隔与大小写）', () => {
    const b = lineBytes(mkLine('AA 55 0d 0a', null), 'ascii-hex')
    expect(Array.from(b)).toEqual([0xaa, 0x55, 0x0d, 0x0a])
    expect(lineBytes(mkLine('AA550D', null), 'ascii-hex')).toEqual(new Uint8Array([0xaa, 0x55, 0x0d]))
    expect(lineBytes(mkLine('01,02', null), 'ascii-hex')).toEqual(new Uint8Array([1, 2]))
  })

  it('binary + 后端原始字节：直接使用（含 0x80+ 孤立字节）', () => {
    const raw = [0x80, 0xc3, 0x28]
    const b = lineBytes(mkLine('???', raw), 'binary')
    expect(Array.from(b)).toEqual(raw)
    // 返回副本：改写不影响原数组
    b[0] = 0xff
    expect(raw[0]).toBe(0x80)
  })

  it('binary 无原始字节：TextEncoder 回退（UTF-8 多字节）', () => {
    const b = lineBytes(mkLine('温25', null), 'binary')
    expect(Array.from(b)).toEqual(Array.from(new TextEncoder().encode('温25')))
  })

  it('binary 空字节数组视为缺失，走 TextEncoder 回退', () => {
    const b = lineBytes(mkLine('AB', []), 'binary')
    expect(Array.from(b)).toEqual([0x41, 0x42])
  })

  it('ascii-hex 无 hex 内容返回空数组', () => {
    expect(lineBytes(mkLine('hello', null), 'ascii-hex')).toEqual(new Uint8Array(0))
  })
})
