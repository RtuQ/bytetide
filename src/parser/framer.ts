/**
 * 主线程流式切帧状态机（纯函数，vitest 直测，不 mock Worker）。
 *
 * 语义钉死（docs/parser-spec.md 逐条展开）：
 * - 所有偏移 0 基；frame = 线上完整帧（含 sync、长度域、CRC 字节；line 帧不含行尾 CR/LF）
 * - 长度域：总帧长 = 长度域原始值 + add（add 补偿 sync 与头部长度）
 * - CRC：覆盖范围 = frame 去掉 CRC 字节自身；位置 tail:N（帧尾倒数起算）
 * - 重同步：有 sync 时定长/长度域坏长度或 CRC 失败 → 前进一字节滑窗找下一个 sync
 *   （沿用 parseFrames「head mismatch 前进一字节」语义）；until/line 在 sync 模式下
 *   找不到分隔符且超限 → 直接复位缓冲（逐字节滑窗对分隔符协议无意义且 O(n²)）；
 *   无 sync 的长度域协议坏长度 → 丢弃整段缓冲复位并计警告（坏长度值下无法可靠重同步）
 * - 状态按 (sessionId, dir) 隔离由调用方（engine）保证，本模块只管单条字节流
 */
import { parseHexField } from '../composables/usePlotParser'
import { computeCrc } from './crc'
import type { NormalizedFraming } from '../types/parser'

export interface FrameOut {
  bytes: Uint8Array
  /** true/false/null = 校验通过/失败/未配置 CRC */
  crcOk: boolean | null
}

export interface FramerState {
  buf: Uint8Array
  /** 累计警告次数（坏长度复位 / 超大帧丢弃） */
  warnings: number
}

export function createFramerState(): FramerState {
  return { buf: new Uint8Array(0), warnings: 0 }
}

export interface FeedResult {
  frames: FrameOut[]
  /** 本次 feed 产生的警告数 */
  warnings: number
}

/** 解析 framing 声明中的 hex 串为字节（schema 校验已在 validateDecl 完成，这里只做转换） */
export function normalizeFramingParts(
  f: BytetideParser.Framing,
): Pick<NormalizedFraming, 'sync' | 'length' | 'crc' | 'maxSize'> {
  const maxSize =
    typeof f.maxSize === 'number' && Number.isFinite(f.maxSize) && f.maxSize > 0
      ? Math.floor(f.maxSize)
      : 4096
  const sync = f.sync ? new Uint8Array(parseHexField(f.sync)) : new Uint8Array(0)
  let length: NormalizedFraming['length']
  switch (f.length.kind) {
    case 'fixed':
      length = { kind: 'fixed', value: Math.floor(f.length.value) }
      break
    case 'field':
      length = {
        kind: 'field',
        at: Math.floor(f.length.at),
        fmt: f.length.fmt,
        endian: f.length.endian ?? 'little',
        add: Math.floor(f.length.add),
      }
      break
    case 'until':
      length = { kind: 'until', tail: new Uint8Array(parseHexField(f.length.tail)) }
      break
    case 'line':
      length = { kind: 'line' }
      break
  }
  let crc: NormalizedFraming['crc'] = null
  if (f.crc) {
    const m = /^tail:(\d+)$/.exec(f.crc.at.trim())
    crc = {
      algo: f.crc.algo,
      n: m ? Math.floor(Number(m[1])) : 0,
      endian: f.crc.endian ?? 'little',
    }
  }
  return { sync, length, crc, maxSize }
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length)
  out.set(a, 0)
  out.set(b, a.length)
  return out
}

/** 朴素子串搜索（sync/tail 均为短字节串，O(n·m) 足够） */
function findPattern(buf: Uint8Array, pat: Uint8Array, from: number): number {
  if (pat.length === 0) return -1
  outer: for (let i = from; i + pat.length <= buf.length; i++) {
    for (let k = 0; k < pat.length; k++) {
      if (buf[i + k] !== pat[k]) continue outer
    }
    return i
  }
  return -1
}

/** 按端序读 n 字节无符号整数（乘法拼装，避免位运算 32 位截断） */
function readUint(buf: Uint8Array, at: number, n: number, endian: 'big' | 'little'): number {
  let v = 0
  if (endian === 'big') {
    for (let k = 0; k < n; k++) v = v * 256 + buf[at + k]
  } else {
    for (let k = 0; k < n; k++) v = v * 256 + buf[at + n - 1 - k]
  }
  return v
}

const FIELD_FMT_SIZE: Record<'u8' | 'u16' | 'u32', number> = { u8: 1, u16: 2, u32: 4 }

/**
 * 从 buf[0]（已对齐帧起点）判读帧边界。
 * total = 消费字节数（帧 + 分隔符/行尾）；frameEnd = 帧字节数（line 不含 CR/LF）。
 * 'bad' = 长度域越界/超限/分隔符缺失且超限；'more' = 需要更多字节。
 */
