import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { setStorageBackend, type StorageLike } from '../../persistence/storage'
import {
  makeScenario,
  parseScenario,
  type Scenario,
  type ScenarioLibraryEntry,
} from '../../types/automation'

// ---- ipc 打桩：store 的全部后端交互经 src/ipc 门面，测试注入假实现 ----
const cmdMocks = vi.hoisted(() => ({
  scenarioValidate: vi.fn(),
  scenarioStart: vi.fn(),
  scenarioStop: vi.fn(),
  scenarioStatus: vi.fn(),
  scenarioReport: vi.fn(),
}))
vi.mock('../../ipc/commands', () => ({ commands: cmdMocks }))

const eventMocks = vi.hoisted(() => ({
  onScenarioProgress: vi.fn(async () => () => {}),
  onScenarioFinished: vi.fn(async () => () => {}),
  onSessionStatus: vi.fn(async () => () => {}),
}))
vi.mock('../../ipc/events', () => eventMocks)

import { useAutomationStore, SCENARIO_LIBRARY_CAP, newScenarioId } from '../automation'

// ---- 内存 localStorage（persistence 可注入后端） ----
function memStorage(): StorageLike & { dump: (k: string) => unknown } {
  const m = new Map<string, string>()
  return {
    getItem: (k) => m.get(k) ?? null,
    setItem: (k, v) => void m.set(k, v),
    removeItem: (k) => void m.delete(k),
    dump: (k) => (m.has(k) ? (JSON.parse(m.get(k) as string) as unknown) : undefined),
  }
}

const KEY = 'serialtool.scenarios'
const SCHEMA = 'bytetide.scenario-library'

function mkScenario(name: string, patch: Partial<Scenario> = {}): Scenario {
  return { ...makeScenario(name), ...patch }
}

function mkEntry(id: string, name: string): ScenarioLibraryEntry {
  return { id, scenario: mkScenario(name) }
}

beforeEach(() => {
  setActivePinia(createPinia())
  for (const fn of Object.values(cmdMocks)) {
    fn.mockReset()
    fn.mockImplementation(async () => null as unknown)
  }
  for (const fn of Object.values(eventMocks)) {
    fn.mockClear()
  }
})

afterEach(() => {
  setStorageBackend(null)
  const store = useAutomationStore()
  store.detach()
})

// ===================== 库持久化（load/persist/cap） =====================

describe('场景库 load/persist（serialtool.scenarios 信封）', () => {
  it('load：v1 信封读回条目', () => {
    const s = memStorage()
    s.setItem(
      KEY,
      JSON.stringify({ schema: SCHEMA, version: 1, data: [mkEntry('a1', '演示')] }),
    )
    setStorageBackend(s)
    const store = useAutomationStore()
    store.load()
    expect(store.loaded).toBe(true)
    expect(store.entries).toHaveLength(1)
    expect(store.entries[0].id).toBe('a1')
    expect(store.entries[0].scenario.name).toBe('演示')
  })

  it('load：键缺失 → 空库不落盘', () => {
    const s = memStorage()
    setStorageBackend(s)
    const store = useAutomationStore()
    store.load()
    expect(store.entries).toEqual([])
    expect(s.getItem(KEY)).toBeNull()
  })

  it('load：条目形状烂（缺 id / 步骤空）→ invalid，库为空且原键被重置', () => {
    const s = memStorage()
    s.setItem(
      KEY,
      JSON.stringify({ schema: SCHEMA, version: 1, data: [{ id: 'x', scenario: { name: 'no steps' } }] }),
    )
    setStorageBackend(s)
    const store = useAutomationStore()
    store.load()
    expect(store.entries).toEqual([])
    expect(s.getItem(KEY)).toBeNull() // invalid 已被重置（备份键存在与否由 storage 层管）
  })

  it('load：旧裸数组（无信封）走迁移并回写信封', () => {
    const s = memStorage()
    s.setItem(KEY, JSON.stringify([mkEntry('lg1', '旧数据')]))
    setStorageBackend(s)
    const store = useAutomationStore()
    store.load()
    expect(store.entries).toHaveLength(1)
    const raw = s.dump(KEY) as { schema?: string; version?: number; data?: unknown[] }
    expect(raw.schema).toBe(SCHEMA)
    expect(raw.version).toBe(1)
    expect((raw.data as unknown[]).length).toBe(1)
  })

  it('load：未来版本信封 → invalid 且不动现场', () => {
    const s = memStorage()
    const future = JSON.stringify({ schema: SCHEMA, version: 99, data: [] })
    s.setItem(KEY, future)
    setStorageBackend(s)
    const store = useAutomationStore()
    store.load()
    expect(store.entries).toEqual([])
    expect(s.getItem(KEY)).toBe(future)
  })

  it('persist：修改后写回 v1 信封', async () => {
    const s = memStorage()
    setStorageBackend(s)
    const store = useAutomationStore()
    cmdMocks.scenarioValidate.mockResolvedValue({ ok: true, stepCount: 1, name: 'a' })
    const id = store.addScenario(mkScenario('  空格名  '))
    await store.saveScenario(id as string, mkScenario('空格名'))
    const raw = s.dump(KEY) as { schema?: string; version?: number; data?: unknown[] }
    expect(raw.schema).toBe(SCHEMA)
    expect(raw.version).toBe(1)
    expect((raw.data as ScenarioLibraryEntry[])[0].scenario.name).toBe('空格名')
    // 重开 store（新 pinia）从盘上读回
    setActivePinia(createPinia())
    const store2 = useAutomationStore()
    store2.load()
    expect(store2.entries.some((e) => e.scenario.name === '空格名')).toBe(true)
  })

  it('cap：库上限 100，超出丢最旧', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    for (let i = 0; i < SCENARIO_LIBRARY_CAP + 5; i++) {
      store.addScenario(mkScenario(`s${i}`))
    }
    expect(store.entries).toHaveLength(SCENARIO_LIBRARY_CAP)
    expect(store.entries[0].scenario.name).toBe('s5') // 最旧 5 条被挤出
    expect(store.entries[store.entries.length - 1].scenario.name).toBe(
      `s${SCENARIO_LIBRARY_CAP + 4}`,
    )
  })
})

