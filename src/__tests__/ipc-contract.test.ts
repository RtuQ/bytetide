// 前端 IPC DTO 契约测试（Stage 2 Task 1）。
// 与 crates/bytetide-core/tests/session_contract.rs **同源**的字面量镜像：Rust 侧用
// serde_json::to_string 冻结后端序列化字节，这里用同一批 JSON 冻结前端消费侧——
// 两边任一形状漂移都会红（Task 7 起迁移到共享 testdata/protocol/ fixtures）。
// 冻结点：
// - ring 拉取行（BridgeLine）→ useTauriEvents 的 PulledLine → RawLogLine & {ringNo}
//   → store.appendPulled 的 LogLine，映射不丢字段、bytes 数组原样
// - session-status / session-error 事件载荷（src-tauri/src/gui_sink.rs 的
//   StatusPayload/ErrorPayload）与 useTauriEvents handler 实际消费的键一致
// - ring_bounds_cmd 返回（RingBounds）与 requestBackfill 消费的 firstNo 一致
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../stores/session'
import type { Dir, LogLine, RawLogLine } from '../types'

// invoke 打桩：store 模块在无 Tauri 后端的测试环境可安全导入（对齐 session.test.ts）
const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

// ---------------------------------------------------------------------------
// 冻结的 JSON 字面量（与 session_contract.rs 逐字节同源）
// ---------------------------------------------------------------------------

// BridgeLine：bytes/match 均在（dir=rx，serde rename_all=camelCase，键序=字段声明序）
const BRIDGE_LINE_RX_FULL =
  '{"no":7,"ts":"12:00:00.000","dir":"rx","text":"AA 55 01","bytes":[170,85,1],' +
  '"epochMillis":1727000000000,"match":{"offset":0,"length":2,"field":"head"}}'
// BridgeLine：bytes=None + match=None → 两键整体消失（skip_serializing_if）
const BRIDGE_LINE_TX_PLAIN =
  '{"no":8,"ts":"12:00:00.001","dir":"tx","text":"hello","epochMillis":1727000000001}'
// RingBounds（ring_bounds_cmd / ring_lines_before 前置探测的返回）
const RING_BOUNDS_JSON = '{"firstNo":3,"lastNo":102,"size":100,"ringCap":100000}'
// Tauri 事件载荷（gui_sink.rs StatusPayload / ErrorPayload，camelCase）
const SESSION_STATUS_PAYLOAD = '{"sessionId":"s1","status":"connected"}'
const SESSION_ERROR_PAYLOAD = '{"sessionId":"s1","error":"拒绝访问"}'

/** 镜像 useTauriEvents.ts 的 PulledLine（后端 BridgeLine 去掉前端不消费的 match） */
interface PulledLine {
  no: number
  ts: string
  dir: Dir
  text: string
  bytes: number[] | null
  epochMillis: number
}

/** drainSession/requestBackfill 里 PulledLine → appendPulled 入参的既有映射（逐字段） */
function toStoreInput(l: PulledLine): RawLogLine & { ringNo: number } {
  return {
    ts: l.ts,
    dir: l.dir,
    text: l.text,
    bytes: l.bytes,
    epochMillis: l.epochMillis,
    ringNo: l.no,
  }
}

