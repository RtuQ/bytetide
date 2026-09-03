/**
 * 声明式字段层（零代码执行）：字段抽取 + 文本模板渲染 + type 类型名解析。
 * 纯函数，主线程直接求值；越界读取安全返回（不抛错，字段标 '—' 并注越界位置）。
 * value = raw × scale + offset；map 命中显示映射值（raw 保留在位置标注里）。
 */
import type { DecodedField, ParserEndian, ParserFmt, ValidatedField, ValidatedScript } from '../types/parser'

const FMT_SIZE: Record<ParserFmt, number> = { u8: 1, i8: 1, u16: 2, i16: 2, u32: 4, i32: 4, f32: 4 }

/** 读一个字段值；越界返回 null（安全） */
export function readValue(
  frame: Uint8Array,
  at: number,
  fmt: ParserFmt,
  endian: ParserEndian,
): number | null {
  const size = FMT_SIZE[fmt]
  if (at < 0 || at + size > frame.length) return null
  if (fmt === 'f32') {
    return new DataView(frame.buffer, frame.byteOffset + at, 4).getFloat32(0, endian !== 'big')
  }
  let v = 0
  if (endian === 'big') {
    for (let k = 0; k < size; k++) v = v * 256 + frame[at + k]
  } else {
    for (let k = 0; k < size; k++) v = v * 256 + frame[at + size - 1 - k]
  }
  if (fmt[0] === 'i') {
    const max = 256 ** size
    if (v >= max / 2) v -= max
  }
  return v
}

/** 数值展示：去除浮点噪声（0.1×250 → 25 而非 25.000000000000004） */
export function formatNumber(v: number): string {
  if (!Number.isFinite(v)) return String(v)
  return String(Number(v.toPrecision(12)))
}

/** 位置标注（demo 风格）：i16be@4×0.1、u8@6；map 命中追加 =0x.. 保留原始值 */
export function annotate(f: ValidatedField, raw: number): string {
  const scale = f.scale ?? 1
  const offset = f.offset ?? 0
  const sfx = f.fmt === 'u8' || f.fmt === 'i8' ? '' : f.endian === 'big' ? 'be' : 'le'
  let s = `${f.fmt}${sfx}@${f.at}`
  if (scale !== 1) s += `×${scale}`
  if (offset !== 0) s += `+${offset}`
  if (f.map && f.map[String(raw)] !== undefined && f.fmt !== 'f32') {
    s += `=0x${(raw >>> 0).toString(16).toUpperCase()}`
  }
  return s
}

/** type 类型名解析：无 type 声明 → meta.name；读数越界 → '未知'；map 未命中 → 0x.. */
export function resolveTypeName(script: ValidatedScript, frame: Uint8Array): string {
  const t = script.type
  if (!t) return script.meta.name
  const raw = readValue(frame, t.at, t.fmt, t.endian)
  if (raw === null) return '未知'
  const mapped = t.map[String(raw)]
  if (mapped !== undefined) return mapped
  if (t.fmt === 'f32') return String(raw)
  return `0x${(raw >>> 0).toString(16).toUpperCase()}`
}

/** 模板插值：{label} 按字段 label 替换；未命中占位符替换为 '—' */
export function renderTemplate(template: string, fields: DecodedField[]): string {
  return template.replace(/\{([^{}]+)\}/g, (_raw, label: string) => {
    const f = fields.find((x) => x.label === label.trim())
    return f === undefined ? '—' : String(f.value)
  })
}

/** 缺省文本：自动拼接 label=value(unit) */
export function autoText(fields: DecodedField[]): string {
  return fields
    .map((f) => `${f.label}=${String(f.value)}${f.unit ? `(${f.unit})` : ''}`)
    .join('，')
}

export interface DeclarativeResult {
  type: string
  text: string
  fields: DecodedField[]
}

/**
 * 声明式解码一帧（CRC 失败帧由引擎拦截，不进这里）。
 * 纯切帧器（无 fields 且无 type 之外的翻译声明）不走本函数，由引擎直接出原始 hex 行。
 */
export function decodeDeclarative(script: ValidatedScript, frame: Uint8Array): DeclarativeResult {
  const out: DecodedField[] = []
  for (const f of script.fields ?? []) {
    const raw = readValue(frame, f.at, f.fmt, f.endian)
    if (raw === null) {
      out.push({ label: f.label, value: '—', unit: f.unit || undefined, raw: `越界@${f.at}` })
      continue
    }
    const mapped = f.map ? f.map[String(raw)] : undefined
    const value = mapped !== undefined ? mapped : raw * (f.scale ?? 1) + (f.offset ?? 0)
    out.push({
      label: f.label,
      value: typeof value === 'number' ? formatNumber(value) : value,
      unit: f.unit || undefined,
      raw: annotate(f, raw),
    })
  }
  const type = resolveTypeName(script, frame)
  const text = script.text ? renderTemplate(script.text, out) : autoText(out)
  return { type, text, fields: out }
}
