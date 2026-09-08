import { describe, expect, it } from 'vitest'
import { checksumResults, parseSendBytes, valueBytesHex } from '../useChecksum'

describe('parseSendBytes', () => {
  it('ascii 走 UTF-8 编码', () => {
    expect([...parseSendBytes('AB', 'ascii')]).toEqual([0x41, 0x42])
  })
  it('hex 容忍空白/逗号/0x 前缀，按对切分（对齐后端 decode_hex）', () => {
    expect([...parseSendBytes('01 03 00 00 00 02', 'hex')]).toEqual([
      0x01, 0x03, 0x00, 0x00, 0x00, 0x02,
    ])
    expect([...parseSendBytes('0x01,0x03', 'hex')]).toEqual([0x01, 0x03])
    // 奇数个 hex 字符：末尾落单的丢弃（向下取整）
    expect(parseSendBytes('0 1 0 3 0', 'hex').length).toBe(2)
  })
})

describe('valueBytesHex', () => {
  it('16 位大端高位在前', () => {
    expect(valueBytesHex(0x0bc4, 2, 'be')).toBe('0B C4')
  })
  it('16 位小端反转字节序（Modbus 线上顺序）', () => {
    expect(valueBytesHex(0x0bc4, 2, 'le')).toBe('C4 0B')
  })
  it('32 位与 8 位', () => {
    expect(valueBytesHex(0x12345678, 4, 'le')).toBe('78 56 34 12')
    expect(valueBytesHex(0x06, 1, 'le')).toBe('06')
  })
})

describe('checksumResults', () => {
  // "01 03 00 00 00 02" 的 modbus CRC=0x0BC4，线上字节 C4 0B（小端）——与常见 Modbus 帧一致
  const input = '01 03 00 00 00 02'

  it('七算法齐全且顺序稳定', () => {
    const rows = checksumResults(input, 'hex', 'be')
    expect(rows.map((r) => r.algo)).toEqual([
      'sum8',
      'xor8',
      'crc16-modbus',
      'crc16-ccitt-false',
      'crc16-xmodem',
      'crc16-kermit',
      'crc32',
    ])
    expect(rows.map((r) => r.bytes)).toEqual([1, 1, 2, 2, 2, 2, 4])
  })

  it('modbus CRC 命中已知帧校验值（小端 C4 0B）', () => {
    const rows = checksumResults(input, 'hex', 'le')
    const modbus = rows.find((r) => r.algo === 'crc16-modbus')!
    expect(modbus.hexBytes).toBe('C4 0B')
  })

  it('sum8/xor8 手算吻合', () => {
    const rows = checksumResults(input, 'hex', 'be')
    // 01+03+02 = 06；01^03^02 = 00
    expect(rows[0].hexBytes).toBe('06')
    expect(rows[1].hexBytes).toBe('00')
  })

  it('ascii 模式与 hex 模式走不同字节源', () => {
    const rows = checksumResults('AB', 'ascii', 'be')
    expect(rows[0].hexBytes).toBe('83') // 0x41+0x42=0x83 → 低 8 位
  })

  it('空输入返回空数组', () => {
    expect(checksumResults('', 'hex', 'be')).toEqual([])
    expect(checksumResults('  ', 'hex', 'be')).toEqual([]) // hex 模式下空白全剔除
    expect(checksumResults('', 'ascii', 'be')).toEqual([])
  })
})
