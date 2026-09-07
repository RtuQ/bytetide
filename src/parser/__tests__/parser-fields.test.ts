import { describe, it, expect } from 'vitest'
import { readValue, decodeDeclarative, formatNumber, renderTemplate, autoText } from '../fields'
import { normalizeFramingParts } from '../framer'
import type { ValidatedField, ValidatedScript } from '../../types/parser'

const bytes = (arr: number[]) => new Uint8Array(arr)

/** 构造一个通过校验的声明式脚本（framing 部分由 normalizeFramingParts 归一化） */
type TestField = Partial<ValidatedField> & Pick<ValidatedField, 'label' | 'at' | 'fmt'>
function mkScript(over: {
  name?: string
  type?: ValidatedScript['type']
  fields?: TestField[] | null
  text?: string | null
}): ValidatedScript {
  const norm = (fs: TestField[] | null): ValidatedField[] | null =>
    fs === null ? null : fs.map((f) => ({ endian: 'little', scale: 1, offset: 0, unit: '', map: null, ...f }) as ValidatedField)
  return {
    meta: { name: over.name ?? '温控协议', version: '2.1' },
    framing: {
      ...normalizeFramingParts({ source: 'binary', length: { kind: 'line' } }),
      source: 'binary',
      sync: new Uint8Array([0xaa, 0x55]),
    },
    type: over.type ?? null,
    fields: norm(over.fields ?? null),
    text: over.text ?? null,
    hasParse: false,
  }
}

describe('readValue', () => {
  it('u8/i8 有符号边界', () => {
    expect(readValue(bytes([0x80]), 0, 'i8', 'little')).toBe(-128)
    expect(readValue(bytes([0x7f]), 0, 'i8', 'little')).toBe(127)
  })
  it('i16 大端/小端', () => {
    expect(readValue(bytes([0x00, 0xfa]), 0, 'i16', 'big')).toBe(250)
    expect(readValue(bytes([0xfa, 0x00]), 0, 'i16', 'little')).toBe(250)
    expect(readValue(bytes([0x9c, 0xff]), 0, 'i16', 'little')).toBe(-100)
  })
  it('u32 乘法拼装无 32 位截断', () => {
    expect(readValue(bytes([0x00, 0x00, 0x01, 0x00]), 0, 'u32', 'big')).toBe(256)
  })
  it('f32 大端 25.5', () => {
    // 25.5 = 0x41CC0000
    expect(readValue(bytes([0x41, 0xcc, 0x00, 0x00]), 0, 'f32', 'big')).toBeCloseTo(25.5, 5)
  })
  it('偏移非 0 与越界安全返回 null', () => {
    expect(readValue(bytes([0x01, 0x02]), 1, 'u8', 'little')).toBe(2)
    expect(readValue(bytes([0x01, 0x02]), 1, 'u16', 'little')).toBeNull()
    expect(readValue(bytes([0x01]), 5, 'u8', 'little')).toBeNull()
  })
})

describe('decodeDeclarative 字段抽取', () => {
  it('scale/offset/unit 与缺省自动拼接文本', () => {
    const s = mkScript({
      fields: [
        { label: '温度', at: 4, fmt: 'i16', endian: 'big', scale: 0.1, unit: '℃' },
        { label: '电量', at: 6, fmt: 'u8', endian: 'little', scale: 1, offset: 0, unit: '%', map: null },
      ],
    })
    const frame = bytes([0xaa, 0x55, 0x01, 0x07, 0x00, 0xfa, 87])
    const r = decodeDeclarative(s, frame)
    expect(r.fields[0]).toMatchObject({ label: '温度', value: '25', unit: '℃', raw: 'i16be@4×0.1' })
    expect(r.fields[1]).toMatchObject({ label: '电量', value: '87', unit: '%' })
    expect(r.text).toBe('温度=25(℃)，电量=87(%)')
    expect(r.type).toBe('温控协议') // 无 type 声明回退 meta.name
  })

  it('map 优先于 scale，raw 保留原始值', () => {
    const s = mkScript({
      fields: [
        { label: '模式', at: 4, fmt: 'u8', endian: 'little', scale: 0.5, offset: 0, unit: '', map: { 0: '待机', 1: '制冷' } },
      ],
    })
    const r = decodeDeclarative(s, bytes([0xaa, 0x55, 0x02, 0x03, 0x01]))
    expect(r.fields[0]).toMatchObject({ value: '制冷', raw: 'u8@4×0.5=0x1' })
  })

  it('map 未命中回退数值', () => {
    const s = mkScript({
      fields: [
        { label: '模式', at: 4, fmt: 'u8', endian: 'little', scale: 1, offset: 0, unit: '', map: { 0: '待机' } },
      ],
    })
    const r = decodeDeclarative(s, bytes([0xaa, 0x55, 0x02, 0x03, 0x09]))
    expect(r.fields[0]!.value).toBe('9')
  })

  it('越界帧安全返回：字段标 — 与越界标注，不抛错', () => {
    const s = mkScript({
      fields: [
        { label: '温度', at: 20, fmt: 'i16', endian: 'big', scale: 0.1, offset: 0, unit: '℃', map: null },
        { label: '模式', at: 4, fmt: 'u8', endian: 'little', scale: 1, offset: 0, unit: '', map: null },
      ],
    })
    const r = decodeDeclarative(s, bytes([0xaa, 0x55, 0x01, 0x02, 0x01]))
    expect(r.fields[0]).toMatchObject({ value: '—', raw: '越界@20' })
    expect(r.fields[1]!.value).toBe('1')
    expect(r.text).toContain('—')
  })

  it('type 声明：map 命中 / 未命中 0x.. / 缺省回退 meta.name', () => {
    const s = mkScript({
      type: { at: 2, fmt: 'u8', endian: 'little', map: { 1: '状态上报' } },
      fields: [],
    })
    expect(decodeDeclarative(s, bytes([0xaa, 0x55, 0x01])).type).toBe('状态上报')
    expect(decodeDeclarative(s, bytes([0xaa, 0x55, 0x7f])).type).toBe('0x7F')
    expect(decodeDeclarative(mkScript({}), bytes([0xaa, 0x55, 0x7f])).type).toBe('温控协议')
  })
})

describe('文本模板', () => {
  it('renderTemplate 插值与未命中占位', () => {
    const fs = [
      { label: '温度', value: '25', unit: '℃' },
      { label: '模式', value: '制冷', unit: undefined },
    ]
    expect(renderTemplate('温度 {温度}℃，模式{模式}', fs)).toBe('温度 25℃，模式制冷')
    expect(renderTemplate('压力{压力}Pa', fs)).toBe('压力—Pa')
  })

  it('autoText 拼接 label=value(unit)', () => {
    expect(autoText([{ label: '温度', value: 25.5, unit: '℃' }, { label: 'X', value: 1, unit: undefined }])).toBe(
      '温度=25.5(℃)，X=1',
    )
  })

  it('formatNumber 去浮点噪声', () => {
    expect(formatNumber(250 * 0.1)).toBe('25')
    expect(formatNumber(-0.1 * 3)).toBe('-0.3')
  })
})