// ===================== 增删改 / 导入导出 =====================

describe('场景库 add/rename/duplicate/delete/import/export', () => {
  it('add：trim 名称、生成 id、选入编辑', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('  新场景 '))
    expect(id).toBeTypeOf('string')
    expect(store.entries[0].scenario.name).toBe('新场景')
    expect(store.editingId).toBe(id)
    expect(store.editing?.id).toBe(id)
  })

  it('add：空名拒绝', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    expect(store.addScenario(mkScenario('   '))).toBeNull()
    expect(store.entries).toHaveLength(0)
  })

  it('rename：更新 scenario.name；空名/未知 id 拒绝', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('旧名')) as string
    expect(store.renameScenario(id, ' 新名 ')).toBe(true)
    expect(store.entries[0].scenario.name).toBe('新名')
    expect(store.renameScenario(id, '  ')).toBe(false)
    expect(store.renameScenario('nope', 'x')).toBe(false)
  })

  it('duplicate：深拷贝 + 新 id + 「副本」后缀，互不影响', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('原版')) as string
    const dupId = store.duplicateScenario(id) as string
    expect(dupId).not.toBe(id)
    const orig = store.entries.find((e) => e.id === id) as ScenarioLibraryEntry
    const dup = store.entries.find((e) => e.id === dupId) as ScenarioLibraryEntry
    expect(dup.scenario.name).toBe('原版 副本')
    expect(dup.scenario.steps).not.toBe(orig.scenario.steps)
    orig.scenario.steps[0] = { kind: 'delay', ms: 5 }
    expect(dup.scenario.steps[0].kind).toBe('send')
    expect(store.duplicateScenario('nope')).toBeNull()
  })

  it('delete：删除条目；编辑中的被删则清 editingId', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    store.removeScenario(id)
    expect(store.entries).toHaveLength(0)
    expect(store.editingId).toBeNull()
  })

  it('export：单场景 / 整库均可反序列化还原', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('导出我')) as string
    const one = JSON.parse(store.exportScenario(id) as string) as Scenario
    expect(one.name).toBe('导出我')
    expect(parseScenario(one).name).toBe('导出我')
    expect(store.exportScenario('nope')).toBeNull()
    const all = JSON.parse(store.exportLibrary()) as Scenario[]
    expect(all).toHaveLength(1)
  })

  it('import：数组 / 单个 / {scenarios} 三种形状，坏条目跳过，id 重生成防撞', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('已有')) as string
    const existingSchema = store.entries[0].scenario.schema
    const fileScen = mkScenario('文件场景')
    expect(store.importScenarios([fileScen, { bogus: true }, mkScenario('第二个')])).toBe(2)
    expect(store.importScenarios(fileScen)).toBe(1)
    expect(store.importScenarios({ scenarios: [mkScenario('信封内')] })).toBe(1)
    expect(store.importScenarios('not json shape')).toBe(0)
    const names = store.entries.map((e) => e.scenario.name)
    expect(names).toContain('文件场景')
    expect(names).toContain('信封内')
    // 导入条目 id 不与已有冲突
    const ids = store.entries.map((e) => e.id)
    expect(new Set(ids).size).toBe(ids.length)
    // 原 schema 字段原样保留（值校验在后端）
    const imported = store.entries.find((e) => e.scenario.name === '文件场景')
    expect(imported?.scenario.schema).toBe(existingSchema)
    void id
  })

  it('import：超出 cap 丢最旧', () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const arr: Scenario[] = []
    for (let i = 0; i < SCENARIO_LIBRARY_CAP + 3; i++) arr.push(mkScenario(`i${i}`))
    expect(store.importScenarios(arr)).toBe(SCENARIO_LIBRARY_CAP + 3)
    expect(store.entries).toHaveLength(SCENARIO_LIBRARY_CAP)
    expect(store.entries[0].scenario.name).toBe('i3')
  })

  it('newScenarioId：连续生成不撞号', () => {
    const a = newScenarioId()
    const b = newScenarioId()
    expect(a).not.toBe(b)
  })
})

