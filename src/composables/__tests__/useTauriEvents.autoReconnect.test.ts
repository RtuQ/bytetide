import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../../stores/session'
import { evalAutoReconnect, stopAutoReconnectLoop } from '../useTauriEvents'
import { autoReconnectStateForTest, retryDueAt, _resetAutoReconnectForTest } from '../autoReconnect'
import type { PortConfig, PortInfo } from '../../types'

// 自动重连调度器集成测试：假 invoke + 假时钟（Date 与调度器时基一致），直接驱动
// evalAutoReconnect（生产中的唤醒源是事件回调尾部，这里手动调等价）。
// connect_cmd 回新 id 模拟后端建会话；ring_lines_no_cmd 回空表（drain 幂等）。
const invokeMock = vi.hoisted(() =>
  vi.fn(async (_cmd: string, _args?: unknown) => null as unknown),
)
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

const CFG: PortConfig = {
  transport: 'serial',
  name: 'COM-TEST',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
  autoReconnect: true,
}

function portInfo(name: string): PortInfo {
  return { name, portType: 'usb' }
}

let connectSeq = 0
/** 每次 connect_cmd 的端口名（顺序） */
const connects: string[] = []
/** 全部命令调用序列（断言收尾顺序用） */
const calls: string[] = []

beforeEach(() => {
  setActivePinia(createPinia())
  _resetAutoReconnectForTest()
  invokeMock.mockClear()
  connectSeq = 0
  connects.length = 0
  calls.length = 0
  invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
    calls.push(cmd)
    if (cmd === 'connect_cmd') {
      connectSeq += 1
      connects.push((args as { config: PortConfig }).config.name)
      return `s-new-${connectSeq}`
    }
    if (cmd === 'ring_lines_no_cmd') return []
    return null
  })
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] })
})

afterEach(() => {
  stopAutoReconnectLoop()
  vi.useRealTimers()
})

/** 建一个开了自动重连、error 态的 live 会话（意外断开形态） */
function mkBroken(id: string, name = 'COM-TEST'): string {
  const store = useSessionStore()
  const sid = store.createLocalSession(id, { ...CFG, name })
  store.setStatus(sid, 'error')
  return sid
}

describe('自动重连调度器（evalAutoReconnect）', () => {
  it('端口在场：立即重连一次，账目经 rekey 迁到新 id 且旗标随 config 迁移', async () => {
    const store = useSessionStore()
    const id = mkBroken('a')
    store.setPorts([portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual(['COM-TEST'])
    expect(store.sessions[id]).toBeUndefined() // 旧 id 原位替换
    expect(store.sessions['s-new-1']!.config.autoReconnect).toBe(true)
    expect(autoReconnectStateForTest('s-new-1').attempts).toBe(1)
    expect(retryDueAt(id)).toBeNull()
  })

  it('端口不在场不重试；端口回归（评估唤醒）后立即重连', async () => {
    const store = useSessionStore()
    mkBroken('a')
    store.setPorts([portInfo('OTHER')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual([])
    store.setPorts([portInfo('OTHER'), portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual(['COM-TEST'])
  })

  it('打开失败按退避重试：1s 内不重复，到期再试且退避跨 id 连续', async () => {
    const store = useSessionStore()
    mkBroken('a')
    store.setPorts([portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects.length).toBe(1)
    // 新会话 open 失败（error 事件路径），端口仍在场：退避 1s 未到不重复
    store.setStatus('s-new-1', 'error')
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(500)
    expect(connects.length).toBe(1)
    // 跨过 nextAt：定时链唤醒，第二次尝试（第二次退避 2s）
    await vi.advanceTimersByTimeAsync(600)
    expect(connects.length).toBe(2)
    expect(autoReconnectStateForTest('s-new-2').attempts).toBe(2)
  })

  it('用户主动停止（stopSession）不自动重连；重开开关=改主意立即尝试', async () => {
    const store = useSessionStore()
    const id = mkBroken('a')
    store.setPorts([portInfo('COM-TEST')])
    await store.stopSession(id) // 停靠在 disconnect 之前写入
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual([])
    store.setAutoReconnect(id, true)
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual(['COM-TEST'])
  })

  it('重连成功（connected）后账目清空，不再排定时唤醒', async () => {
    const store = useSessionStore()
    mkBroken('a')
    store.setPorts([portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(autoReconnectStateForTest('s-new-1').attempts).toBe(1)
    store.setStatus('s-new-1', 'connected')
    evalAutoReconnect() // 生产中 connected 事件回调尾部同款评估
    expect(retryDueAt('s-new-1')).toBeNull()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(connects.length).toBe(1) // 闲置一分钟后也不再有新尝试
  })

  it('同端口两个候选标签页只重连一个（不互殴）', async () => {
    mkBroken('a')
    mkBroken('b')
    useSessionStore().setPorts([portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects.length).toBe(1)
  })

  it('未开自动重连 / 网络源会话不尝试', async () => {
    const store = useSessionStore()
    const off = store.createLocalSession('a', { ...CFG, autoReconnect: false })
    store.setStatus(off, 'error')
    const net = store.createLocalSession('b', {
      ...CFG, transport: 'tcp-client', autoReconnect: true,
    })
    store.setStatus(net, 'error')
    store.setPorts([portInfo('COM-TEST')])
    evalAutoReconnect()
    await vi.advanceTimersByTimeAsync(0)
    expect(connects).toEqual([])
  })
})
