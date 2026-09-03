/**
 * 解析引擎协议测试（engine + bootstrap 契约，计划 §6 engine-protocol 行）。
 * 不 mock Worker：hostFactory / importModule / makeBlobUrl 全部注入 fake，
 * 看门狗/泵批时序用 vi.useFakeTimers 控制（node 环境可测，引擎不依赖 Vue/store）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ParserEngine, staticHasParse, validateDecl, classifyTrial, ErrorRateWindow } from '../engine'
import type { EngineBatchItem, EngineBatchResult, EngineHost } from '../engine'
import { createScriptHost } from '../bootstrap'
import { computeCrc } from '../crc'
import type { LogLine } from '../../types'
import type { DecodedFrame } from '../../types/parser'

// ---- fixtures ----

/** 温控声明式脚本源码（无 parse，静态扫描不命中） */
const DECL_SRC = `
export default {
  meta: { name: '温控协议', version: '1.0' },
  framing: {
    source: 'binary',
    sync: 'AA 55',
    length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
    crc: { algo: 'crc16-modbus', at: 'tail:2' },
    maxSize: 64,
  },
  type: { at: 2, fmt: 'u8', map: { 1: '状态上报', 2: '设置响应' } },
  fields: [
    { label: '温度', at: 4, fmt: 'i16', scale: 0.1, unit: '℃' },
    { label: '模式', at: 6, fmt: 'u8', map: { 0: '待机', 1: '制冷' } },
  ],
  text: '温度 {温度}℃，模式{模式}',
}
`

/** 含脚本层 parse 的源码（静态扫描命中） */
const PARSE_SRC = `
export default {
  meta: { name: '温控协议', version: '1.0' },
  framing: { source: 'binary', sync: 'AA 55', length: { kind: 'field', at: 3, fmt: 'u8', add: 4 }, crc: { algo: 'crc16-modbus', at: 'tail:2' } },
  parse(frame, ctx) { return { type: '脚本', text: ctx.sessionName + ' len=' + frame.length } },
}
`

/** 纯切帧器：定长 2 字节、无 sync、无 CRC、无 fields/parse */
const PURE_SRC = `
export default {
  meta: { name: '定长协议', version: '1' },
  framing: { source: 'binary', length: { kind: 'fixed', value: 2 } },
}
`

/** 脚本层 + line 切帧（一行一帧，无 CRC）：队列/熔断批量构造用 */
const LINE_PARSE_SRC = `
export default {
  meta: { name: '行协议', version: '1' },
  framing: { source: 'binary', length: { kind: 'line' } },
  parse(frame) { return { type: '行', text: 'len=' + frame.length } },
}
`

/** DECL_SRC 对应的纯数据 decl（fake importModule 的返回值；ESM 求值不在本测试范围） */
function tcDecl(): Record<string, unknown> {
  return {
    meta: { name: '温控协议', version: '1.0' },
    framing: {
      source: 'binary',
      sync: 'AA 55',
      length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
      crc: { algo: 'crc16-modbus', at: 'tail:2' },
      maxSize: 64,
    },
    type: { at: 2, fmt: 'u8', map: { 1: '状态上报', 2: '设置响应' } },
    fields: [
      { label: '温度', at: 4, fmt: 'i16', scale: 0.1, unit: '℃' },
      { label: '模式', at: 6, fmt: 'u8', map: { 0: '待机', 1: '制冷' } },
    ],
    text: '温度 {温度}℃，模式{模式}',
  }
}

function pureDecl(): Record<string, unknown> {
  return {
    meta: { name: '定长协议', version: '1' },
    framing: { source: 'binary', length: { kind: 'fixed', value: 2 } },
  }
}

function lineDecl(): Record<string, unknown> {
  return {
    meta: { name: '行协议', version: '1' },
    framing: { source: 'binary', length: { kind: 'line' } },
  }
}

