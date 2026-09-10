import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../session'
import type { PortConfig, SendSequence } from '../../types'

// invoke 全文件打桩：set_signal_cmd / send_cmd 在无 Tauri 后端的测试环境可走通
const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

const CFG: PortConfig = {
  transport: 'serial',
  name: 'COM-TEST',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
}

beforeEach(() => {
  setActivePinia(createPinia())
  vi.stubGlobal(
    'localStorage',
    (() => {
      const m = new Map<string, string>()
      return {
        getItem: (k: string) => m.get(k) ?? null,
        setItem: (k: string, v: string) => void m.set(k, v),
        removeItem: (k: string) => void m.delete(k),
      }
    })(),
  )
})
afterEach(() => {
  vi.unstubAllGlobals()
})

function mkLiveSession(store: ReturnType<typeof useSessionStore>): string {
  const id = store.createLocalSession('local-send', CFG)
  const s = store.sessions[id]!
  // createLocalSession 建的是本地态；发送/信号要求 live + connected
  s.kind = 'live'
  s.status = 'connected'
  return id
}

describe('快捷帧（sendPresets）', () => {
  it('保存/更新/删除并持久化 localStorage', () => {
    const store = useSessionStore()
    store.saveSendPreset({ name: '查询', payload: '01 03', mode: 'hex' })
    expect(store.sendPresets).toHaveLength(1)
    const p = store.sendPresets[0]!
    expect(p.name).toBe('查询')

    store.saveSendPreset({ id: p.id, name: '查询温度', payload: '01 03 00', mode: 'hex' })
    expect(store.sendPresets[0]!.name).toBe('查询温度')
    expect(store.sendPresets).toHaveLength(1)

    store.removeSendPreset(p.id)
    expect(store.sendPresets).toHaveLength(0)
    expect(localStorage.getItem('serialtool.sendPresets')).toBe('[]')
  })

  it('空名拒绝；超出上限丢最旧', () => {
    const store = useSessionStore()
    store.saveSendPreset({ name: '  ', payload: 'x', mode: 'ascii' })
    expect(store.sendPresets).toHaveLength(0)
    for (let i = 0; i < 52; i++) {
      store.saveSendPreset({ name: `p${i}`, payload: String(i), mode: 'ascii' })
    }
    expect(store.sendPresets).toHaveLength(50)
    expect(store.sendPresets[0]!.name).toBe('p2') // 最旧的 p0/p1 被挤掉
  })
})

describe('发送序列（sendSequences）', () => {
  it('保存整体覆盖同 id、intervalMs 钳制 ≥50、非法步骤被过滤', () => {
    const store = useSessionStore()
    const seq: SendSequence = {
      id: 'sq1',
      name: '复位查询',
      steps: [
        { kind: 'signal', pin: 'dtr', level: false },
        { kind: 'delay', ms: 120 },
        { kind: 'send', payload: '01 03', mode: 'hex', appendNewline: false },
        // 非法步骤（kind 未知 / 字段缺失）保存时被过滤
        { kind: 'bogus' } as unknown as SendSequence['steps'][number],
      ],
      loop: true,
      intervalMs: 10,
    }
    store.saveSendSequence(seq)
    const saved = store.sendSequences[0]!
    expect(saved.intervalMs).toBe(50)
    expect(saved.steps).toHaveLength(3)

    saved.name = '改名'
    store.saveSendSequence(saved)
    expect(store.sendSequences).toHaveLength(1)
    expect(store.sendSequences[0]!.name).toBe('改名')

    store.removeSendSequence('sq1')
    expect(store.sendSequences).toHaveLength(0)
  })
})

describe('触发式现场捕获（capture 字段三处同步纪律）', () => {
  it('makeSession 默认关闭 + 默认窗口；updateCapture 合并并推送后端', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-cap', CFG)
    const s = store.sessions[id]!
    expect(s.capture).toEqual({
      enabled: false,
      onDisconnect: true,
      preMs: 120000,
      postMs: 60000,
      rules: [],
    })
    invokeMock.mockClear()
    store.updateCapture(id, {
      enabled: true,
      rules: [
        { id: 'c1', pattern: 'ERROR|Fault', useRegex: true, caseSensitive: false, wholeWord: false, enabled: true },
      ],
    })
    expect(s.capture.enabled).toBe(true)
    expect(s.capture.rules).toHaveLength(1)
    expect(invokeMock).toHaveBeenCalledWith(
      'set_live_rules_cmd',
      expect.objectContaining({
        sessionId: id,
        capture: expect.objectContaining({ enabled: true }),
      }),
    )
  })

  it('updateCapture 对离线会话为 no-op', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-cap3', CFG)
    store.sessions[id]!.kind = 'offline'
    invokeMock.mockClear()
    store.updateCapture(id, { enabled: true })
    expect(store.sessions[id]!.capture.enabled).toBe(false)
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('重连迁移 capture 配置（reconnectSession carried 清单）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-cap2', CFG)
    const s = store.sessions[id]!
    s.kind = 'live'
    s.status = 'connected'
    store.updateCapture(id, { enabled: true, preMs: 30000 })
    invokeMock.mockClear()
    invokeMock.mockResolvedValueOnce('s-reconnected')
    await store.reconnectSession(id)
    const carried = store.sessions['s-reconnected']!
    expect(carried.capture.enabled).toBe(true)
    expect(carried.capture.preMs).toBe(30000)
  })
})

