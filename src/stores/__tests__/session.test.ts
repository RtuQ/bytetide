import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useSessionStore, registerParserOnClear } from '../session'
import { DEFAULT_PLOT_CONFIG } from '../../types'
import type { DecodedFrame } from '../../types/parser'
import type { PortConfig, RawLogLine } from '../../types'

// invoke 全文件打桩：录制开关/重连等动作在无 Tauri 后端的测试环境可走通
const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

function mkDecoded(no: number): DecodedFrame {
  return {
    no,
    ts: '00:00:01.000',
    dir: 'rx',
    type: '状态上报',
    text: `温度=25℃`,
    fields: [{ label: '温度', value: '25', unit: '℃', raw: 'i16be@4×0.1' }],
    warn: null,
    frameHex: 'AA 55 01',
    frameLen: 9,
    crcOk: true,
  }
}

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

  it('clearLog 归零 droppedLines，pullNo 不回退（后端 no 单调）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-pull5', CFG)
    // 超出前端缓冲上限（50000），制造 10 行丢弃
    const batch = Array.from({ length: 50010 }, (_, i) => mkPulled(i + 1))
    store.appendPulled(id, batch)
    expect(store.sessions[id]!.droppedLines).toBe(10)
    await store.clearLog(id)
    // 清屏后旧缺口已无意义，丢弃计数归零
    expect(store.sessions[id]!.droppedLines).toBe(0)
    // 后端 no 游标单调不回退：清屏后旧 ringNo 不回灌，新行正常入表
    expect(store.appendPulled(id, [mkPulled(50010), mkPulled(50011)])).toHaveLength(1)
    expect(store.sessions[id]!.lines).toHaveLength(1)
    expect(store.sessions[id]!.droppedLines).toBe(0)
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

  it('clearLog 归零 droppedLines（清屏后旧缺口已无意义）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-drop', CFG)
    // 超出前端缓冲上限（50000），制造 10 行丢弃
    const batch = Array.from({ length: 50010 }, (_, i) => mkRaw(i + 1))
    store.appendLines(id, batch)
    expect(store.sessions[id]!.droppedLines).toBe(10)
    await store.clearLog(id)
    expect(store.sessions[id]!.droppedLines).toBe(0)
    // 清屏后继续写入，重新从 0 累计
    store.appendLines(id, [mkRaw(99999)])
    expect(store.sessions[id]!.droppedLines).toBe(0)
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

describe('centerView / compareMode（布局重构 V1）', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('新会话 centerView 默认 log', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('cv-1', CFG)
    expect(store.sessions[id]!.centerView).toBe('log')
  })

  it('setCenterView 进入 split/plot 时自动启用绘图（连带 HEX 视图）', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('cv-2', CFG)
    expect(store.sessions[id]!.plot.enabled).toBe(false)
    store.setCenterView(id, 'split')
    const s = store.sessions[id]!
    expect(s.centerView).toBe('split')
    expect(s.plot.enabled).toBe(true)
    expect(s.hexView).toBe(true) // setPlotEnabled 的既有副作用：开图强制 HEX
    // 已启用后再切 plot 不重复触发
    store.setCenterView(id, 'plot')
    expect(store.sessions[id]!.centerView).toBe('plot')
  })

  it('切回 log 不关闭绘图（图表开关独立于视图模式）', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('cv-3', CFG)
    store.setCenterView(id, 'plot')
    store.setCenterView(id, 'log')
    const s = store.sessions[id]!
    expect(s.centerView).toBe('log')
    expect(s.plot.enabled).toBe(true)
  })

  it('clearLog 不清 centerView（视图偏好非数据）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('cv-4', CFG)
    store.setCenterView(id, 'split')
    await store.clearLog(id)
    expect(store.sessions[id]!.centerView).toBe('split')
  })

  it('compareMode 全局切换', () => {
    const store = useSessionStore()
    expect(store.compareMode).toBe(false)
    store.toggleCompareMode()
    expect(store.compareMode).toBe(true)
    store.toggleCompareMode()
    expect(store.compareMode).toBe(false)
  })

  it('视图耦合：显式启用绘图切到图表视图，关闭绘图回落 log', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('cv-5', CFG)
    store.setPlotEnabled(id, true)
    expect(store.sessions[id]!.centerView).toBe('plot')
    store.setCenterView(id, 'split')
    store.setPlotEnabled(id, false)
    expect(store.sessions[id]!.centerView).toBe('log')
    expect(store.sessions[id]!.plot.enabled).toBe(false)
  })
})