/** 温控帧：sync AA 55 + 类型@2 + 长度域@3(=payload+2) + payload + CRC16-modbus(tail:2, little) */
function mkTc(type: number, payload: number[], badCrc = false): number[] {
  const body = [0xaa, 0x55, type, payload.length + 2, ...payload]
  let crc = computeCrc('crc16-modbus', new Uint8Array(body))
  if (badCrc) crc = (crc + 1) & 0xffff
  return [...body, crc & 0xff, (crc >> 8) & 0xff]
}

const hexOf = (arr: number[]) =>
  arr.map((b) => b.toString(16).toUpperCase().padStart(2, '0')).join(' ')

let lineSeq = 0
function mkLine(text: string, dir: 'rx' | 'tx' = 'rx', bytesArr: number[] | null = null): LogLine {
  lineSeq++
  return {
    no: lineSeq,
    ts: `12:00:0${lineSeq % 10}.000`,
    dir,
    text,
    bytes: bytesArr,
    epochMillis: 1700000000000 + lineSeq,
  }
}

/** 温度=25℃、模式=制冷 的标准 RX 帧 */
const GOOD_FRAME = (): number[] => mkTc(1, [0xfa, 0x00, 0x01])

// ---- fake host / engine 装配 ----

type HangMode = 'none' | 'no-ack' | 'no-results'

class FakeHost implements EngineHost {
  batches: { reqId: number; sessionId: string; items: EngineBatchItem[] }[] = []
  disposed = 0
  hang: HangMode = 'none'
  loadImpl: (src: string) => Promise<{ decl: unknown }> = async () => ({ decl: {} })
  onAck?: (reqId: number) => void
  private waiters = new Map<number, (v: { reqId: number; items: EngineBatchResult[] }) => void>()

  load(src: string): Promise<{ decl: unknown }> {
    return this.loadImpl(src)
  }

  run(batch: { reqId: number; sessionId: string; items: EngineBatchItem[] }): Promise<{ reqId: number; items: EngineBatchResult[] }> {
    this.batches.push(batch)
    if (this.hang === 'no-ack') return new Promise(() => {})
    this.onAck?.(batch.reqId)
    if (this.hang === 'no-results') return new Promise(() => {})
    return new Promise((resolve) => {
      this.waiters.set(batch.reqId, resolve)
    })
  }

  /** 测试手动回包（模拟 worker 的 results 消息） */
  resolve(reqId: number, items: EngineBatchResult[]): void {
    const w = this.waiters.get(reqId)
    this.waiters.delete(reqId)
    w?.({ reqId, items })
  }

  dispose(): void {
    this.disposed++
  }
}

interface Setup {
  engine: ParserEngine
  host: FakeHost
  decoded: { sessionId: string; frames: DecodedFrame[]; replace: boolean }[]
}

function setup(o: { decl?: unknown; withHost?: boolean; importThrows?: boolean } = {}): Setup {
  const decoded: Setup['decoded'] = []
  const host = new FakeHost()
  host.loadImpl = async () => ({ decl: o.decl ?? {} })
  const engine = new ParserEngine({
    hostFactory: o.withHost ? () => host : () => null,
    importModule: async () => {
      if (o.importThrows) throw new Error('blob import failed')
      return { default: o.decl ?? {} }
    },
    makeBlobUrl: () => 'blob:test',
    onDecoded: (sessionId, frames, replace) => decoded.push({ sessionId, frames, replace }),
    onStateChange: () => {},
    getSessionName: (id) => `会话${id}`,
  })
  return { engine, host, decoded }
}

/**
 * fake timers 下刷微任务（泵批/结果消费都是微任务级）。
 * 注意 toFake 不含 queueMicrotask——引擎泵批走 queueMicrotask，必须保持真微任务语义。
 */
function useFakeClock(): void {
  vi.useFakeTimers({
    toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'setImmediate', 'clearImmediate', 'Date'],
  })
}

async function adv(ms = 1): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms)
  await vi.advanceTimersByTimeAsync(ms)
}

beforeEach(() => {
  lineSeq = 0
})
afterEach(() => {
  vi.useRealTimers()
})

