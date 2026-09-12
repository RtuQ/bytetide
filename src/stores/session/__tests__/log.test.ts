import { describe, it, expect } from 'vitest'
import { reactive, isReactive } from 'vue'
import { createSession } from '../model'
import {
  appendLinesInto,
  appendPulledInto,
  prependBackfillInto,
  takeEvictedFrom,
  takeBackfilledFrom,
  tallyBytesInto,
  applyDecodedInto,
  resetDecodedOf,
  MAX_DECODED,
} from '../log'
import type { LogLine, PortConfig, RawLogLine } from '../../../types'
import type { DecodedFrame } from '../../../types/parser'

function makeConfig(): PortConfig {
  return {
    transport: 'serial',
    name: 'COM-TEST',
    baudRate: 115200,
    dataBits: 8,
    parity: 'none',
    stopBits: '1',
    flowControl: 'none',
  }
}

function mkRaw(epoch: number, dir: 'rx' | 'tx' = 'rx'): RawLogLine {
  return { ts: '00:00:00.000', dir, text: `l${epoch}`, bytes: null, epochMillis: epoch }
}

function mkPulled(ringNo: number): RawLogLine & { ringNo: number } {
  return { ...mkRaw(ringNo), ringNo }
}

function mkDecoded(no: number): DecodedFrame {
  return {
    no,
    ts: '00:00:01.000',
    dir: 'rx',
    type: '状态上报',
    text: `d${no}`,
    fields: [],
    warn: null,
    frameHex: 'AA 55',
    frameLen: 2,
    crcOk: true,
  }
}

describe('appendLinesInto（事件流入表 + 水位去重 + cap 裁剪）', () => {
  it('行号自增入表、返回本次插入行', () => {
    const s = createSession('s1', makeConfig())
    const fresh = appendLinesInto(s, [mkRaw(1), mkRaw(2)], 100)
    expect(fresh.map((l) => l.no)).toEqual([1, 2])
    expect(s.lines.map((l) => l.epochMillis)).toEqual([1, 2])
    expect(s.lineCounter).toBe(2)
    expect(s.droppedLines).toBe(0)
  })

  it('补拉水位以下的迟到行被丢弃（防补拉重复）', () => {
    const s = createSession('s1', makeConfig())
    appendLinesInto(s, [mkRaw(100)], 100)
    s.pulledThrough = 5000
    const fresh = appendLinesInto(s, [mkRaw(4800), mkRaw(5000), mkRaw(5200)], 100)
    expect(fresh.map((l) => l.epochMillis)).toEqual([5200])
    expect(s.lines).toHaveLength(2)
  })

  it('超 cap 从头部裁剪并累计 droppedLines/evictedPending', () => {
    const s = createSession('s1', makeConfig())
    appendLinesInto(s, Array.from({ length: 15 }, (_, i) => mkRaw(i + 1)), 10)
    expect(s.lines).toHaveLength(10)
    expect(s.droppedLines).toBe(5)
    expect(s.evictedPending).toBe(5)
    // 同 tick 多批次累计不丢
    appendLinesInto(s, [mkRaw(100)], 10)
    expect(s.evictedPending).toBe(6)
    expect(takeEvictedFrom(s)).toBe(6)
    expect(s.evictedPending).toBe(0)
  })

  it('空批次直接返回空、不动状态', () => {
    const s = createSession('s1', makeConfig())
    expect(appendLinesInto(s, [], 10)).toEqual([])
    expect(s.lineCounter).toBe(0)
  })

  it('markRaw 纪律：入 reactive 会话的行元素保持非响应式（长跑红线）', () => {
    const s = reactive(createSession('s1', makeConfig()))
    appendLinesInto(s, [mkRaw(1)], 100)
    expect(isReactive(s.lines[0])).toBe(false)
  })
})

