/**
 * 解析引擎运行时数据类型（与 UI/store 的契约）。
 * 脚本 ABI 形状来源是 src/parser/parser-abi.d.ts（全局命名空间 BytetideParser），
 * 这里只放引擎产出与内部规范化结构，不重复声明 ABI。
 */
import type { Dir } from './index'

export type ParserSource = BytetideParser.Source
export type ParserEndian = BytetideParser.Endian
export type ParserFmt = BytetideParser.Fmt
export type ParserCrcAlgo = BytetideParser.CrcAlgo

// ---- framing 规范化结构（hex 串已解析为字节，引擎内部用） ----

export type NormLength =
  | { kind: 'fixed'; value: number }
  | { kind: 'field'; at: number; fmt: 'u8' | 'u16' | 'u32'; endian: ParserEndian; add: number }
  | { kind: 'until'; tail: Uint8Array }
  | { kind: 'line' }

export interface NormCrc {
  algo: ParserCrcAlgo
  /** CRC 字节数（tail:N 的 N，1/2/4） */
  n: number
  endian: ParserEndian
}

export interface NormalizedFraming {
  source: ParserSource
  sync: Uint8Array
  length: NormLength
  crc: NormCrc | null
  maxSize: number
}

// ---- 脚本校验产物（schema 校验 + 规范化一次性完成） ----

export interface ValidatedType {
  at: number
  fmt: ParserFmt
  endian: ParserEndian
  map: Record<string, string>
}

export interface ValidatedField {
  label: string
  at: number
  fmt: ParserFmt
  endian: ParserEndian
  scale: number
  offset: number
  unit: string
  map: Record<string, string> | null
}

export interface ValidatedScript {
  meta: BytetideParser.Meta
  framing: NormalizedFraming
  type: ValidatedType | null
  fields: ValidatedField[] | null
  text: string | null
  /** 静态扫描：源码含 parse（脚本层）= true，帧翻译走 Worker */
  hasParse: boolean
}

// ---- 解码结果（store 落表与 UI 展示的数据契约） ----

export interface DecodedField {
  label: string
  value: number | string
  unit?: string
  /** 位置标注（如 'i16be@4×0.1' / '越界@12'） */
  raw?: string
}

/** 一条解码帧：no 锚定 LogLine.no（定位日志原文用）；元素入 store 前 markRaw */
export interface DecodedFrame {
  no: number
  ts: string
  dir: Dir
  type: string
  text: string
  fields: DecodedField[] | null
  warn: string | null
  frameHex: string
  frameLen: number
  /** true/false/null = 校验通过/失败/未配置 CRC */
  crcOk: boolean | null
}

// ---- 试运行 / 引擎状态 ----

/** 试运行三分类：no-data=无数据直接启用；suspect=疑似 framing 配置错误；ok=通过 */
export type TrialVerdict = 'no-data' | 'suspect' | 'ok'

export interface TrialReport {
  verdict: TrialVerdict
  /** 试运行扫描的行数与切出的帧数 */
  lines: number
  frames: number
  crcFailed: number
  parseErrors: number
  /** 抽样展示（前 N 帧：hex → 结果文本） */
  samples: { hex: string; text: string; type: string }[]
}

/** 引擎全局统计（脚本级，加载/启用时清零） */
export interface ParserStats {
  frames: number
  ok: number
  crcFailed: number
  parseErrors: number
  /** Worker 队列背压丢弃的帧（丢结果不脏流） */
  dropped: number
  types: number
}

/** 引擎横幅（面板顶部提示）：suspect=试运行未过；tripped=错误率熔断；timeout=看门狗 */
export type ParserBannerKind = 'suspect' | 'tripped' | 'timeout' | 'error'

export interface ParserBanner {
  kind: ParserBannerKind
  msg: string
}