// ---- 1. staticHasParse ----

describe('staticHasParse 静态扫描', () => {
  it('parse( / parse: / parse = 命中', () => {
    expect(staticHasParse('export default { parse(frame, ctx) {} }')).toBe(true)
    expect(staticHasParse('export default { parse: parseFrame }')).toBe(true)
    expect(staticHasParse('obj.parse = function (f) {}')).toBe(true)
    expect(staticHasParse('parse: (frame, ctx) => ({})')).toBe(true)
  })
  it('纯声明式源码与相近标识符不命中', () => {
    expect(staticHasParse(DECL_SRC)).toBe(false)
    expect(staticHasParse(PURE_SRC)).toBe(false)
    expect(staticHasParse('const parser = createParser()')).toBe(false)
    expect(staticHasParse('parseHexField(s)')).toBe(false)
  })
})

// ---- 2. validateDecl ----

describe('validateDecl schema 校验', () => {
  function brokenDecl(mut: (d: Record<string, any>) => void): unknown {
    const d = tcDecl() as Record<string, any>
    mut(d)
    return d
  }

  it('缺 meta 拒绝', () => {
    const r = validateDecl({ framing: tcDecl().framing }, DECL_SRC)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('meta')
  })
  it('未知 length.kind 拒绝', () => {
    const r = validateDecl(brokenDecl((d) => {
      d.framing.length = { kind: 'weird' }
    }), DECL_SRC)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('kind')
  })
  it('field 长度域缺 add 拒绝', () => {
    const r = validateDecl(brokenDecl((d) => {
      d.framing.length = { kind: 'field', at: 3, fmt: 'u8' }
    }), DECL_SRC)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('add')
  })
  it('fields at 为负拒绝', () => {
    const r = validateDecl(brokenDecl((d) => {
      d.fields = [{ label: 'x', at: -1, fmt: 'u8' }]
    }), DECL_SRC)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('at')
  })
  it('crc 非法 algo 拒绝', () => {
    const r = validateDecl(brokenDecl((d) => {
      d.framing.crc = { algo: 'crc16', at: 'tail:2' }
    }), DECL_SRC)
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('CRC')
  })
  it('合法声明式脚本通过且 framing 归一化', () => {
    const r = validateDecl(tcDecl(), DECL_SRC)
    expect(r.ok).toBe(true)
    if (r.ok) {
      expect(r.script.hasParse).toBe(false)
      expect(r.script.meta.name).toBe('温控协议')
      expect(r.script.framing.sync).toEqual(new Uint8Array([0xaa, 0x55]))
      expect(r.script.framing.length).toEqual({ kind: 'field', at: 3, fmt: 'u8', endian: 'little', add: 4 })
      expect(r.script.framing.crc).toEqual({ algo: 'crc16-modbus', n: 2, endian: 'little' })
      expect(r.script.framing.maxSize).toBe(64)
      // 字段缺省值：scale/offset/endian、map 透传
      expect(r.script.fields![0]!.scale).toBe(0.1)
      expect(r.script.fields![0]!.offset).toBe(0)
      expect(r.script.fields![0]!.endian).toBe('little')
      expect(r.script.fields![1]!.map).toEqual({ 0: '待机', 1: '制冷' })
      expect(r.script.text).toContain('{温度}')
    }
  })
  it('脚本层源码 hasParse=true', () => {
    const r = validateDecl(tcDecl(), PARSE_SRC)
    expect(r.ok && r.script.hasParse).toBe(true)
  })
})

// ---- 3. load 三路径 ----