describe('appendPulledInto（拉模型游标摄取）', () => {
  it('按 ringNo 升序入表、游标推进到最新、行号全局单调', () => {
    const s = createSession('s1', makeConfig())
    const fresh = appendPulledInto(s, [mkPulled(3), mkPulled(7), mkPulled(9)], 100)
    expect(fresh.map((l) => l.no)).toEqual([1, 2, 3])
    expect(s.pullNo).toBe(9)
    // rn=后端 ring no，随行携带
    expect((s.lines[0] as LogLine).rn).toBe(3)
  })

  it('游标防御：ringNo <= pullNo 的重复拉取不入表、不推进游标', () => {
    const s = createSession('s1', makeConfig())
    appendPulledInto(s, [mkPulled(1), mkPulled(2)], 100)
    const fresh = appendPulledInto(s, [mkPulled(1), mkPulled(2), mkPulled(5)], 100)
    expect(fresh.map((l) => l.epochMillis)).toEqual([5])
    expect(s.lines).toHaveLength(3)
    expect(s.pullNo).toBe(5)
  })

  it('ring 缺口检测（去重过滤之前）：首行跳变即累计 ringDropped', () => {
    const s = createSession('s1', makeConfig())
    appendPulledInto(s, [mkPulled(5), mkPulled(6)], 100)
    expect(s.ringDropped).toBe(4)
    // 7..9 被覆盖再计 3
    appendPulledInto(s, [mkPulled(10)], 100)
    expect(s.ringDropped).toBe(7)
  })

  it('纯重复拉取不误报缺口', () => {
    const s = createSession('s1', makeConfig())
    appendPulledInto(s, [mkPulled(1), mkPulled(2)], 100)
    appendPulledInto(s, [mkPulled(1), mkPulled(2)], 100)
    expect(s.ringDropped).toBe(0)
    expect(s.pullNo).toBe(2)
  })

  it('超 cap 裁剪并累计缺口计数；cap 传入显式（门面算 Math.max(1, viewBufCap)）', () => {
    const s = createSession('s1', makeConfig())
    appendPulledInto(s, Array.from({ length: 30 }, (_, i) => mkPulled(i + 1)), 20)
    expect(s.lines).toHaveLength(20)
    expect(s.droppedLines).toBe(10)
    expect(s.evictedPending).toBe(10)
    expect(s.pullNo).toBe(30)
  })

  it('空批次返回空且不推进游标、不误报缺口', () => {
    const s = createSession('s1', makeConfig())
    expect(appendPulledInto(s, [], 10)).toEqual([])
    expect(s.pullNo).toBe(0)
    expect(s.ringDropped).toBe(0)
  })

  it('markRaw 纪律：拉取行入 reactive 会话保持非响应式', () => {
    const s = reactive(createSession('s1', makeConfig()))
    appendPulledInto(s, [mkPulled(1)], 100)
    expect(isReactive(s.lines[0])).toBe(false)
  })
})

