import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../../stores/session'
import { startPullLoop, stopPullLoop } from '../useTauriEvents'
import type { PortConfig } from '../../types'

// 调度器集成测试：假 invoke + 假时钟（含 performance.now，时基与生产一致），
// 验证自适应节奏的真实拉取时刻——活跃贴地板、空闲维持天花板、剪除、零 IPC。
// 注意 pullsOf 返回的是 Map 内同一数组引用（随拉取增长），断言计数前先快照。
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

/** ring_lines_no_cmd 每次调用的触发时刻（假时钟 ms），按会话分账 */
const pullTimes = new Map<string, number[]>()
/** 脚本：第 n 次拉取（按会话计）是否回一行数据 */
let dataAt: (sessionId: string, n: number) => boolean = () => false
const lineSeq = new Map<string, number>()

function pullsOf(id: string): number[] {
  return pullTimes.get(id) ?? []
}

beforeEach(() => {
  setActivePinia(createPinia())
  invokeMock.mockClear()
  pullTimes.clear()
  lineSeq.clear()
  dataAt = () => false
  invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'ring_lines_no_cmd') {
      const sessionId = (args as { sessionId: string }).sessionId
      const times = pullTimes.get(sessionId) ?? []
      times.push(performance.now())
      pullTimes.set(sessionId, times)
      if (dataAt(sessionId, times.length)) {
        const no = (lineSeq.get(sessionId) ?? 0) + 1
        lineSeq.set(sessionId, no)
        return [
          { ts: '00:00:01.000', dir: 'rx', text: `line-${sessionId}-${no}`, epochMillis: 1000, no },
        ]
      }
      return []
    }
    return null
  })
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] })
})

afterEach(() => {
  stopPullLoop()
  vi.useRealTimers()
})

function mkLive(id: string): string {
  const store = useSessionStore()
  const sid = store.createLocalSession(id, { ...CFG, name: id })
  store.setStatus(sid, 'connected')
  return sid
}

describe('自适应拉取调度器（startPullLoop）', () => {
  it('活跃期贴 25ms 地板，空拉经宽限后按 25→45→81→146→200 爬回天花板', async () => {
    const id = mkLive('a')
    dataAt = (_sid, n) => n === 1
    startPullLoop()
    await vi.advanceTimersByTimeAsync(600)
    // 首拍有数据贴地板；之后 3 拍宽限维持 25ms；第 5 拍起乘性爬升到 200 封顶
    expect(pullsOf(id)).toEqual([0, 25, 50, 75, 100, 145, 226, 372, 572])
  })

  it('无数据的会话维持 200ms 天花板节奏（空拍不误贴地板）', async () => {
    const id = mkLive('a')
    startPullLoop()
    await vi.advanceTimersByTimeAsync(1000)
    expect(pullsOf(id)).toEqual([0, 200, 400, 600, 800, 1000])
  })

  it('静默后来数据：下一拍内被拉到，且再下一拍恢复 25ms 地板', async () => {
    const id = mkLive('a')
    startPullLoop()
    await vi.advanceTimersByTimeAsync(2000)
    const times = pullsOf(id)
    expect(times[times.length - 1]! - times[times.length - 2]!).toBe(200) // 已在天花板
    const before = times.length
    dataAt = (_sid, n) => n === before + 1 // 下一拍回数据
    await vi.advanceTimersByTimeAsync(210) // 只够一拍（2000→2210 覆盖 2200 数据拍）
    expect(pullsOf(id).length).toBe(before + 1) // 数据拍（≤200ms 内被拉到）
    dataAt = () => false
    await vi.advanceTimersByTimeAsync(35) // 只够一拍（2210→2245 覆盖 2225）
    const t2 = pullsOf(id)
    expect(t2.length).toBe(before + 2)
    expect(t2[t2.length - 1]! - t2[t2.length - 2]!).toBe(25) // 数据后贴地板
  })

  it('多会话独立节奏：持续活跃的会话贴地板，空闲会话自行维持天花板不陪跑', async () => {
    mkLive('busy')
    mkLive('idle')
    dataAt = (sid) => sid === 'busy'
    startPullLoop()
    await vi.advanceTimersByTimeAsync(600)
    const busy = pullsOf('busy')
    const idle = pullsOf('idle')
    expect(busy.length).toBe(25) // 0,25,…,600：全程地板
    for (let i = 1; i < busy.length; i++) expect(busy[i]! - busy[i - 1]!).toBe(25)
    expect(idle).toEqual([0, 200, 400, 600]) // 与 busy 互不干扰
  })

  it('断开的 live 会话零 IPC：状态守卫在 invoke 之前短路', async () => {
    const id = mkLive('a')
    startPullLoop()
    await vi.advanceTimersByTimeAsync(30)
    expect(pullsOf(id)).toEqual([0])
    useSessionStore().setStatus(id, 'disconnected')
    await vi.advanceTimersByTimeAsync(1000)
    expect(pullsOf(id)).toEqual([0])
  })

  it('会话关闭（closeTab）后节奏条目剪除：不再产生任何拉取', async () => {
    const store = useSessionStore()
    const id = mkLive('a')
    startPullLoop()
    await vi.advanceTimersByTimeAsync(30)
    expect(pullsOf(id).length).toBe(1)
    // closeTab 不补拉直接 release（关闭=彻底丢弃）；随后调度器剪除条目
    await store.closeTab(id)
    await vi.advanceTimersByTimeAsync(1000)
    expect(pullsOf(id).length).toBe(1)
  })
})