describe('load 三路径', () => {
  it('声明式成功：零 Worker 装载，不自动启用', async () => {
    const s = setup({ decl: tcDecl() })
    const r = await s.engine.load(DECL_SRC)
    expect(r.ok).toBe(true)
    expect(s.engine.loaded).toBe(true)
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.banner).toBeNull()
    expect(s.engine.meta?.name).toBe('温控协议')
    expect(s.engine.hasParseScript()).toBe(false)
  })
  it('声明式 import 抛错 → banner error', async () => {
    const s = setup({ decl: tcDecl(), importThrows: true })
    const r = await s.engine.load(DECL_SRC)
    expect(r.ok).toBe(false)
    expect(s.engine.loaded).toBe(false)
    expect(s.engine.banner?.kind).toBe('error')
  })
  it('脚本层 hostFactory=null → 无法创建脚本沙箱', async () => {
    const s = setup({ decl: tcDecl() })
    const r = await s.engine.load(PARSE_SRC)
    expect(r.ok).toBe(false)
    expect(r.error).toContain('无法创建脚本沙箱')
    expect(s.engine.banner?.kind).toBe('error')
    expect(s.engine.loaded).toBe(false)
  })
  it('脚本层 loadError → banner error 且 host 释放', async () => {
    const s = setup({ withHost: true })
    s.host.loadImpl = async () => {
      throw new Error('SyntaxError: bad script')
    }
    const r = await s.engine.load(PARSE_SRC)
    expect(r.ok).toBe(false)
    expect(s.engine.banner?.kind).toBe('error')
    expect(s.host.disposed).toBe(1)
  })
  it('脚本层 decl 非法（缺 framing）→ schema 拦截', async () => {
    const s = setup({ withHost: true })
    s.host.loadImpl = async () => ({ decl: { meta: { name: 'x', version: '1' } } })
    const r = await s.engine.load(PARSE_SRC)
    expect(r.ok).toBe(false)
    expect(s.engine.banner?.kind).toBe('error')
    expect(s.host.disposed).toBe(1)
  })
  it('脚本层成功装载：hasParseScript=true，ack 钩子挂上', async () => {
    const s = setup({ withHost: true, decl: tcDecl() })
    const r = await s.engine.load(PARSE_SRC)
    expect(r.ok).toBe(true)
    expect(s.engine.hasParseScript()).toBe(true)
    expect(s.engine.enabled).toBe(false)
    expect(typeof s.host.onAck).toBe('function')
  })
})

// ---- 4. 声明式 feed 全链路 ----

describe('声明式 feed 全链路', () => {
  it('sync+长度域+CRC：正常翻译 + CRC 失败行，stats 正确', async () => {
    const s = setup({ decl: tcDecl() })
    await s.engine.load(DECL_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    const good = mkLine('rx', 'rx', GOOD_FRAME())
    const bad = mkLine('rx', 'rx', mkTc(1, [0xfa, 0x00, 0x01], true))
    s.engine.feed('s1', [good, bad])
    // 声明式同步出结果
    expect(s.decoded).toHaveLength(2) // enable 空表 replace + feed 追加
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.sessionId).toBe('s1')
    expect(batch.replace).toBe(false)
    expect(batch.frames).toHaveLength(2)
    const ok = batch.frames[0]!
    expect(ok.no).toBe(good.no)
    expect(ok.type).toBe('状态上报')
    expect(ok.text).toBe('温度 25℃，模式制冷')
    expect(ok.crcOk).toBe(true)
    expect(ok.fields?.map((f) => f.label)).toEqual(['温度', '模式'])
    expect(ok.frameHex).toBe(hexOf(GOOD_FRAME()))
    expect(ok.frameLen).toBe(GOOD_FRAME().length)
    const badf = batch.frames[1]!
    expect(badf.type).toBe('CRC 错误')
    expect(badf.text).toBe('帧校验失败，未翻译')
    expect(badf.crcOk).toBe(false)
    expect(badf.warn).toContain('crc16-modbus')
    expect(badf.fields).toBeNull()
    const st = s.engine.stats
    expect(st.frames).toBe(2)
    expect(st.ok).toBe(1)
    expect(st.crcFailed).toBe(1)
    expect(st.types).toBe(1)
  })
  it('纯切帧器：出原始 hex 行（大写空格分隔，无 fields）', async () => {
    const s = setup({ decl: pureDecl() })
    await s.engine.load(PURE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    s.engine.feed('s1', [mkLine('x', 'rx', [0xde, 0xad])])
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.frames).toHaveLength(1)
    const f = batch.frames[0]!
    expect(f.type).toBe('定长协议')
    expect(f.text).toBe('DE AD')
    expect(f.frameHex).toBe('DE AD')
    expect(f.fields).toBeNull()
    expect(f.crcOk).toBeNull()
    expect(s.engine.stats.ok).toBe(1)
  })
})