// ===================== 保存预检（scenarioValidate） =====================

describe('saveScenario 校验预检', () => {
  it('校验通过：替换条目并持久化、清 validationError', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioValidate.mockResolvedValue({ ok: true, stepCount: 2, name: 'a' })
    const next = mkScenario('a', { steps: [{ kind: 'delay', ms: 10 }] })
    await expect(store.saveScenario(id, next)).resolves.toBe(true)
    expect(cmdMocks.scenarioValidate).toHaveBeenCalledWith(next)
    expect(store.entries[0].scenario.steps[0].kind).toBe('delay')
    expect(store.validationError).toBeNull()
  })

  it('校验失败：validationError 按 code/path/message 落账，条目不动', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    const err = { code: 'invalid_regex', path: 'steps[0]', message: 'boom' }
    cmdMocks.scenarioValidate.mockResolvedValue({ ok: false, error: err, stepCount: 0, name: 'a' })
    await expect(store.saveScenario(id, mkScenario('a'))).resolves.toBe(false)
    expect(store.validationError).toEqual(err)
    expect(store.entries[0].scenario.steps[0].kind).toBe('send') // 旧内容原样
  })

  it('后端不可用（浏览器冒烟）：结构合法则放行保存', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioValidate.mockRejectedValue('invoke unavailable')
    await expect(store.saveScenario(id, mkScenario('a'))).resolves.toBe(true)
    expect(store.lastError).toContain('invoke unavailable')
  })
})

// ===================== 运行生命周期（start/stop/progress/finish/断开） =====================