describe('DTR/RTS 信号线（setSignal）', () => {
  it('下发 set_signal_cmd 且参数为 camelCase', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    invokeMock.mockClear()
    await store.setSignal(id, 'dtr', true)
    expect(invokeMock).toHaveBeenCalledWith('set_signal_cmd', {
      sessionId: id,
      pin: 'dtr',
      level: true,
    })
  })

  it('离线/非 live 会话不下发', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-off', CFG) // 默认 live，改离线
    store.sessions[id]!.kind = 'offline'
    invokeMock.mockClear()
    await store.setSignal(id, 'rts', true)
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('后端失败向上抛错（调用方提示）', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    invokeMock.mockRejectedValueOnce(new Error('设置信号线失败'))
    await expect(store.setSignal(id, 'rts', false)).rejects.toThrow('设置信号线失败')
  })
})

describe('序列运行（runSequence/stopSequence）', () => {
  it('按步骤执行发送与信号命令；单轮结束自动清运行态', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    store.saveSendSequence({
      id: 'sq-run',
      name: '流程',
      steps: [
        { kind: 'signal', pin: 'dtr', level: false },
        { kind: 'delay', ms: 1 },
        { kind: 'send', payload: 'AT', mode: 'ascii', appendNewline: true },
      ],
      loop: false,
      intervalMs: 50,
    })
    invokeMock.mockClear()
    await store.runSequence(id, 'sq-run')
    expect(invokeMock).toHaveBeenCalledWith('set_signal_cmd', {
      sessionId: id,
      pin: 'dtr',
      level: false,
    })
    expect(invokeMock).toHaveBeenCalledWith('send_cmd', {
      sessionId: id,
      mode: 'ascii',
      text: 'AT\n',
    })
    expect(store.seqRun).toBeNull()
  })

  it('循环模式：stopSequence 中止后清运行态', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    store.saveSendSequence({
      id: 'sq-loop',
      name: '轮询',
      steps: [
        { kind: 'send', payload: 'P', mode: 'ascii', appendNewline: false },
        { kind: 'delay', ms: 400 },
      ],
      loop: true,
      intervalMs: 400,
    })
    invokeMock.mockClear()
    const running = store.runSequence(id, 'sq-loop')
    await new Promise((r) => setTimeout(r, 30))
    expect(store.seqRun).not.toBeNull()
    store.stopSequence()
    await running
    expect(store.seqRun).toBeNull()
    // 至少发过一轮
    expect(
      (invokeMock.mock.calls as unknown[][]).some((c) => c[0] === 'send_cmd'),
    ).toBe(true)
  })

  it('会话断开即中止（不发后续步骤）', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    store.saveSendSequence({
      id: 'sq-disc',
      name: '断开中止',
      steps: [
        { kind: 'delay', ms: 60 },
        { kind: 'send', payload: 'X', mode: 'ascii', appendNewline: false },
      ],
      loop: false,
      intervalMs: 50,
    })
    invokeMock.mockClear()
    const running = store.runSequence(id, 'sq-disc')
    // 延时窗口内断开会话
    await new Promise((r) => setTimeout(r, 10))
    store.sessions[id]!.status = 'disconnected'
    await running
    expect((invokeMock.mock.calls as unknown[][]).some((c) => c[0] === 'send_cmd')).toBe(false)
    expect(store.seqRun).toBeNull()
  })

  it('发送失败中止并抛错', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    store.saveSendSequence({
      id: 'sq-err',
      name: '出错中止',
      steps: [{ kind: 'send', payload: 'A', mode: 'ascii', appendNewline: false }],
      loop: false,
      intervalMs: 50,
    })
    invokeMock.mockRejectedValueOnce(new Error('写入失败'))
    await expect(store.runSequence(id, 'sq-err')).rejects.toThrow('写入失败')
    expect(store.seqRun).toBeNull()
  })

  it('同一时刻仅允许一个序列运行', async () => {
    const store = useSessionStore()
    const id = mkLiveSession(store)
    for (const sid of ['sq-a', 'sq-b']) {
      store.saveSendSequence({
        id: sid,
        name: sid,
        steps: [
          { kind: 'send', payload: sid, mode: 'ascii', appendNewline: false },
          { kind: 'delay', ms: 60 },
        ],
        loop: false,
        intervalMs: 50,
      })
    }
    invokeMock.mockClear()
    const first = store.runSequence(id, 'sq-a')
    await store.runSequence(id, 'sq-b') // 第二个立即返回不执行
    store.stopSequence()
    await first
    expect((invokeMock.mock.calls as unknown[][]).filter((c) => c[0] === 'send_cmd')).toHaveLength(1)
  })
})