// ---- 5. 脚本层 feed ----

describe('脚本层 feed（Worker 批处理）', () => {
  it('结果异步到达：type 缺省回退 meta.name，会话名透传', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: tcDecl() })
    await s.engine.load(PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    const line = mkLine('rx', 'rx', GOOD_FRAME())
    s.engine.feed('s1', [line])
    await adv()
    expect(s.host.batches).toHaveLength(1)
    const b = s.host.batches[0]!
    expect(b.items).toHaveLength(1)
    expect(b.items[0]!.no).toBe(line.no)
    expect(b.items[0]!.sessionName).toBe('会话s1')
    expect(b.items[0]!.bytes).toEqual(GOOD_FRAME())
    s.host.resolve(b.reqId, [{ no: line.no, ok: true, result: { text: '在线' } }])
    await adv()
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.replace).toBe(false)
    expect(batch.frames).toHaveLength(1)
    const f = batch.frames[0]!
    expect(f.type).toBe('温控协议') // result.type 缺省 → meta.name
    expect(f.text).toBe('在线')
    expect(f.crcOk).toBe(true)
    expect(f.frameHex).toBe(hexOf(GOOD_FRAME()))
    expect(s.engine.stats.ok).toBe(1)
    expect(s.engine.stats.parseErrors).toBe(0)
  })
  it('parse 抛错/返回 null → parseErrors++，不产出行、不崩', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: lineDecl() })
    await s.engine.load(LINE_PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    // 混合批：1 错 + 9 好（错误率 10% < 30%，不触发熔断）
    const lines = Array.from({ length: 10 }, () => mkLine('x', 'rx', [0x41, 0x0a]))
    s.engine.feed('s1', lines)
    await adv()
    const b = s.host.batches[0]!
    s.host.resolve(b.reqId, b.items.map((it, i) => ({ no: it.no, ok: i > 0, result: { type: '行', text: 'x' } })))
    await adv()
    expect(s.engine.stats.parseErrors).toBe(1)
    expect(s.engine.stats.ok).toBe(9)
    // 错误项不产出行：10 帧只出 9 行
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.frames).toHaveLength(9)
    expect(batch.frames.every((f) => f.no !== lines[0]!.no)).toBe(true)
    expect(s.engine.enabled).toBe(true)
  })
  it('队列超限：丢最旧并计 dropped', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: lineDecl() })
    await s.engine.load(LINE_PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    const lines = Array.from({ length: 1005 }, () => mkLine('x', 'rx', [0x41, 0x0a]))
    s.engine.feed('s1', lines)
    await adv()
    const b = s.host.batches[0]!
    expect(b.items).toHaveLength(1000)
    expect(s.engine.stats.frames).toBe(1005)
    expect(s.engine.stats.dropped).toBe(5)
    s.host.resolve(b.reqId, b.items.map((it) => ({ no: it.no, ok: true, result: { type: '行', text: 'x' } })))
    await adv()
    expect(s.engine.stats.ok).toBe(1000)
    expect(s.decoded[s.decoded.length - 1]!.frames).toHaveLength(1000)
  })
})

// ---- 6. gen 代际 ----