describe('ring 拉取行契约（BridgeLine → PulledLine → RawLogLine）', () => {
  it('含 bytes/match 的 rx 行解析后满足 PulledLine 形状、字段无丢失', () => {
    const pulled = JSON.parse(BRIDGE_LINE_RX_FULL) as PulledLine
    expect(pulled.no).toBe(7)
    expect(pulled.ts).toBe('12:00:00.000')
    expect(pulled.dir).toBe('rx')
    expect(pulled.text).toBe('AA 55 01')
    // bytes 原样是原始字节数组（不是文本再编码，也不是 HEX 字符串）
    expect(pulled.bytes).toEqual([0xaa, 0x55, 0x01])
    expect(pulled.epochMillis).toBe(1727000000000)
  })

  it('dir 值域冻结为小写 rx/tx（serde rename_all=lowercase）', () => {
    expect((JSON.parse(BRIDGE_LINE_RX_FULL) as PulledLine).dir).toBe('rx')
    expect((JSON.parse(BRIDGE_LINE_TX_PLAIN) as PulledLine).dir).toBe('tx')
  })

  it('bytes/match 缺省行：两键不在 JSON 里（后端省略而非 null）', () => {
    const parsed = JSON.parse(BRIDGE_LINE_TX_PLAIN) as Record<string, unknown>
    expect('bytes' in parsed).toBe(false)
    expect('match' in parsed).toBe(false)
    // PulledLine.bytes 声明 number[]|null，但后端实际产出是「键缺失→undefined」；
    // 映射直传后 RawLogLine.bytes 同为 optional，消费侧（lineHexDump）对
    // undefined/null/[] 一律回退 text 编码——冻结这一现状
    const pulled = JSON.parse(BRIDGE_LINE_TX_PLAIN) as PulledLine
    expect(pulled.bytes).toBeUndefined()
  })

  it('映射到 RawLogLine（+ringNo）编译期形状兼容且无字段丢失', () => {
    const pulled = JSON.parse(BRIDGE_LINE_RX_FULL) as PulledLine
    const input: RawLogLine & { ringNo: number } = toStoreInput(pulled)
    expect(input).toEqual({
      ts: '12:00:00.000',
      dir: 'rx',
      text: 'AA 55 01',
      bytes: [0xaa, 0x55, 0x01],
      epochMillis: 1727000000000,
      ringNo: 7,
    })
  })
})

describe('appendPulled 入表契约（fixture 字段 → LogLine）', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('拉取行经映射入表后 LogLine 携带 no/rn，bytes 数组原样', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-contract', {
      transport: 'serial',
      name: 'COM-CONTRACT',
      baudRate: 115200,
      dataBits: 8,
      parity: 'none',
      stopBits: '1',
      flowControl: 'none',
    })
    const lines: PulledLine[] = [
      JSON.parse(BRIDGE_LINE_RX_FULL) as PulledLine,
      JSON.parse(BRIDGE_LINE_TX_PLAIN) as PulledLine,
    ]
    const fresh: LogLine[] = store.appendPulled(id, lines.map(toStoreInput))
    expect(fresh).toHaveLength(2)
    const [first, second] = fresh
    expect(first!.no).toBe(1)
    expect(first!.rn).toBe(7) // ring no 进 rn，不占显示列
    expect(first!.ts).toBe('12:00:00.000')
    expect(first!.dir).toBe('rx')
    expect(first!.text).toBe('AA 55 01')
    expect(first!.bytes).toEqual([0xaa, 0x55, 0x01])
    expect(first!.epochMillis).toBe(1727000000000)
    // 缺 bytes 的行入表后 bytes 为 undefined（可选键原样传递）
    expect(second!.rn).toBe(8)
    expect(second!.bytes).toBeUndefined()
    // 游标推进到最后一条 ring no
    expect(store.sessions[id]!.pullNo).toBe(8)
  })
})

describe('事件载荷契约（gui_sink.rs ↔ useTauriEvents handler）', () => {
  it('session-status payload 键为 sessionId/status，与 handler 消费一致', () => {
    const payload = JSON.parse(SESSION_STATUS_PAYLOAD) as { sessionId: string; status: string }
    // useTauriEvents: store.setStatus(e.payload.sessionId, e.payload.status)
    const keys = Object.keys(payload).sort()
    expect(keys).toEqual(['sessionId', 'status'])
    expect(typeof payload.sessionId).toBe('string')
    expect(payload.status).toBe('connected')
    expect(['connecting', 'connected', 'disconnected', 'error']).toContain(payload.status)
  })

  it('session-error payload 键为 sessionId/error，与 handler 消费一致', () => {
    const payload = JSON.parse(SESSION_ERROR_PAYLOAD) as { sessionId: string; error: string }
    // useTauriEvents: store.setError(e.payload.sessionId, e.payload.error)
    expect(Object.keys(payload).sort()).toEqual(['error', 'sessionId'])
    expect(typeof payload.error).toBe('string')
    expect(payload.error).toBe('拒绝访问')
  })

  it('ring bounds payload 键为 firstNo/…，requestBackfill 消费的 firstNo 在位', () => {
    const bounds = JSON.parse(RING_BOUNDS_JSON) as { firstNo: number; lastNo: number; size: number; ringCap: number }
    expect(Object.keys(bounds).sort()).toEqual(['firstNo', 'lastNo', 'ringCap', 'size'])
    expect(bounds.firstNo).toBe(3)
    expect(bounds.ringCap).toBe(100000)
  })
})