function frameBounds(
  buf: Uint8Array,
  cfg: NormalizedFraming,
): { total: number; frameEnd: number } | 'bad' | 'more' {
  const len = cfg.length
  const crcN = cfg.crc ? cfg.crc.n : 0
  switch (len.kind) {
    case 'fixed':
      return { total: len.value, frameEnd: len.value }
    case 'field': {
      const size = FIELD_FMT_SIZE[len.fmt]
      if (buf.length < len.at + size) return 'more'
      const raw = readUint(buf, len.at, size, len.endian)
      const total = raw + len.add
      const minTotal = Math.max(len.at + size, crcN > 0 ? crcN + 1 : 0)
      if (!Number.isFinite(total) || total < minTotal || total > cfg.maxSize) return 'bad'
      return { total, frameEnd: total }
    }
    case 'until': {
      if (len.tail.length === 0) return 'bad'
      const j = findPattern(buf, len.tail, 0)
      if (j < 0) return buf.length > cfg.maxSize ? 'bad' : 'more'
      return { total: j + len.tail.length, frameEnd: j + len.tail.length }
    }
    case 'line': {
      const nl = buf.indexOf(0x0a)
      if (nl < 0) return buf.length > cfg.maxSize ? 'bad' : 'more'
      const frameEnd = nl > 0 && buf[nl - 1] === 0x0d ? nl - 1 : nl
      return { total: nl + 1, frameEnd }
    }
  }
}

/** 帧内 CRC 校验（覆盖范围 = frame 去掉 CRC 字节自身） */
function checkCrc(frame: Uint8Array, cfg: NormalizedFraming): boolean | null {
  if (!cfg.crc) return null
  const n = cfg.crc.n
  if (n <= 0 || frame.length <= n) return false
  const body = frame.subarray(0, frame.length - n)
  const expect = computeCrc(cfg.crc.algo, body)
  const stored = readUint(frame, frame.length - n, n, cfg.crc.endian)
  return expect === stored
}

/**
 * 喂入一段字节，吐出切出的完整帧（含 CRC 失败帧——计入警告不进翻译，由引擎包装展示）。
 * 有 sync 时 CRC 失败帧不消费（前进一字节滑窗重同步）；无 sync 时消费整帧继续。
 */
export function framerFeed(
  state: FramerState,
  cfg: NormalizedFraming,
  chunk: Uint8Array,
): FeedResult {
  const frames: FrameOut[] = []
  let warnings = 0
  if (chunk.length > 0) state.buf = concat(state.buf, chunk)
  const hasSync = cfg.sync.length > 0

  /** failAdvance：CRC 失败时的前进量——有 sync 滑窗 1 字节重同步；无 sync 帧边界自洽，消费整帧 */
  const emit = (total: number, frameEnd: number, failAdvance: number) => {
    const bytes = state.buf.subarray(0, frameEnd)
    const crcOk = checkCrc(bytes, cfg)
    frames.push({ bytes, crcOk })
    // 消费量按 total（帧 + 分隔符/行尾），line 帧的 CRLF 不残留
    state.buf = state.buf.slice(crcOk === false ? failAdvance : total)
  }

  const resetBuffer = () => {
    state.buf = new Uint8Array(0)
    warnings += 1
    state.warnings += 1
  }

  outer: while (state.buf.length > 0) {
    if (hasSync) {
      // 丢弃首个 sync 之前的垃圾前缀；找不到 sync 时保留 syncLen-1 字节防跨批截断
      const i = findPattern(state.buf, cfg.sync, 0)
      if (i < 0) {
        const keep = cfg.sync.length - 1
        if (state.buf.length > keep) state.buf = state.buf.slice(state.buf.length - keep)
        break outer
      }
      if (i > 0) state.buf = state.buf.slice(i)
      const b = frameBounds(state.buf, cfg)
      if (b === 'bad') {
        // 定长/长度域：滑窗重同步（前进一字节，sync 字节本身计入垃圾）；
        // until/line：分隔符缺失即垃圾，复位缓冲（逐字节滑窗 O(n²) 无意义）
        if (cfg.length.kind === 'until' || cfg.length.kind === 'line') resetBuffer()
        else state.buf = state.buf.slice(1)
        continue
      }
      if (b === 'more' || state.buf.length < b.total) break outer
      emit(b.total, b.frameEnd, 1)
      continue
    }

    // 无 sync：帧起点即缓冲起点
    const len = cfg.length
    switch (len.kind) {
      case 'fixed': {
        if (state.buf.length < len.value) break outer
        emit(len.value, len.value, len.value)
        break
      }
      case 'field': {
        const b = frameBounds(state.buf, cfg)
        if (b === 'bad') {
          // 坏长度且无 sync：无法可靠重同步，丢弃整段缓冲复位
          resetBuffer()
          break outer
        }
        if (b === 'more' || state.buf.length < b.total) break outer
        emit(b.total, b.frameEnd, b.total)
        break
      }
      case 'until': {
        const b = frameBounds(state.buf, cfg)
        if (b === 'bad') {
          resetBuffer()
          break outer
        }
        if (b === 'more' || state.buf.length < b.total) break outer
        emit(b.total, b.frameEnd, b.total)
        break
      }
      case 'line': {
        const b = frameBounds(state.buf, cfg)
        if (b === 'bad') {
          resetBuffer()
          break outer
        }
        if (b === 'more' || state.buf.length < b.total) break outer
        emit(b.total, b.frameEnd, b.total)
        break
      }
    }
  }
  return { frames, warnings }
}