describe('gen 代际：迟到结果丢弃', () => {
  it('resetSession 后旧批结果不产出行、不进 stats', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: tcDecl() })
    await s.engine.load(PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    const line = mkLine('rx', 'rx', mkTc(1, [0x01]))
    s.engine.feed('s1', [line])
    await adv()
    const b = s.host.batches[0]!
    s.engine.resetSession('s1')
    s.host.resolve(b.reqId, [{ no: line.no, ok: true, result: { type: '迟到的', text: '应被丢弃' } }])
    await adv()
    expect(s.decoded).toHaveLength(1) // 只有 enable 的空表 replace
    expect(s.engine.stats.ok).toBe(0)
    expect(s.engine.stats.parseErrors).toBe(0)
  })
})

// ---- 7. 看门狗 ----

describe('看门狗（按批超时）', () => {
  it('ack 永不到达 → 3s 超时：dispose + 卸载脚本 + banner timeout', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: tcDecl() })
    s.host.hang = 'no-ack'
    await s.engine.load(PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    s.engine.feed('s1', [mkLine('rx', 'rx', mkTc(1, [0x01]))])
    await adv()
    expect(s.host.batches).toHaveLength(1)
    expect(s.engine.loaded).toBe(true)
    await adv(3000)
    expect(s.engine.loaded).toBe(false)
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.script).toBeNull()
    expect(s.engine.banner?.kind).toBe('timeout')
    expect(s.engine.banner?.msg).toContain('超时')
    expect(s.host.disposed).toBeGreaterThanOrEqual(1)
  })
  it('ack 到达但 results 超时 → 10s 后卸载', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: tcDecl() })
    s.host.hang = 'no-results'
    await s.engine.load(PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    s.engine.feed('s1', [mkLine('rx', 'rx', mkTc(1, [0x01]))])
    await adv()
    await adv(3000) // ack 已到，results 看门狗才刚起
    expect(s.engine.loaded).toBe(true)
    await adv(10000)
    expect(s.engine.loaded).toBe(false)
    expect(s.engine.banner?.kind).toBe('timeout')
    expect(s.host.disposed).toBeGreaterThanOrEqual(1)
  })
})

// ---- 8. 错误率熔断 ----

describe('错误率熔断', () => {
  it('ErrorRateWindow：300/1000 不触发、301/1000 触发、clear 归零', () => {
    const w = new ErrorRateWindow()
    for (let i = 0; i < 700; i++) w.push(true)
    for (let i = 0; i < 300; i++) w.push(false)
    expect(w.shouldTrip()).toBe(false) // 300/1000 = 30%，不越阈
    w.push(false) // 挤出最旧一条 ok → 301/1000
    expect(w.shouldTrip()).toBe(true)
    w.clear()
    expect(w.shouldTrip()).toBe(false)
  })
  it('引擎层：3 错 7 好不触发；错误攒够 → enabled=false + banner tripped（脚本保留）', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: lineDecl() })
    await s.engine.load(LINE_PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    // 第一批：3 错 7 好（3/10 = 30%，不越阈）
    s.engine.feed('s1', Array.from({ length: 10 }, () => mkLine('x', 'rx', [0x41, 0x0a])))
    await adv()
    let b = s.host.batches[0]!
    s.host.resolve(b.reqId, b.items.map((it, i) => ({ no: it.no, ok: i >= 3, result: { type: '行', text: 'x' } })))
    await adv()
    expect(s.engine.enabled).toBe(true)
    expect(s.engine.stats.parseErrors).toBe(3)
    expect(s.engine.stats.ok).toBe(7)
    // 第二批：291 错 → 窗口 301 条 / 294 错（>30%）→ 熔断
    s.engine.feed('s1', Array.from({ length: 291 }, () => mkLine('x', 'rx', [0x42, 0x0a])))
    await adv()
    b = s.host.batches[1]!
    s.host.resolve(b.reqId, b.items.map((it) => ({ no: it.no, ok: false })))
    await adv()
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.banner?.kind).toBe('tripped')
    expect(s.engine.banner?.msg).toContain('30%')
    expect(s.engine.loaded).toBe(true) // 脚本保留，可重新启用
  })
  it('熔断后重新启用：窗口清零，恢复正常解析', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: lineDecl() })
    await s.engine.load(LINE_PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    // 喂 301 错触发熔断
    s.engine.feed('s1', Array.from({ length: 301 }, () => mkLine('x', 'rx', [0x41, 0x0a])))
    await adv()
    const b1 = s.host.batches[0]!
    s.host.resolve(b1.reqId, b1.items.map((it) => ({ no: it.no, ok: false })))
    await adv()
    expect(s.engine.enabled).toBe(false)
    // 重新启用
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    expect(s.engine.enabled).toBe(true)
    expect(s.engine.banner).toBeNull()
    expect(s.engine.stats.frames).toBe(0)
    s.engine.feed('s1', [mkLine('x', 'rx', [0x42, 0x0a])])
    await adv()
    const b2 = s.host.batches[s.host.batches.length - 1]!
    s.host.resolve(b2.reqId, b2.items.map((it) => ({ no: it.no, ok: true, result: { type: '行', text: 'ok' } })))
    await adv()
    expect(s.engine.enabled).toBe(true)
    expect(s.engine.stats.ok).toBe(1)
  })
})

