/**
 * 解析引擎主线程编排（parser V1 · docs/plan-parser-v1.md §1/§2）。
 *
 * 职责切分：
 * - 切帧（framer）与声明式字段翻译（fields）在本模块主线程同步执行；
 * - 脚本层 parse 经 EngineHost（bootstrap.ts 的 Worker 沙箱）攒批异步执行，
 *   主线程只负责切帧、攒批、gen 代际、按批看门狗与错误率熔断；
 * - 状态查询全部是 plain object 快照，响应式包装由组合层（useParserEngine）负责。
 *
 * 钉死语义：
 * - load 成功不自动启用（启用走组合层 trialRun → setEnabled 流程）；不清 decoded；
 * - setEnabled(true) 对每会话最近 2000 行回溯（framer 状态直接作为后续 live 状态），
 *   onDecoded(id, frames, true) 整体替换；disable 清 banner/framer、decoded 保留；
 * - gen：resetSession/dropSession 时 +1，迟到 worker 结果按 gen 丢弃；
 * - 看门狗按批：ack 3s、results 10s（ack 到了才起 results 定时器，host.run 的
 *   Promise resolve 即视为 results）；超时 → dispose host + 卸载脚本；
 * - 熔断仅脚本层：滚动 1000 帧错误率 >30% 自动停用，脚本保留可重新启用。
 */
import { parseHexField } from '../composables/usePlotParser'
import type { Dir, LogLine } from '../types'
import type {
  DecodedFrame,
  ParserBanner,
  ParserEndian,
  ParserFmt,
  ParserStats,
  TrialReport,
  TrialVerdict,
  ValidatedField,
  ValidatedScript,
  ValidatedType,
} from '../types/parser'
import { lineBytes } from './lineBytes'
import { normalizeFramingParts, createFramerState, framerFeed } from './framer'
import type { FramerState } from './framer'
import { decodeDeclarative, resolveTypeName } from './fields'

// ===================== 宿主与批处理契约（下游 useParserEngine 依赖） =====================

/** 发往脚本层的一个待解析帧（bytes 已 Array.from 为可 structured-clone 的纯数据） */
export interface EngineBatchItem {
  no: number
  ts: string
  dir: 'rx' | 'tx'
  epochMillis: number
  bytes: number[]
  sessionName: string
}

/** 脚本层逐项结果：ok=false = parse 抛错/返回 null（计入解析错误，不产出行） */
export interface EngineBatchResult {
  no: number
  ok: boolean
  result?: BytetideParser.ParseResult
}

/** 脚本层宿主：load 装载用户模块，run 执行一批 parse，dispose 终止（看门狗/卸载用） */
export interface EngineHost {
  load(src: string): Promise<{ decl: unknown }>
  run(batch: { reqId: number; sessionId: string; items: EngineBatchItem[] }): Promise<{
    reqId: number
    items: EngineBatchResult[]
  }>
  dispose(): void
  /**
   * 批次 ack 通知（看门狗用）：createScriptHost(onAck) 注入初值；
   * 引擎装载 host 后会覆写为自己的看门狗回调（handleAck）。
   */
  onAck?: (reqId: number) => void
}

export interface EngineOpts {
  /** 创建脚本沙箱宿主；返回 null = Worker 不可用（声明式脚本不受影响） */
  hostFactory: () => EngineHost | null
  /** 主线程 blob import（声明式脚本路径，零 Worker） */
  importModule: (blobUrl: string) => Promise<{ default: unknown }>
  makeBlobUrl: (src: string) => string
  /** 解码结果出口；replace=true 整体替换（回溯/卸载），false 追加（live） */
  onDecoded: (sessionId: string, frames: DecodedFrame[], replace: boolean) => void
  onStateChange: () => void
  getSessionName: (id: string) => string
}

// ===================== 可测纯函数 =====================

