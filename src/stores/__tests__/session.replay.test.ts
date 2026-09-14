import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../session'
import { isPullSession } from '../session/model'
import { drainSession } from '../../composables/useTauriEvents'
import type { PortConfig } from '../../types'

// invoke 全文件打桩：回放命令在无 Tauri 后端的测试环境可走通
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
}

function calledCommands(): string[] {
  return invokeMock.mock.calls.map((c) => c[0] as string)
}

beforeEach(() => {
  setActivePinia(createPinia())
  invokeMock.mockClear()
  invokeMock.mockImplementation(async () => null)
})

describe('loadReplaySession（回放会话建账）', () => {
  it('建 replay 会话：kind/status/offline 元信息/replay 控制面初值', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'open_replay_session_cmd') {
        return { sessionId: 'r1', lineCount: 30, durationMs: 29000 }
      }
      return null
    })
    const store = useSessionStore()
    const id = await store.loadReplaySession('/logs/demo.log', 2.5, true)
    expect(id).toBe('r1')
    expect(calledCommands()).toContain('open_replay_session_cmd')
    const s = store.sessions['r1']!
    expect(s).toBeDefined()
    expect(s.kind).toBe('replay')
    expect(s.status).toBe('connecting') // runner 起跑即发 connected，事件随后修正
    expect(s.config.name).toBe('demo')
    expect(s.offlineLineCount).toBe(30)
    expect(s.replay).toEqual({ state: 'ready', speed: 2.5, looped: true, line: 0 })
  })

  it('speed/looped 参数透传 open_replay_session_cmd', async () => {
    invokeMock.mockImplementation(async () => ({ sessionId: 'r2', lineCount: 1, durationMs: 0 }))
    const store = useSessionStore()
    await store.loadReplaySession('/logs/a.log')
    const call = invokeMock.mock.calls.find((c) => c[0] === 'open_replay_session_cmd')
    expect(call![1]).toEqual({ path: '/logs/a.log', speed: 1, looped: false })
  })
})

describe('setReplayView（replay-state 事件 / 轮询落账）', () => {
  it('replay 会话写入控制面视图；live/未知会话忽略', () => {
    const store = useSessionStore()
    const liveId = store.createLocalSession('live-1', CFG)
    store.sessions[liveId]!.kind = 'live'
    const view = { state: 'paused' as const, speed: 5, looped: false, line: 7 }
    // 未知会话与 live 会话一律忽略
    store.setReplayView('r-nope', view)
    store.setReplayView(liveId, view)
    expect(store.sessions[liveId]!.replay).toBeNull()

    // 建一个 replay 会话（不经后端，直接改账）
    store.createLocalSession('r1', { ...CFG, name: 'replay-src' })
    const r = store.sessions['r1']!
    r.kind = 'replay'
    r.status = 'connected'
    store.setReplayView('r1', view)
    expect(r.replay).toEqual(view)
    // 整体替换：后续事件覆盖
    store.setReplayView('r1', { state: 'running', speed: 10, looped: true, line: 9 })
    expect(r.replay).toEqual({ state: 'running', speed: 10, looped: true, line: 9 })
  })
})

describe('isPullSession（拉模型会话判定）', () => {
  it('live/replay 为真、offline 为假', () => {
    expect(isPullSession({ kind: 'live' })).toBe(true)
    expect(isPullSession({ kind: 'replay' })).toBe(true)
    expect(isPullSession({ kind: 'offline' })).toBe(false)
  })
})