// ---- 9. classifyTrial + trialRun ----

describe('classifyTrial 三分类', () => {
  it('no-data / suspect / ok', () => {
    expect(classifyTrial(false, 5, 5, 0)).toBe('no-data')
    expect(classifyTrial(true, 0, 0, 0)).toBe('suspect')
    expect(classifyTrial(true, 5, 5, 5)).toBe('suspect')
    expect(classifyTrial(true, 5, 5, 2)).toBe('ok')
    expect(classifyTrial(true, 3, 0, 0)).toBe('ok') // 未配置 CRC
  })
})

describe('trialRun 试运行', () => {
  it('声明式：verdict ok + samples（activeId 会话，hex→type:text），不改变 enabled', async () => {
    const s = setup({ decl: tcDecl() })
    await s.engine.load(DECL_SRC)
    const lines = [mkLine('rx', 'rx', GOOD_FRAME()), mkLine('rx', 'rx', GOOD_FRAME()), mkLine('rx', 'rx', GOOD_FRAME())]
    const report = await s.engine.trialRun('s1', [{ id: 's1', lines }])
    expect(report.verdict).toBe('ok')
    expect(report.lines).toBe(3)
    expect(report.frames).toBe(3)
    expect(report.crcFailed).toBe(0)
    expect(report.parseErrors).toBe(0)
    expect(report.samples).toHaveLength(3)
    expect(report.samples[0]!.hex).toBe(hexOf(GOOD_FRAME()))
    expect(report.samples[0]!.type).toBe('状态上报')
    expect(report.samples[0]!.text).toContain('温度 25℃')
    expect(s.engine.trialReport?.verdict).toBe('ok')
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.stats.frames).toBe(0) // 试运行不进全局 stats
  })
  it('全 CRC 失败 → suspect；无 RX → no-data', async () => {
    const s = setup({ decl: tcDecl() })
    await s.engine.load(DECL_SRC)
    const bad = await s.engine.trialRun('s1', [{ id: 's1', lines: [mkLine('rx', 'rx', mkTc(1, [0x01], true))] }])
    expect(bad.verdict).toBe('suspect')
    expect(bad.frames).toBe(1)
    expect(bad.crcFailed).toBe(1)
    const nodata = await s.engine.trialRun('s2', [{ id: 's2', lines: [mkLine('tx', 'tx', mkTc(1, [0x01]))] }])
    expect(nodata.verdict).toBe('no-data')
  })
  it('脚本层：parseErrors 进 report 不参与 verdict，也不进全局 stats', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: lineDecl() })
    await s.engine.load(LINE_PARSE_SRC)
    const lines = [mkLine('a', 'rx', [0x41, 0x0a]), mkLine('b', 'rx', [0x42, 0x0a])]
    const p = s.engine.trialRun('s1', [{ id: 's1', lines }])
    await adv()
    const b = s.host.batches[0]!
    expect(b.items).toHaveLength(2)
    s.host.resolve(b.reqId, b.items.map((it) => ({ no: it.no, ok: false })))
    const report = await p
    expect(report.verdict).toBe('ok') // 有帧即 ok，解析错误不参与 verdict
    expect(report.frames).toBe(2)
    expect(report.parseErrors).toBe(2)
    expect(s.engine.stats.parseErrors).toBe(0)
  })
})