describe('prependBackfillInto（翻页补旧行）', () => {
  it('回补行沿用被裁前原行号插到头部，不推进 lineCounter/pullNo，累计 backfillTotal', () => {
    const s = createSession('s1', makeConfig())
    appendPulledInto(s, Array.from({ length: 10 }, (_, i) => mkPulled(i + 1)), 100)
    // 手工裁到只剩尾部 4 行（模拟滑动窗口），不经过 cap 逻辑
    s.lines = s.lines.slice(6)
    expect(s.lines.map((l) => l.no)).toEqual([7, 8, 9, 10])
    const beforeCounter = s.lineCounter
    const beforePullNo = s.pullNo
    // 头部 rn=7：回补 rn 5/6 → no = head.no-2+i 连续延伸
    const back = prependBackfillInto(s, [mkPulled(5), mkPulled(6)])
    expect(back.map((l) => l.no)).toEqual([5, 6])
    expect(back.map((l) => l.rn)).toEqual([5, 6])
    expect(s.lines.map((l) => l.no)).toEqual([5, 6, 7, 8, 9, 10])
    expect(s.lineCounter).toBe(beforeCounter)
    expect(s.pullNo).toBe(beforePullNo)
    expect(s.droppedLines).toBe(0) // lifetime 累计不回退也不在此推进
    expect(s.backfillTotal).toBe(2)
    expect(takeBackfilledFrom(s)).toEqual(back)
    expect(s.backfillPending).toEqual([])
    expect(takeBackfilledFrom(s)).toEqual([])
  })

  it('守卫：offline / 空视图 / 旧 ring 纪元（no <= reconnectNo）/ 重复行过滤', () => {
    const s = createSession('s1', makeConfig())
    // 空视图不可补
    expect(prependBackfillInto(s, [mkPulled(1)])).toEqual([])
    appendPulledInto(s, Array.from({ length: 4 }, (_, i) => mkPulled(i + 1)), 2)
    // 头部 rn=3：混入 rn>=3 的重复行被防御过滤
    const back = prependBackfillInto(s, [mkPulled(1), mkPulled(2), mkPulled(3)])
    expect(back.map((l) => l.rn)).toEqual([1, 2])
    // 旧 ring 纪元：头部 no <= reconnectNo 不可补
    s.reconnectNo = s.lineCounter
    expect(prependBackfillInto(s, [mkPulled(0)])).toEqual([])
    // offline 会话不可补
    const off = createSession('off', makeConfig())
    off.kind = 'offline'
    expect(prependBackfillInto(off, [mkPulled(1)])).toEqual([])
  })

  it('head 无 rn 时按 MAX_SAFE_INTEGER 处理（防御全收）', () => {
    const s = createSession('s1', makeConfig())
    appendLinesInto(s, [mkRaw(1)], 100)
    const back = prependBackfillInto(s, [mkPulled(1)])
    expect(back).toHaveLength(1)
    expect(back[0]!.no).toBe(0)
  })
})

describe('tallyBytesInto（字节统计，bytes 优先）', () => {
  it('带原始 bytes 的行按 bytes.length 计数，不把 lossy 文本再编码', () => {
    const s = createSession('s1', makeConfig())
    tallyBytesInto(s, [
      { ...mkRaw(1), dir: 'rx', text: '\ufffd\ufffd\ufffd', bytes: [0xff, 0xfe] },
      { ...mkRaw(2), dir: 'tx', text: '\ufffd', bytes: [0x00] },
    ])
    expect(s.rxBytes).toBe(2)
    expect(s.txBytes).toBe(1)
    expect(s.rxLines).toBe(1)
    expect(s.txLines).toBe(1)
  })

  it('无原始 bytes 回退文本 UTF-8 长度；空批次不动状态', () => {
    const s = createSession('s1', makeConfig())
    tallyBytesInto(s, [{ ...mkRaw(1), text: '中' }])
    expect(s.rxBytes).toBe(3)
    tallyBytesInto(s, [])
    expect(s.rxLines).toBe(1)
  })
})

describe('applyDecodedInto / resetDecodedOf（解码帧入表）', () => {
  it('追加 + markRaw + MAX_DECODED FIFO', () => {
    expect(MAX_DECODED).toBe(1000)
    const s = reactive(createSession('s1', makeConfig()))
    applyDecodedInto(s, [mkDecoded(1), mkDecoded(2)])
    applyDecodedInto(s, [mkDecoded(3)])
    expect(s.decoded.map((d) => d.no)).toEqual([1, 2, 3])
    expect(isReactive(s.decoded[0])).toBe(false)
    applyDecodedInto(s, Array.from({ length: MAX_DECODED + 5 }, (_, i) => mkDecoded(i + 10)))
    expect(s.decoded).toHaveLength(MAX_DECODED)
    expect(s.decoded[0]!.no).toBe(15)
  })

  it('replace=true 整表替换（回溯语义）；resetDecodedOf 清空', () => {
    const s = createSession('s1', makeConfig())
    applyDecodedInto(s, [mkDecoded(1), mkDecoded(2)])
    applyDecodedInto(s, [mkDecoded(9)], true)
    expect(s.decoded.map((d) => d.no)).toEqual([9])
    resetDecodedOf(s)
    expect(s.decoded).toEqual([])
  })
})