describe('回放会话生命周期', () => {
  async function mkReplay(): Promise<{ store: ReturnType<typeof useSessionStore>; id: string }> {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'open_replay_session_cmd') {
        return { sessionId: 'r1', lineCount: 10, durationMs: 9000 }
      }
      // 停止/断开的最终补拉：拉空（墓碑或在线 ring 均无增量）
      if (cmd === 'ring_lines_no_cmd') return []
      return null
    })
    const store = useSessionStore()
    const id = await store.loadReplaySession('/logs/demo.log')
    return { store, id }
  }

  it('stopSession 断开：两阶段关闭（disconnect → 补拉 → release）、status 置 disconnected、控制面定格 stopped', async () => {
    const { store, id } = await mkReplay()
    const r = store.sessions[id]!
    r.status = 'connected'
    r.replay = { state: 'running', speed: 1, looped: false, line: 3 }
    await store.stopSession(id)
    const cmds = calledCommands()
    expect(cmds).toContain('disconnect_cmd')
    expect(cmds).toContain('release_session_cmd')
    // 两阶段顺序：先断开（挂墓碑）再释放（补拉完成后）
    expect(cmds.indexOf('disconnect_cmd')).toBeLessThan(cmds.indexOf('release_session_cmd'))
    expect(r.status).toBe('disconnected')
    expect(r.replay?.state).toBe('stopped')
  })

  it('closeTab 关闭：disconnect + release 下发并移除会话', async () => {
    const { store, id } = await mkReplay()
    await store.closeTab(id)
    const cmds = calledCommands()
    expect(cmds).toContain('disconnect_cmd')
    expect(cmds).toContain('release_session_cmd')
    expect(store.sessions[id]).toBeUndefined()
  })

  it('复审 R-P1-2：在途常规拉取未完成时，stopSession 等待其结束并完成最终拉空后才 release', async () => {
    const { store, id } = await mkReplay()
    const r = store.sessions[id]!
    r.status = 'connected'
    r.replay = { state: 'running', speed: 1, looped: false, line: 0 }
    // 第一批拉取挂起（模拟渲染进程被节流/IPC 慢），手控放行
    let releaseFirstPull!: (v: unknown[]) => void
    const firstPull = new Promise<unknown[]>((resolve) => {
      releaseFirstPull = resolve
    })
    let pullCount = 0
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'open_replay_session_cmd') {
        return { sessionId: 'r1', lineCount: 10, durationMs: 9000 }
      }
      if (cmd === 'ring_lines_no_cmd') {
        pullCount += 1
        if (pullCount === 1) return firstPull
        if (pullCount === 2) {
          // 最终补拉：游标已到 1，尾行 no=2
          return [
            { ts: '00:00:02.000', dir: 'rx', text: 'tail-line', epochMillis: 2000, no: 2 },
          ]
        }
        return []
      }
      return null
    })
    // 在途常规拉取占用 draining
    drainSession(id)
    await vi.waitFor(() => expect(pullCount).toBe(1)) // 常规拉取已走到挂起点
    // 停止：drainSessionTail 必须等在途拉取真正结束（而非固定超时后放弃）
    const stopP = store.stopSession(id)
    // 在途未放行期间，release 不得发生
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(calledCommands()).not.toContain('release_session_cmd')
    // 放行在途拉取 → 常规拉取收尾 → 最终补拉拉尾行 → release
    releaseFirstPull([
      { ts: '00:00:01.000', dir: 'rx', text: 'inflight-line', epochMillis: 1000, no: 1 },
    ])
    await stopP
    const cmds = calledCommands()
    expect(cmds).toContain('release_session_cmd')
    expect(cmds.indexOf('release_session_cmd')).toBeGreaterThan(cmds.indexOf('disconnect_cmd'))
    // 尾批两行（在途行 + 最终补拉行）都进了表，不丢尾批
    const texts = r.lines.map((l) => l.text)
    expect(texts).toContain('inflight-line')
    expect(texts).toContain('tail-line')
    expect(r.status).toBe('disconnected')
    expect(r.replay?.state).toBe('stopped')
  })

  it('clearLog 允许（清屏不清控制面与离线元信息）', async () => {
    const { store, id } = await mkReplay()
    const r = store.sessions[id]!
    r.status = 'connected'
    r.replay = { state: 'paused', speed: 1, looped: false, line: 3 }
    await store.clearLog(id)
    expect(calledCommands()).toContain('clear_log_cmd')
    expect(r.lines).toHaveLength(0)
    expect(r.replay).toEqual({ state: 'paused', speed: 1, looped: false, line: 3 })
    expect(r.offlineLineCount).toBe(10)
  })

  it('重连拒绝：replay 会话不可重连（不发 connect_cmd、会话原样保留）', async () => {
    const { store, id } = await mkReplay()
    const before = store.sessions[id]
    await store.reconnectSession(id)
    expect(calledCommands()).not.toContain('connect_cmd')
    expect(store.sessions[id]).toBe(before)
  })

  it('pushLiveRules 放行 replay（告警经 common ingest 仍评估）', async () => {
    const { store, id } = await mkReplay()
    store.addAlertRule(id)
    store.pushLiveRules(id)
    expect(calledCommands()).toContain('set_live_rules_cmd')
  })
})