describe('运行生命周期', () => {
  it('start：登记 running 视图 + 徽标；后端拒绝则 lastError 且不登记', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioStart.mockResolvedValueOnce('run1')
    const runId = await store.start('sess1', id)
    expect(runId).toBe('run1')
    expect(cmdMocks.scenarioStart).toHaveBeenCalledWith('sess1', expect.objectContaining({ name: 'a' }))
    expect(store.runs['run1'].status).toBe('running')
    expect(store.runs['run1'].sessionId).toBe('sess1')
    expect(store.runOrder[0]).toBe('run1')
    expect(store.entries[0].lastRunStatus).toBe('running')

    cmdMocks.scenarioStart.mockRejectedValueOnce('同一会话同时只允许运行一个场景: sess1')
    expect(await store.start('sess1', id)).toBeNull()
    expect(store.lastError).toContain('同一会话')
    expect(store.runOrder).toHaveLength(1)
  })

  it('start：未知条目返回 null', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    expect(await store.start('sess1', 'nope')).toBeNull()
    expect(cmdMocks.scenarioStart).not.toHaveBeenCalled()
  })

  it('onProgress：更新水位与叶步种类；未知/非运行中 run 忽略', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioStart.mockResolvedValueOnce('run1')
    await store.start('sess1', id)
    store.onProgress({ runId: 'run1', sessionId: 'sess1', currentStep: 2, totalSteps: 5, kind: 'wait' })
    expect(store.runs['run1'].progress).toEqual({ currentStep: 2, totalSteps: 5 })
    expect(store.runs['run1'].kind).toBe('wait')
    // 未知 run / 已完成的 run：迟到事件忽略
    store.onProgress({ runId: 'ghost', sessionId: 's', currentStep: 1, totalSteps: 1, kind: 'send' })
    cmdMocks.scenarioStop.mockResolvedValueOnce(undefined)
    await store.stop('run1')
    store.onProgress({ runId: 'run1', sessionId: 'sess1', currentStep: 3, totalSteps: 5, kind: 'send' })
    expect(store.runs['run1'].progress).toEqual({ currentStep: 2, totalSteps: 5 })
  })

  it('onFinished：终态落账 + 条目徽标更新；未知 run 忽略', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioStart.mockResolvedValueOnce('run1')
    await store.start('sess1', id)
    store.onFinished({
      runId: 'run1',
      sessionId: 'sess1',
      status: 'failed',
      startedEpochMs: 1,
      durationMs: 250,
      error: 'wait_timeout: no matching line within 10 ms',
    })
    expect(store.runs['run1'].status).toBe('failed')
    expect(store.entries[0].lastRunStatus).toBe('failed')
    expect(store.entries[0].lastRunAt).toBeTypeOf('number')
    store.onFinished({ runId: 'ghost', sessionId: 's', status: 'passed', startedEpochMs: 1 })
    expect(store.runs['ghost']).toBeUndefined()
  })

  it('stop：running → cancelled（本地先行，finished 事件随后覆盖）', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioStart.mockResolvedValueOnce('run1')
    await store.start('sess1', id)
    cmdMocks.scenarioStop.mockResolvedValueOnce(undefined)
    await store.stop('run1')
    expect(cmdMocks.scenarioStop).toHaveBeenCalledWith('run1')
    expect(store.runs['run1'].status).toBe('cancelled')
    expect(store.entries[0].lastRunStatus).toBe('cancelled')
    // 幂等：已终态再 stop 不回改
    await store.stop('run1')
    expect(store.runs['run1'].status).toBe('cancelled')
  })

  it('会话断开清理：该会话全部 running 置 cancelled；connected 不动', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const idA = store.addScenario(mkScenario('a')) as string
    const idB = store.addScenario(mkScenario('b')) as string
    cmdMocks.scenarioStart
      .mockResolvedValueOnce('run1')
      .mockResolvedValueOnce('run2')
      .mockResolvedValueOnce('run3')
    await store.start('sess1', idA)
    await store.start('sess1', idB) // 不同条目；后端会拒同会话并发——此处只验前端簿记
    await store.start('sess2', idA)
    store.onSessionStatus({ sessionId: 'sess1', status: 'connected' })
    expect(store.runs['run1'].status).toBe('running')
    store.onSessionStatus({ sessionId: 'sess1', status: 'disconnected' })
    expect(store.runs['run1'].status).toBe('cancelled')
    expect(store.runs['run2'].status).toBe('cancelled')
    expect(store.runs['run3'].status).toBe('running') // 其他会话不受影响
    expect(store.entries[0].lastRunStatus).toBe('cancelled')
  })

  it('runList/latestRun：最新运行在前', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    const id = store.addScenario(mkScenario('a')) as string
    cmdMocks.scenarioStart.mockResolvedValueOnce('run1').mockResolvedValueOnce('run2')
    await store.start('s1', id)
    await store.start('s2', id)
    expect(store.runOrder).toEqual(['run2', 'run1'])
    expect(store.latestRun?.runId).toBe('run2')
  })

  it('report：透传 scenarioReport 并返回文本；失败入 lastError', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    cmdMocks.scenarioReport.mockResolvedValueOnce('<testsuite/>')
    await expect(store.fetchReport('run1', 'junit')).resolves.toBe('<testsuite/>')
    expect(cmdMocks.scenarioReport).toHaveBeenCalledWith('run1', 'junit')
    cmdMocks.scenarioReport.mockRejectedValueOnce('场景尚未完成，报告未生成: run9')
    await expect(store.fetchReport('run9', 'json')).resolves.toBeNull()
    expect(store.lastError).toContain('尚未完成')
  })
})

// ===================== 事件接线（attach/detach） =====================

describe('attach/detach 事件订阅', () => {
  it('attach：订阅三个事件且幂等；detach 退订', async () => {
    setStorageBackend(memStorage())
    const store = useAutomationStore()
    await store.attach()
    expect(eventMocks.onScenarioProgress).toHaveBeenCalledTimes(1)
    expect(eventMocks.onScenarioFinished).toHaveBeenCalledTimes(1)
    expect(eventMocks.onSessionStatus).toHaveBeenCalledTimes(1)
    await store.attach()
    expect(eventMocks.onScenarioProgress).toHaveBeenCalledTimes(1) // 幂等
    store.detach()
    expect(store.subscribed).toBe(false)
    await store.attach()
    expect(eventMocks.onScenarioProgress).toHaveBeenCalledTimes(2)
    store.detach()
  })
})
