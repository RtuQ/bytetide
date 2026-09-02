import { describe, it, expect, beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore } from '../session'
import { DEFAULT_PLOT_CONFIG } from '../../types'
import type { PortConfig, RawLogLine } from '../../types'

const CFG: PortConfig = {
  transport: 'serial',
  name: 'COM-TEST',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
}

function mkRaw(epoch: number, dir: 'rx' | 'tx' = 'rx'): RawLogLine {
  return { ts: '00:00:00.000', dir, text: `l${epoch}`, bytes: null, epochMillis: epoch }
}

describe('appendPulled 拉模型摄取', () => {
  beforeEach(() => setActivePinia(createPinia()))

  function mkPulled(ringNo: number) {
    return { ...mkRaw(ringNo), ringNo }
  }

  it('按 ringNo 升序入表、游标推进到最新、行号全局单调', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-pull', CFG)
    const fresh = store.appendPulled(id, [mkPulled(3), mkPulled(7), mkPulled(9)])
    const s = store.sessions[id]!
    expect(fresh.map((l) => l.no)).toEqual([1, 2, 3])
    expect(s.lines.map((l) => l.epochMillis)).toEqual([3, 7, 9])
    expect(s.pullNo).toBe(9)
  })

  it('游标防御：ringNo <= pullNo 的重复拉取不入表', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-pull2', CFG)
    store.appendPulled(id, [mkPulled(1), mkPulled(2)])
    // 重复拉到已消费的 1/2 + 新行 5：只有 5 入表
    const fresh = store.appendPulled(id, [mkPulled(1), mkPulled(2), mkPulled(5)])
    expect(fresh.map((l) => l.epochMillis)).toEqual([5])
    expect(store.sessions[id]!.lines).toHaveLength(3)
    expect(store.sessions[id]!.pullNo).toBe(5)
  })

  it('超过 MAX_LINES 裁剪最旧行并累计 droppedLines', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-pull3', CFG)
    const batch = Array.from({ length: 50010 }, (_, i) => mkPulled(i + 1))
    store.appendPulled(id, batch)
    const s = store.sessions[id]!
    expect(s.lines).toHaveLength(50000)
    expect(s.droppedLines).toBe(10)
    expect(s.pullNo).toBe(50010)
  })

  it('空批次返回空且不推进游标', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-pull4', CFG)
    expect(store.appendPulled(id, [])).toEqual([])
    expect(store.sessions[id]!.pullNo).toBe(0)
  })

  it('appendLines 丢弃水位以下的迟到事件行（防补拉重复）', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-test', CFG)
    store.appendLines(id, [mkRaw(100)])
    // 模拟补拉已插入到 5000：此后迟到的 5000 及以下行应被丢弃
    const s = store.sessions[id]!
    s.pulledThrough = 5000
    const fresh = store.appendLines(id, [mkRaw(4800), mkRaw(5000), mkRaw(5200)])
    expect(fresh.map((l) => l.epochMillis)).toEqual([5200])
    expect(s.lines).toHaveLength(2)
  })

  it('clearLog 重置水位', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-test', CFG)
    store.appendLines(id, [mkRaw(100)])
    const s = store.sessions[id]!
    s.pulledThrough = 200
    expect(store.sessions[id]!.pulledThrough).toBe(200)
    // clearLog 会调后端命令；仅验证水位重置（invoke 失败被吞）
    void store.clearLog(id)
    // invoke 在无 Tauri 环境抛错走 catch，字段重置仍应发生
    expect(store.sessions[id]!.pulledThrough).toBe(0)
  })

  it('建账竞态：会话未落账时的状态先暂存、落账后回放', () => {
    const store = useSessionStore()
    // 模拟后端线程抢先：connected 在会话存在前到达
    store.setStatus('race-1', 'connected')
    store.setError('race-1', '')
    expect(store.sessions['race-1']).toBeUndefined()
    // 落账后回放 -> 直接是 connected，不再卡 connecting
    store.createLocalSession('race-1', CFG)
    expect(store.sessions['race-1']!.status).toBe('connected')
  })

  it('建账竞态：提前到达的打开失败错误同样回放', () => {
    const store = useSessionStore()
    store.setError('race-2', '打开串口失败: 权限不足')
    store.createLocalSession('race-2', CFG)
    const s = store.sessions['race-2']!
    expect(s.error).toContain('权限不足')
    expect(s.status).toBe('error')
  })

  it('已落账会话的 setStatus 行为不变', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('race-3', CFG)
    store.setStatus(id, 'connected')
    expect(store.sessions[id]!.status).toBe('connected')
  })
})

describe('adoptBridgePlot 桥接文法写回', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('整包替换 plot，未知会话静默不抛', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-bp', CFG)
    store.adoptBridgePlot(id, {
      ...DEFAULT_PLOT_CONFIG,
      enabled: true,
      frameHead: 'AA55',
      checksum: 'xor',
      endian: 'little',
    })
    const s = store.sessions[id]!
    expect(s.plot.enabled).toBe(true)
    expect(s.plot.frameHead).toBe('AA55')
    expect(s.plot.checksum).toBe('xor')
    expect(s.plot.endian).toBe('little')
    // 写回缺省字段回填默认值（整包替换语义）
    expect(s.plot.maxPoints).toBe(DEFAULT_PLOT_CONFIG.maxPoints)
    expect(() => store.adoptBridgePlot('nope', { ...DEFAULT_PLOT_CONFIG })).not.toThrow()
  })
})

describe('AI 批注同步', () => {
  beforeEach(() => setActivePinia(createPinia()))

  function mkNote(id: string, no: number) {
    return { id, no, ts: '00:00:01.000', text: 'ERR line', note: `note-${id}`, at: 1000 }
  }

  it('applyBridgeAnnotations 整包替换，未知会话静默', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-ai', CFG)
    store.applyBridgeAnnotations(id, [mkNote('a', 3), mkNote('b', 9)])
    expect(store.sessions[id]!.aiNotes.map((n) => n.id)).toEqual(['a', 'b'])
    // 事件重放：整包替换语义（不是追加）
    store.applyBridgeAnnotations(id, [mkNote('c', 5)])
    expect(store.sessions[id]!.aiNotes.map((n) => n.id)).toEqual(['c'])
    expect(() => store.applyBridgeAnnotations('nope', [mkNote('x', 1)])).not.toThrow()
  })

  it('clearLog 连带清空 AI 批注（行号重计数后失义）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-ai2', CFG)
    store.appendLines(id, [mkRaw(100), mkRaw(200)])
    store.applyBridgeAnnotations(id, [mkNote('a', 1)])
    await store.clearLog(id)
    expect(store.sessions[id]!.aiNotes).toEqual([])
  })
})