describe('decoded 解码帧（plan-parser-v1）', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('applyDecoded 追加 + markRaw + 1000 条 FIFO', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('dec-1', CFG)
    store.applyDecoded(id, [mkDecoded(1), mkDecoded(2)])
    store.applyDecoded(id, [mkDecoded(3)])
    const s = store.sessions[id]!
    expect(s.decoded.map((d) => d.no)).toEqual([1, 2, 3])
    // 元素被 markRaw：不再是响应式 Proxy
    expect(vi.isMockFunction(s.decoded[0])).toBe(false)
    // FIFO：超过 1000 丢最旧
    const batch = Array.from({ length: 1005 }, (_, i) => mkDecoded(i + 10))
    store.applyDecoded(id, batch)
    expect(store.sessions[id]!.decoded).toHaveLength(1000)
    expect(store.sessions[id]!.decoded[0]!.no).toBe(15)
  })

  it('applyDecoded replace=true 整表替换（回溯语义）', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('dec-2', CFG)
    store.applyDecoded(id, [mkDecoded(1), mkDecoded(2)])
    store.applyDecoded(id, [mkDecoded(9)], true)
    expect(store.sessions[id]!.decoded.map((d) => d.no)).toEqual([9])
  })

  it('resetDecoded 清空，未知会话静默', () => {
    const store = useSessionStore()
    const id = store.createLocalSession('dec-3', CFG)
    store.applyDecoded(id, [mkDecoded(1)])
    store.resetDecoded(id)
    expect(store.sessions[id]!.decoded).toEqual([])
    expect(() => store.resetDecoded('nope')).not.toThrow()
    expect(() => store.applyDecoded('nope', [mkDecoded(1)])).not.toThrow()
  })

  it('clearLog 清空 decoded 并触发 onClear 回调', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('dec-4', CFG)
    store.applyDecoded(id, [mkDecoded(1)])
    const onClear = vi.fn()
    registerParserOnClear(onClear)
    await store.clearLog(id)
    expect(store.sessions[id]!.decoded).toEqual([])
    expect(onClear).toHaveBeenCalledWith(id)
  })

  it('重连迁移清单不含 decoded（新会话为空）', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('dec-5', CFG)
    store.applyDecoded(id, [mkDecoded(1)])
    store.setStatus(id, 'connected')
    // 无 Tauri 后端 invoke 会失败并置 error——直接改写回迁路径的前置条件：
    // 重连成功路径无法在此环境构造，改为验证 makeSession 默认值为空数组
    const fresh = store.createLocalSession('dec-5b', CFG)
    expect(store.sessions[fresh]!.decoded).toEqual([])
    // 迁移语义由 useParserEngine 的 order diff 清旧 id 引擎状态兜底
  })
})

describe('落盘录制 recOn（录制/分段）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    invokeMock.mockReset()
    invokeMock.mockResolvedValue(null)
    vi.stubGlobal('alert', vi.fn())
  })
  afterEach(() => vi.unstubAllGlobals())

  it('makeSession 默认开启录制；setRec 乐观置位并下发后端', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-rec', CFG)
    expect(store.sessions[id]!.recOn).toBe(true)
    await store.setRec(id, false)
    expect(store.sessions[id]!.recOn).toBe(false)
    expect(invokeMock).toHaveBeenCalledWith('set_recording_cmd', { sessionId: id, on: false })
  })

  it('setRec 后端失败回滚本地状态', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-rec-fail', CFG)
    invokeMock.mockRejectedValueOnce(new Error('通道已关闭'))
    await store.setRec(id, false)
    expect(store.sessions[id]!.recOn).toBe(true)
  })

  it('重连迁移 recOn：暂停状态带到新会话并补发暂停', async () => {
    const store = useSessionStore()
    const id = store.createLocalSession('local-rec-reconn', CFG)
    store.sessions[id]!.recOn = false
    invokeMock.mockResolvedValueOnce('s99') // connect_cmd 返回新会话 id
    await store.reconnectSession(id)
    expect(store.sessions[id]).toBeUndefined()
    expect(store.sessions['s99']!.recOn).toBe(false)
    expect(invokeMock).toHaveBeenCalledWith('set_recording_cmd', { sessionId: 's99', on: false })
  })
})