/** 静态扫描源码是否含脚本层 parse（声明式层不含函数，函数不可 structured-clone） */
export function staticHasParse(src: string): boolean {
  return /(^|[^$\w])parse\s*[(=:]/.test(src)
}

function isObj(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

/** 非负整数（at/位置类声明通用） */
function isAt(v: unknown): v is number {
  return typeof v === 'number' && Number.isInteger(v) && v >= 0
}

function endianOk(v: unknown): boolean {
  return v === undefined || v === 'big' || v === 'little'
}

const FMT_ALL = new Set<string>(['u8', 'u16', 'u32', 'i8', 'i16', 'i32', 'f32'])
const FMT_LEN_SIZE: Record<string, number> = { u8: 1, u16: 2, u32: 4 }
const CRC_ALGOS = new Set<string>([
  'sum8',
  'xor8',
  'crc16-modbus',
  'crc16-ccitt-false',
  'crc16-xmodem',
  'crc16-kermit',
  'crc32',
])
const CRC_TAIL_RE = /^tail:(\d+)$/

/**
 * 声明式 schema 校验 + 规范化（一次性产出 ValidatedScript，framing 走
 * normalizeFramingParts 归一化）。hasParse 取 staticHasParse(src)——worker 回传的
 * decl 只含数据字段，函数留在 worker 内。
 */
export function validateDecl(
  decl: unknown,
  src: string,
): { ok: true; script: ValidatedScript } | { ok: false; error: string } {
  if (!isObj(decl)) return { ok: false, error: '脚本必须 export default 一个对象' }

  // meta
  const meta = decl.meta
  if (!isObj(meta)) return { ok: false, error: '缺少 meta 声明（meta.name / meta.version 必填）' }
  if (typeof meta.name !== 'string' || meta.name.trim() === '' || typeof meta.version !== 'string' || meta.version.trim() === '')
    return { ok: false, error: 'meta.name / meta.version 必须为非空字符串' }

  // framing.source
  const framing = decl.framing
  if (!isObj(framing)) return { ok: false, error: '缺少 framing 切帧声明' }
  if (framing.source !== 'binary' && framing.source !== 'ascii-hex')
    return { ok: false, error: "framing.source 必须是 'binary' 或 'ascii-hex'" }
  if (framing.sync !== undefined && framing.sync !== null && typeof framing.sync !== 'string')
    return { ok: false, error: 'framing.sync 必须为 hex 字符串（如 AA 55）' }

  // maxSize（先定，长度域越界校验要用）
  if (framing.maxSize !== undefined && framing.maxSize !== null) {
    const ms = framing.maxSize
    if (typeof ms !== 'number' || !Number.isFinite(ms) || ms <= 0)
      return { ok: false, error: 'framing.maxSize 必须为正数' }
  }
  const maxSize =
    typeof framing.maxSize === 'number' && Number.isFinite(framing.maxSize) && framing.maxSize > 0
      ? Math.floor(framing.maxSize)
      : 4096

  // framing.length
  const len = framing.length
  if (!isObj(len)) return { ok: false, error: '缺少 framing.length 长度声明' }
  switch (len.kind) {
    case 'fixed':
      if (typeof len.value !== 'number' || !Number.isFinite(len.value) || len.value <= 0)
        return { ok: false, error: 'framing.length.value 必须为正数（定长帧的线上总字节数）' }
      break
    case 'field': {
      if (!isAt(len.at)) return { ok: false, error: 'framing.length.at 必须为非负整数' }
      if (typeof len.fmt !== 'string' || !(len.fmt in FMT_LEN_SIZE))
        return { ok: false, error: "framing.length.fmt 必须是 'u8' / 'u16' / 'u32'" }
      if (!endianOk(len.endian)) return { ok: false, error: "framing.length.endian 必须是 'big' 或 'little'" }
      if (typeof len.add !== 'number' || !Number.isInteger(len.add))
        return { ok: false, error: 'framing.length.add 必须为整数（长度域补偿值，补齐 sync 与头部长度）' }
      const size = FMT_LEN_SIZE[len.fmt]!
      if (len.at + size > maxSize)
        return { ok: false, error: `framing.length.at 越界（at+${size} 超过 maxSize=${maxSize}）` }
      break
    }
    case 'until':
      if (typeof len.tail !== 'string' || parseHexField(len.tail).length === 0)
        return { ok: false, error: "framing.length.tail 必须为非空 hex 串（如 '0D 0A'）" }
      break
    case 'line':
      break
    default:
      return { ok: false, error: `不支持的 framing.length.kind：${String(len.kind)}（fixed/field/until/line）` }
  }

  // framing.crc（可空）
  if (framing.crc !== undefined && framing.crc !== null) {
    const c = framing.crc
    if (!isObj(c)) return { ok: false, error: 'framing.crc 声明形状非法' }
    if (typeof c.algo !== 'string' || !CRC_ALGOS.has(c.algo))
      return { ok: false, error: `不支持的 CRC 算法：${String(c.algo)}（sum8/xor8/crc16-modbus/crc16-ccitt-false/crc16-xmodem/crc16-kermit/crc32）` }
    if (typeof c.at !== 'string' || !CRC_TAIL_RE.test(c.at.trim()))
      return { ok: false, error: "framing.crc.at 必须为 'tail:N' 形式（帧尾倒数 N 字节为 CRC 本身）" }
    const n = Number(CRC_TAIL_RE.exec(c.at.trim())![1])
    if (n !== 1 && n !== 2 && n !== 4)
      return { ok: false, error: 'framing.crc.at 的 N 只支持 1 / 2 / 4 字节' }
    if (!endianOk(c.endian)) return { ok: false, error: "framing.crc.endian 必须是 'big' 或 'little'" }
  }

  // type（可选）
  let typeDecl: ValidatedType | null = null
  if (decl.type !== undefined && decl.type !== null) {
    const t = decl.type
    if (!isObj(t)) return { ok: false, error: 'type 声明形状非法' }
    if (!isAt(t.at)) return { ok: false, error: 'type.at 必须为非负整数' }
    if (typeof t.fmt !== 'string' || !FMT_ALL.has(t.fmt))
      return { ok: false, error: `type.fmt 非法：${String(t.fmt)}（u8/u16/u32/i8/i16/i32/f32）` }
    if (!endianOk(t.endian)) return { ok: false, error: "type.endian 必须是 'big' 或 'little'" }
    if (t.map !== undefined && t.map !== null && !isObj(t.map))
      return { ok: false, error: 'type.map 必须为对象（原始值 → 类型名）' }
    typeDecl = {
      at: t.at,
      fmt: t.fmt as ParserFmt,
      endian: (t.endian ?? 'little') as ParserEndian,
      map: isObj(t.map) ? (t.map as Record<string, string>) : {},
    }
  }

  // fields（可选）
  let fieldsOut: ValidatedField[] | null = null
  if (decl.fields !== undefined && decl.fields !== null) {
    if (!Array.isArray(decl.fields)) return { ok: false, error: 'fields 必须为数组' }
    fieldsOut = []
    for (let i = 0; i < decl.fields.length; i++) {
      const f = decl.fields[i] as unknown
      if (!isObj(f)) return { ok: false, error: `fields[${i}] 形状非法` }
      if (typeof f.label !== 'string' || f.label.trim() === '')
        return { ok: false, error: `fields[${i}].label 必须为非空字符串` }
      if (!isAt(f.at)) return { ok: false, error: `fields[${i}].at 必须为非负整数` }
      if (typeof f.fmt !== 'string' || !FMT_ALL.has(f.fmt))
        return { ok: false, error: `fields[${i}].fmt 非法：${String(f.fmt)}（u8/u16/u32/i8/i16/i32/f32）` }
      if (!endianOk(f.endian)) return { ok: false, error: `fields[${i}].endian 必须是 'big' 或 'little'` }
      if (f.scale !== undefined && f.scale !== null && typeof f.scale !== 'number')
        return { ok: false, error: `fields[${i}].scale 必须为数值` }
      if (f.offset !== undefined && f.offset !== null && typeof f.offset !== 'number')
        return { ok: false, error: `fields[${i}].offset 必须为数值` }
      if (f.unit !== undefined && f.unit !== null && typeof f.unit !== 'string')
        return { ok: false, error: `fields[${i}].unit 必须为字符串` }
      if (f.map !== undefined && f.map !== null && !isObj(f.map))
        return { ok: false, error: `fields[${i}].map 必须为对象` }
      fieldsOut.push({
        label: f.label,
        at: f.at,
        fmt: f.fmt as ParserFmt,
        endian: (f.endian ?? 'little') as ParserEndian,
        scale: typeof f.scale === 'number' ? f.scale : 1,
        offset: typeof f.offset === 'number' ? f.offset : 0,
        unit: typeof f.unit === 'string' ? f.unit : '',
        map: isObj(f.map) ? (f.map as Record<string, string>) : null,
      })
    }
  }

  // text（可选）
  if (decl.text !== undefined && decl.text !== null && typeof decl.text !== 'string')
    return { ok: false, error: 'text 必须为字符串模板（{label} 插值）' }

  const framingTyped = framing as unknown as BytetideParser.Framing
  const script: ValidatedScript = {
    meta: {
      name: meta.name,
      version: meta.version,
      ...(typeof meta.author === 'string' ? { author: meta.author } : {}),
      ...(typeof meta.description === 'string' ? { description: meta.description } : {}),
    },
    framing: { source: framingTyped.source, ...normalizeFramingParts(framingTyped) },
    type: typeDecl,
    fields: fieldsOut,
    text: typeof decl.text === 'string' ? decl.text : null,
    hasParse: staticHasParse(src),
  }
  return { ok: true, script }
}

/** 试运行三分类：!hasRx→no-data；0 帧或 CRC 全败→suspect；否则 ok（解析错误不参与 verdict） */
export function classifyTrial(hasRx: boolean, frames: number, crcTotal: number, crcFailed: number): TrialVerdict {
  if (!hasRx) return 'no-data'
  if (frames === 0 || (crcTotal > 0 && crcFailed === crcTotal)) return 'suspect'
  return 'ok'
}

/** 滚动错误率窗口（熔断：最近 cap 帧错误占比 > rate 即触发；300/1000 不触发、301 触发） */
export class ErrorRateWindow {
  private buf: boolean[]
  private idx = 0
  private filled = 0
  private fails = 0

  constructor(
    private cap = 1000,
    private rate = 0.3,
  ) {
    this.buf = new Array(cap)
  }

  push(ok: boolean): void {
    if (this.filled === this.cap) {
      // 窗口已满：挤出最旧一条再写入
      if (this.buf[this.idx] === false) this.fails--
    } else {
      this.filled++
    }
    this.buf[this.idx] = ok
    if (!ok) this.fails++
    this.idx = (this.idx + 1) % this.cap
  }

  shouldTrip(): boolean {
    if (this.filled === 0) return false
    return this.fails / this.filled > this.rate
  }

  clear(): void {
    this.idx = 0
    this.filled = 0
    this.fails = 0
  }
}

// ===================== 引擎 =====================

const ACK_TIMEOUT_MS = 3000
const RESULTS_TIMEOUT_MS = 10000
const QUEUE_CAP = 1000
const BACKFILL_LINES = 2000

interface Inflight {
  reqId: number
  sessionId: string
  gen: number
  /** ack 到达：清 ack 定时器、起 results 定时器 */
  onAck: () => void
  /** 结算本批（结果或 null）；看门狗超时也走这里，保证等待方不悬挂 */
  settle: (v: EngineBatchResult[] | null) => void
}

interface BackfillOutcome {
  frames: DecodedFrame[]
  /** 本会话 gen（回溯开始时），调用方据此判断回溯期间是否被 reset */
  gen: number
  scanned: number
  framesCut: number
  crcTotal: number
  crcFailed: number
  parseErrors: number
  hasRx: boolean
}

function zeroStats(): ParserStats {
  return { frames: 0, ok: 0, crcFailed: 0, parseErrors: 0, dropped: 0, types: 0 }
}

function hexSpaced(bytes: Uint8Array | number[]): string {
  let s = ''
  for (let i = 0; i < bytes.length; i++) {
    s += (i > 0 ? ' ' : '') + bytes[i]!.toString(16).toUpperCase().padStart(2, '0')
  }
  return s
}

/** 同一行切出多帧时 no 会重复：按 no 分桶、按发送顺序消费 */
function groupByNo(items: EngineBatchItem[]): Map<number, EngineBatchItem[]> {
  const m = new Map<number, EngineBatchItem[]>()
  for (const it of items) {
    const arr = m.get(it.no)
    if (arr) arr.push(it)
    else m.set(it.no, [it])
  }
  return m
}

function takeItem(m: Map<number, EngineBatchItem[]>, no: number): EngineBatchItem | undefined {
  const arr = m.get(no)
  if (!arr || arr.length === 0) return undefined
  const it = arr.shift()!
  if (arr.length === 0) m.delete(no)
  return it
}

export class ParserEngine {
  private curScript: ValidatedScript | null = null
  private enabledFlag = false
  private curBanner: ParserBanner | null = null
  private statsObj: ParserStats = zeroStats()
  private typeNames = new Set<string>()
  private report: TrialReport | null = null
  private host: EngineHost | null = null
  /** framer 状态按 (sessionId, dir) 隔离 */
  private framers = new Map<string, Map<Dir, FramerState>>()
  private gens = new Map<string, number>()
  private queues = new Map<string, EngineBatchItem[]>()
  private errWin = new ErrorRateWindow()
  private reqCounter = 0
  private inflight: Inflight | null = null
  /** host 访问串行化：live 泵批 / 回溯批共用一条链，天然不并发 */
  private dispatchChain: Promise<unknown> = Promise.resolve()
  private pumpScheduled = false

  constructor(private opts: EngineOpts) {}

  // ---- 状态查询（plain object 快照，组合层负责转响应式） ----

  get loaded(): boolean {
    return this.curScript !== null
  }

  get enabled(): boolean {
    return this.enabledFlag
  }

  get script(): ValidatedScript | null {
    return this.curScript
  }

  get meta(): BytetideParser.Meta | null {
    return this.curScript?.meta ?? null
  }

  get banner(): ParserBanner | null {
    return this.curBanner
  }

  get stats(): ParserStats {
    return { ...this.statsObj, types: this.typeNames.size }
  }

  get trialReport(): TrialReport | null {
    return this.report
  }

  hasParseScript(): boolean {
    return this.curScript?.hasParse ?? false
  }

  // ---- 生命周期 ----

  /**
   * 校验 + 装载（不启用、不清 decoded）。声明式脚本全程零 Worker；
   * 脚本层经 host.load 拿纯数据 decl（函数留在 worker）。失败置 banner error。
   */
  async load(src: string): Promise<{ ok: boolean; error?: string }> {
    // 换脚本 = 旧沙箱即刻作废（重载场景）
    this.disposeHost()
    const hasParse = staticHasParse(src)
    let decl: unknown
    if (!hasParse) {
      try {
        const mod = await this.opts.importModule(this.opts.makeBlobUrl(src))
        decl = mod.default
      } catch (e) {
        return this.failLoad(`脚本导入失败：${String(e)}`)
      }
    } else {
      const host = this.opts.hostFactory()
      if (!host) return this.failLoad('无法创建脚本沙箱（Worker 不可用）')
      try {
        const res = await host.load(src)
        decl = res.decl
      } catch (e) {
        host.dispose()
        return this.failLoad(`脚本导入失败：${String(e)}`)
      }
      this.attachHost(host)
    }
    const v = validateDecl(decl, src)
    if (!v.ok) {
      this.disposeHost()
      return this.failLoad(v.error)
    }
    // 装载成功：新协议新切帧状态；不启用、不清 decoded（load 成功 + setEnabled(true) 时 stats 清零）
    this.curScript = v.script
    this.enabledFlag = false
    this.curBanner = null
    this.report = null
    this.resetStats()
    this.errWin.clear()
    this.framers.clear()
    this.gens.clear()
    this.queues.clear()
    this.opts.onStateChange()
    return { ok: true }
  }

  /** 启停。enable：stats 清零 + 每会话最近 2000 行回溯（整体替换 decoded，无帧也 replace）；disable：banner/framer 清空，decoded 保留 */
  async setEnabled(v: boolean, sessions: { id: string; lines: LogLine[] }[]): Promise<void> {
    if (!this.curScript) return
    if (!v) {
      this.enabledFlag = false
      this.curBanner = null
      this.framers.clear()
      this.opts.onStateChange()
      return
    }
    this.enabledFlag = true
    this.curBanner = null
    this.resetStats()
    this.errWin.clear()
    this.opts.onStateChange()
    for (const s of sessions) {
      const r = await this.backfillSession(s.id, s.lines, true)
      // 回溯期间会话被 reset（clearLog/重连）则放弃替换，交由 live feed 重建
      if (this.genOf(s.id) === r.gen) this.opts.onDecoded(s.id, r.frames, true)
    }
    this.opts.onStateChange()
  }

  /** 停用 + 丢弃脚本：terminate host、清 framer/gen、banner 清空、stats 清零（localStorage 清理由组合层负责） */
  async unload(sessions: { id: string; lines: LogLine[] }[]): Promise<void> {
    this.enabledFlag = false
    this.curScript = null
    this.curBanner = null
    this.report = null
    this.resetStats()
    this.errWin.clear()
    this.disposeAll()
    // 丢弃脚本 = 该脚本的解码表一并清空
    for (const s of sessions) this.opts.onDecoded(s.id, [], true)
    this.opts.onStateChange()
  }

  /**
   * 导入后的试运行三分类（不改变 enabled、不进全局 stats）。
   * activeId 会话出 samples（前 5 帧）；结束后清掉回溯 framer 状态，避免污染后续 enable 的正式回溯。
   */
  async trialRun(activeId: string | null, sessions: { id: string; lines: LogLine[] }[]): Promise<TrialReport> {
    let scanned = 0
    let frames = 0
    let crcTotal = 0
    let crcFailed = 0
    let parseErrors = 0
    let hasRx = false
    let samples: TrialReport['samples'] = []
    for (const s of sessions) {
      const r = await this.backfillSession(s.id, s.lines, false)
      scanned += r.scanned
      frames += r.framesCut
      crcTotal += r.crcTotal
      crcFailed += r.crcFailed
      parseErrors += r.parseErrors
      if (s.id === activeId) {
        hasRx = r.hasRx
        samples = r.frames.slice(0, 5).map((f) => ({ hex: f.frameHex, text: f.text, type: f.type }))
      }
    }
    // 清掉回溯 framer 状态（framer 缓冲是试运行的临时产物）
    for (const s of sessions) {
      this.framers.delete(s.id)
      this.bumpGen(s.id)
    }
    const report: TrialReport = {
      verdict: classifyTrial(hasRx, frames, crcTotal, crcFailed),
      lines: scanned,
      frames,
      crcFailed,
      parseErrors,
      samples,
    }
    this.report = report
    this.opts.onStateChange()
    return report
  }

  // ---- 数据入口 / 钩子 ----

  /** loaded+enabled 才处理；无脚本层=同步声明式解码；有=CRC 失败帧主线程包装、其余攒批异步 */
  feed(sessionId: string, lines: LogLine[]): void {
    const script = this.curScript
    if (!script || !this.enabledFlag) return
    // parse 存在时走脚本层并忽略 fields/text（ABI 钉死）
    const declarative = !script.hasParse && script.fields !== null && script.fields.length > 0
    const frames: DecodedFrame[] = []
    for (const line of lines) {
      const st = this.framerFor(sessionId, line.dir)
      const r = framerFeed(st, script.framing, lineBytes(line, script.framing.source))
      for (const f of r.frames) {
        this.statsObj.frames++
        if (f.crcOk === false) {
          this.statsObj.crcFailed++
          frames.push(this.crcFailFrame(line.no, line.ts, line.dir, f.bytes))
          continue
        }
        if (declarative) {
          const d = decodeDeclarative(script, f.bytes)
          this.statsObj.ok++
          this.typeNames.add(d.type)
          frames.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            type: d.type,
            text: d.text,
            fields: d.fields,
            warn: null,
            frameHex: hexSpaced(f.bytes),
            frameLen: f.bytes.length,
            crcOk: f.crcOk,
          })
        } else if (script.hasParse) {
          // 脚本层：攒批（队列上限 1000，超出丢最旧——丢结果不脏流）
          let q = this.queues.get(sessionId)
          if (!q) {
            q = []
            this.queues.set(sessionId, q)
          }
          if (q.length >= QUEUE_CAP) {
            q.shift()
            this.statsObj.dropped++
          }
          q.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            epochMillis: line.epochMillis,
            bytes: Array.from(f.bytes),
            sessionName: this.opts.getSessionName(sessionId),
          })
        } else {
          // 纯切帧器（无 fields 无 parse）：原始 hex 行，零执行、零 Worker
          const type = resolveTypeName(script, f.bytes)
          this.statsObj.ok++
          this.typeNames.add(type)
          frames.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            type,
            text: hexSpaced(f.bytes),
            fields: null,
            warn: null,
            frameHex: hexSpaced(f.bytes),
            frameLen: f.bytes.length,
            crcOk: f.crcOk,
          })
        }
      }
    }
    if (frames.length > 0) this.opts.onDecoded(sessionId, frames, false)
    this.schedulePump()
  }

  /** clearLog 钩子：framer 复位 + gen+1（迟到 worker 结果按 gen 丢弃），待发队列一并作废 */
  resetSession(sessionId: string): void {
    this.framers.delete(sessionId)
    this.queues.delete(sessionId)
    this.bumpGen(sessionId)
  }

  /** closeTab / 重连旧 id：清 framer/gen */
  dropSession(sessionId: string): void {
    this.resetSession(sessionId)
  }

  /** terminate host（重建机会留给下次 load）+ 清会话态 */
  disposeAll(): void {
    this.disposeHost()
    this.framers.clear()
    this.gens.clear()
    this.queues.clear()
  }

  // ---- 内部：装载辅助 ----

  /**
   * 批次 ack 入口（组合层经 createScriptHost((reqId) => engine.notifyAck(reqId)) 接线；
   * 也接受宿主对象自带 onAck 的形式）。
   */
  notifyAck(reqId: number): void {
    this.handleAck(reqId)
  }

  private failLoad(msg: string): { ok: false; error: string } {
    this.curBanner = { kind: 'error', msg }
    this.opts.onStateChange()
    return { ok: false, error: msg }
  }

  private attachHost(host: EngineHost): void {
    this.host = host
    // 宿主未自带 ack 转发（如测试注入的 fake host）时，引擎自行挂接
    if (!host.onAck) host.onAck = (reqId) => this.handleAck(reqId)
  }

  private disposeHost(): void {
    // 挂起批先结算（清看门狗定时器），再 terminate
    if (this.inflight) this.inflight.settle(null)
    if (this.host) {
      this.host.dispose()
      this.host = null
    }
  }

  private resetStats(): void {
    this.statsObj = zeroStats()
    this.typeNames.clear()
  }

  // ---- 内部：framer / gen ----

  private framerFor(sessionId: string, dir: Dir): FramerState {
    let byDir = this.framers.get(sessionId)
    if (!byDir) {
      byDir = new Map<Dir, FramerState>()
      this.framers.set(sessionId, byDir)
    }
    let st = byDir.get(dir)
    if (!st) {
      st = createFramerState()
      byDir.set(dir, st)
    }
    return st
  }

  private genOf(id: string): number {
    return this.gens.get(id) ?? 0
  }

  private bumpGen(id: string): void {
    this.gens.set(id, this.genOf(id) + 1)
  }

  // ---- 内部：帧包装 ----

  private crcFailFrame(no: number, ts: string, dir: Dir, bytes: Uint8Array): DecodedFrame {
    return {
      no,
      ts,
      dir,
      type: 'CRC 错误',
      text: '帧校验失败，未翻译',
      fields: null,
      warn: `${this.curScript?.framing.crc?.algo ?? 'CRC'} 校验失败`,
      frameHex: hexSpaced(bytes),
      frameLen: bytes.length,
      crcOk: false,
    }
  }

  // ---- 内部：批处理（live 泵批 + 回溯批共用） ----

  private schedulePump(): void {
    if (this.pumpScheduled) return
    this.pumpScheduled = true
    queueMicrotask(() => {
      this.pumpScheduled = false
      this.pump()
    })
  }

  /** host 空闲时取一个 session 队列整批发（reqId 递增、带发出时的 gen） */
  private pump(): void {
    if (!this.curScript || !this.enabledFlag || !this.host) return
    if (this.inflight) return
    for (const [sessionId, q] of this.queues) {
      if (q.length === 0) continue
      const items = q.splice(0, q.length)
      const itemByNo = groupByNo(items)
      void this.dispatch(items, sessionId).then((results) => {
        // 看门狗卸载（script=null）或 gen 不符 → dispatch 已返回 null
        if (results && this.curScript) {
          const frames: DecodedFrame[] = []
          this.consumeResults(results, itemByNo, frames, true)
          if (frames.length > 0) this.opts.onDecoded(sessionId, frames, false)
        }
      })
      return
    }
  }

  /** host 访问串行化入口；完成时补泵一次（回溯/前批期间积压的 live 队列） */
  private dispatch(items: EngineBatchItem[], sessionId: string): Promise<EngineBatchResult[] | null> {
    const task = this.dispatchChain.then(() => this.doDispatch(items, sessionId))
    this.dispatchChain = task.then(
      () => undefined,
      () => undefined,
    )
    void task.then(
      () => this.schedulePump(),
      () => this.schedulePump(),
    )
    return task
  }

  private doDispatch(items: EngineBatchItem[], sessionId: string): Promise<EngineBatchResult[] | null> {
    const host = this.host
    if (!host || !this.curScript || items.length === 0) return Promise.resolve(null)
    const reqId = ++this.reqCounter
    const gen = this.genOf(sessionId)
    return new Promise((resolve) => {
      let ackTimer: ReturnType<typeof setTimeout> | null = null
      let resTimer: ReturnType<typeof setTimeout> | null = null
      let settled = false
      const settle = (v: EngineBatchResult[] | null) => {
        if (settled) return
        settled = true
        if (ackTimer !== null) {
          clearTimeout(ackTimer)
          ackTimer = null
        }
        if (resTimer !== null) {
          clearTimeout(resTimer)
          resTimer = null
        }
        if (this.inflight && this.inflight.reqId === reqId) this.inflight = null
        resolve(v)
      }
      const onTimeout = () => {
        settle(null)
        this.onWatchdogTimeout()
      }
      // 看门狗：ack 3s；ack 到了才起 results 10s（host.run 的 resolve 即视为 results）
      ackTimer = setTimeout(onTimeout, ACK_TIMEOUT_MS)
      this.inflight = {
        reqId,
        sessionId,
        gen,
        onAck: () => {
          if (settled || ackTimer === null) return
          clearTimeout(ackTimer)
          ackTimer = null
          resTimer = setTimeout(onTimeout, RESULTS_TIMEOUT_MS)
        },
        settle,
      }
      host.run({ reqId, sessionId, items }).then(
        (res) => {
          const ok = res !== null && Array.isArray(res.items) && this.genOf(sessionId) === gen
          settle(ok ? res.items : null)
        },
        () => settle(null),
      )
    })
  }

  private handleAck(reqId: number): void {
    if (this.inflight && this.inflight.reqId === reqId) this.inflight.onAck()
  }

  /** 看门狗超时：dispose host + 卸载脚本 + 清全部 framer/gen（重建机会留给下次 load） */
  private onWatchdogTimeout(): void {
    this.disposeHost()
    this.curScript = null
    this.enabledFlag = false
    this.curBanner = { kind: 'timeout', msg: '脚本执行超时（疑似死循环），已自动卸载' }
    this.framers.clear()
    this.gens.clear()
    this.queues.clear()
    this.errWin.clear()
    this.opts.onStateChange()
  }

  /**
   * 结果 → DecodedFrame + 统计 + 熔断窗口。count=false（试运行）不进全局 stats/熔断。
   * 返回解析错误数（试运行 report 用）。
   */
  private consumeResults(
    results: EngineBatchResult[],
    itemByNo: Map<number, EngineBatchItem[]>,
    framesOut: DecodedFrame[],
    count: boolean,
  ): number {
    const script = this.curScript
    if (!script) return 0
    let errors = 0
    for (const r of results) {
      const item = takeItem(itemByNo, r.no)
      if (r.ok && r.result) {
        if (count) this.errWin.push(true)
        const type = r.result.type ?? script.meta.name
        if (count) this.typeNames.add(type)
        framesOut.push({
          no: r.no,
          ts: item?.ts ?? '',
          dir: item?.dir ?? 'rx',
          type,
          text: String(r.result.text ?? ''),
          fields: r.result.fields ?? null,
          warn: r.result.warn ?? null,
          frameHex: item ? hexSpaced(item.bytes) : '',
          frameLen: item?.bytes.length ?? 0,
          crcOk: true,
        })
        if (count) this.statsObj.ok++
      } else {
        errors++
        if (count) {
          this.errWin.push(false)
          this.statsObj.parseErrors++
        }
      }
    }
    // 错误率熔断（仅脚本层）
    if (count && this.errWin.shouldTrip()) this.trip()
    return errors
  }

  private trip(): void {
    this.enabledFlag = false
    this.curBanner = { kind: 'tripped', msg: '解析错误率超过 30%，已自动停用（可重新启用）' }
    this.errWin.clear()
    this.opts.onStateChange()
  }

  // ---- 内部：回溯管线（setEnabled(true) / trialRun 共用） ----

  /**
   * 每会话取最近 2000 行回溯：先 resetSession 语义（framer 清、gen+1）；
   * framer 状态写在正式 Map 里（feed 在 disabled 期间是 no-op，无并发写）。
   * count=true（正式启用）时计入 stats/熔断窗口，false（试运行）不动全局状态。
   */
  private async backfillSession(id: string, lines: LogLine[], count: boolean): Promise<BackfillOutcome> {
    this.framers.delete(id)
    this.queues.delete(id)
    this.bumpGen(id)
    const gen = this.genOf(id)
    const script = this.curScript
    const frames: DecodedFrame[] = []
    let framesCut = 0
    let crcTotal = 0
    let crcFailed = 0
    let parseErrors = 0
    let hasRx = false
    if (!script) return { frames, gen, scanned: 0, framesCut: 0, crcTotal: 0, crcFailed: 0, parseErrors: 0, hasRx }
    const declarative = !script.hasParse && script.fields !== null && script.fields.length > 0
    const window = lines.length > BACKFILL_LINES ? lines.slice(lines.length - BACKFILL_LINES) : lines.slice()
    const items: EngineBatchItem[] = []
    for (const line of window) {
      if (line.dir === 'rx') hasRx = true
      const st = this.framerFor(id, line.dir)
      const r = framerFeed(st, script.framing, lineBytes(line, script.framing.source))
      for (const f of r.frames) {
        framesCut++
        if (f.crcOk !== null) crcTotal++
        if (f.crcOk === false) {
          crcFailed++
          frames.push(this.crcFailFrame(line.no, line.ts, line.dir, f.bytes))
          continue
        }
        if (declarative) {
          const d = decodeDeclarative(script, f.bytes)
          if (count) {
            this.statsObj.ok++
            this.typeNames.add(d.type)
          }
          frames.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            type: d.type,
            text: d.text,
            fields: d.fields,
            warn: null,
            frameHex: hexSpaced(f.bytes),
            frameLen: f.bytes.length,
            crcOk: f.crcOk,
          })
        } else if (script.hasParse) {
          items.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            epochMillis: line.epochMillis,
            bytes: Array.from(f.bytes),
            sessionName: this.opts.getSessionName(id),
          })
        } else {
          // 纯切帧器：原始 hex 行
          const type = resolveTypeName(script, f.bytes)
          if (count) {
            this.statsObj.ok++
            this.typeNames.add(type)
          }
          frames.push({
            no: line.no,
            ts: line.ts,
            dir: line.dir,
            type,
            text: hexSpaced(f.bytes),
            fields: null,
            warn: null,
            frameHex: hexSpaced(f.bytes),
            frameLen: f.bytes.length,
            crcOk: f.crcOk,
          })
        }
      }
    }
    if (count) {
      this.statsObj.frames += framesCut
      this.statsObj.crcFailed += crcFailed
    }
    // 脚本层：回溯批与 live 队列分开记账，同样受看门狗/熔断约束
    if (items.length > 0) {
      const itemByNo = groupByNo(items)
      const results = await this.dispatch(items, id)
      if (results) parseErrors = this.consumeResults(results, itemByNo, frames, count)
    }
    return { frames, gen, scanned: window.length, framesCut, crcTotal, crcFailed, parseErrors, hasRx }
  }
}