// ---- 10. setEnabled(true) 回溯 ----

describe('setEnabled(true) 回溯', () => {
  it('最近 2000 行截断 + replace=true 整表替换 + stats 计入', async () => {
    const s = setup({ decl: pureDecl() })
    await s.engine.load(PURE_SRC)
    const lines = Array.from({ length: 2010 }, () => mkLine('x', 'rx', [0xde, 0xad]))
    await s.engine.setEnabled(true, [{ id: 's1', lines }])
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.replace).toBe(true)
    expect(batch.frames).toHaveLength(2000)
    expect(batch.frames[0]!.no).toBe(lines[10]!.no) // 前 10 行被截掉
    expect(batch.frames[1999]!.no).toBe(lines[2009]!.no)
    expect(batch.frames[0]!.text).toBe('DE AD')
    const st = s.engine.stats
    expect(st.frames).toBe(2000)
    expect(st.ok).toBe(2000)
    expect(st.types).toBe(1)
    // 启用后 live feed 继续追加（replace=false）
    s.engine.feed('s1', [mkLine('x', 'rx', [0xde, 0xad])])
    const live = s.decoded[s.decoded.length - 1]!
    expect(live.replace).toBe(false)
    expect(live.frames).toHaveLength(1)
  })
  it('无帧也 replace（清空旧表）', async () => {
    const s = setup({ decl: pureDecl() })
    await s.engine.load(PURE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [mkLine('', 'rx', [])] }])
    const batch = s.decoded[s.decoded.length - 1]!
    expect(batch.replace).toBe(true)
    expect(batch.frames).toHaveLength(0)
  })
})

// ---- 11. 卸载 / 停用 / bootstrap 冒烟 ----

describe('unload / 停用', () => {
  it('unload：停用 + 丢脚本 + 清空 decoded（replace 空表）+ host 释放', async () => {
    useFakeClock()
    const s = setup({ withHost: true, decl: tcDecl() })
    await s.engine.load(PARSE_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [] }])
    await s.engine.unload([{ id: 's1', lines: [] }])
    expect(s.engine.loaded).toBe(false)
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.banner).toBeNull()
    expect(s.engine.trialReport).toBeNull()
    expect(s.engine.stats.frames).toBe(0)
    expect(s.host.disposed).toBe(1)
    const last = s.decoded[s.decoded.length - 1]!
    expect(last.replace).toBe(true)
    expect(last.frames).toHaveLength(0)
  })
  it('停用：banner/framer 清空、不再 feed、decoded 保留', async () => {
    const s = setup({ decl: tcDecl() })
    await s.engine.load(DECL_SRC)
    await s.engine.setEnabled(true, [{ id: 's1', lines: [mkLine('rx', 'rx', GOOD_FRAME())] }])
    const before = s.decoded.length
    expect(before).toBe(1)
    await s.engine.setEnabled(false, [])
    expect(s.engine.enabled).toBe(false)
    expect(s.engine.banner).toBeNull()
    s.engine.feed('s1', [mkLine('rx', 'rx', GOOD_FRAME())])
    expect(s.decoded.length).toBe(before) // 停用后 feed 是 no-op，decoded 保留
  })
})

describe('bootstrap.createScriptHost', () => {
  it('node 环境（无 Worker 全局）返回 null', () => {
    // vitest environment=node：无 DOM Worker；Blob/URL 由 node 18+ 提供，守卫按 Worker 缺失拒绝
    expect(typeof Worker).toBe('undefined')
    expect(createScriptHost()).toBeNull()
    expect(createScriptHost(() => {})).toBeNull()
  })
})
