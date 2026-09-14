// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia, getActivePinia, type Pinia } from 'pinia'

// ---- ipc 打桩：组件与 session/automation store 的后端交互全部走门面 ----
const cmd = vi.hoisted(() => ({
  scenarioValidate: vi.fn(),
  scenarioStart: vi.fn(),
  scenarioStop: vi.fn(),
  scenarioStatus: vi.fn(),
  scenarioReport: vi.fn(),
  readTextFile: vi.fn(),
  exportText: vi.fn(),
}))
vi.mock('../../ipc/commands', () => ({ commands: cmd }))

const ev = vi.hoisted(() => ({
  onScenarioProgress: vi.fn(),
  onScenarioFinished: vi.fn(),
  onSessionStatus: vi.fn(),
}))
vi.mock('../../ipc/events', () => ev)

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
}))

import ScenarioPanel from '../ScenarioPanel.vue'
import ScenarioEditor from '../ScenarioEditor.vue'
import { useAutomationStore } from '../../stores/automation'
import { useSessionStore } from '../../stores/session'
import { createSession } from '../../stores/session/model'
import { makeScenario, type Scenario } from '../../types/automation'
import type { PortConfig } from '../../types'
import { setStorageBackend, type StorageLike } from '../../persistence/storage'

const CFG: PortConfig = {
  name: 'COM1',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
  transport: 'serial',
}

/** send + repeat(3){delay} → 静态 4 叶步 */
function leafyScenario(name: string): Scenario {
  return makeScenario(name, {
    steps: [
      { kind: 'send', mode: 'ascii', text: 'hi', appendNewline: true },
      { kind: 'repeat', times: 3, steps: [{ kind: 'delay', ms: 10 }] },
    ],
  })
}

/** 预置库（写 localStorage 信封 + load）：面板 onMounted 的 load 有首次守卫，
 *  与生产时序一致——盘上数据先于挂载存在 */
function seedLibrary(entries: { id: string; scenario: Scenario }[]): void {
  const s: StorageLike = (() => {
    const m = new Map<string, string>()
    m.set(
      'serialtool.scenarios',
      JSON.stringify({
        schema: 'bytetide.scenario-library',
        version: 1,
        data: entries.map((e) => ({ ...e, scenario: JSON.parse(JSON.stringify(e.scenario)) })),
      }),
    )
    return {
      getItem: (k) => m.get(k) ?? null,
      setItem: (k, v) => void m.set(k, v),
      removeItem: (k) => void m.delete(k),
    }
  })()
  setStorageBackend(s)
  useAutomationStore().load()
}

describe('ScenarioPanel（侧栏场景库）', () => {
  let pinia: Pinia
  beforeEach(() => {
    pinia = createPinia()
    setActivePinia(pinia)
    for (const fn of Object.values(cmd)) {
      fn.mockReset()
      fn.mockImplementation(async () => null as unknown)
    }
    for (const fn of Object.values(ev)) {
      fn.mockReset()
      fn.mockImplementation(async () => () => {})
    }
  })

  afterEach(() => {
    setStorageBackend(null) // 防注入存储泄漏到后续测试
  })

  function mountPanel() {
    return mount(ScenarioPanel, { global: { plugins: [getActivePinia() as Pinia] } })
  }

  it('库列表渲染名称、静态叶步数与最近运行徽标', async () => {
    seedLibrary([
      { id: 'a1', scenario: leafyScenario('压测') },
      { id: 'a2', scenario: makeScenario('另一个') },
    ])
    const automation = useAutomationStore()
    const w = mountPanel()
    const items = w.findAll('.sc-item')
    expect(items).toHaveLength(2)
    expect(items[0].text()).toContain('压测')
    expect(items[0].text()).toContain('4 步') // send 1 + repeat(3){delay} 展开
    expect(items[1].text()).toContain('1 步')
    // 运行后徽标出现（Vue 渲染异步，先 nextTick）
    automation.entries[0].lastRunStatus = 'failed'
    await w.vm.$nextTick()
    expect(w.find('.sc-runbadge.sc-st-failed').exists()).toBe(true)
    expect(w.find('.sc-runbadge').text()).toBe('失败')
  })

  it('新建：入库并进入编辑器；关闭回到列表', async () => {
    const automation = useAutomationStore()
    const w = mountPanel()
    await w.find('.sc-new').trigger('click')
    expect(automation.entries).toHaveLength(1)
    expect(automation.editingId).toBe(automation.entries[0].id)
    expect(w.findComponent(ScenarioEditor).exists()).toBe(true)
    // 编辑器 close → 回列表
    automation.editingId = null
    await w.vm.$nextTick()
    expect(w.findComponent(ScenarioEditor).exists()).toBe(false)
    expect(w.findAll('.sc-item')).toHaveLength(1)
  })

  it('目标会话下拉只列 live+connected 会话；运行按钮调 scenarioStart 并亮运行中徽标', async () => {
    const ss = useSessionStore()
    ss.sessions['s1'] = { ...createSession('s1', CFG), status: 'connected' }
    ss.order.push('s1')
    ss.sessions['s2'] = { ...createSession('s2', CFG), status: 'connecting' } // 未连接不可跑
    ss.order.push('s2')
    const offline = { ...createSession('s3', CFG), status: 'offline' as const, kind: 'offline' as const }
    ss.sessions['s3'] = offline
    ss.order.push('s3')

    seedLibrary([{ id: 'a1', scenario: makeScenario('压测') }])

    const automation = useAutomationStore()
    cmd.scenarioStart.mockResolvedValue('run1')
    const w = mountPanel()
    const select = w.find('select.sc-target')
    expect(select.exists()).toBe(true)
    const optionValues = select.findAll('option').map((o) => o.element.value)
    expect(optionValues).toEqual(['s1']) // connecting/offline 均不可选

    await select.setValue('s1')
    await w.find('.sc-item .sc-run').trigger('click')
    await flushPromises()
    expect(cmd.scenarioStart).toHaveBeenCalledTimes(1)
    expect(cmd.scenarioStart).toHaveBeenCalledWith('s1', expect.objectContaining({ name: '压测' }))
    expect(automation.runs['run1'].status).toBe('running')
    expect(automation.entries.find((e) => e.id === 'a1')?.lastRunStatus).toBe('running')
    // 运行视图出现（运行区）
    expect(w.findAll('.sc-runs .sc-runview')).toHaveLength(1)
  })

  it('无可运行会话时运行按钮禁用并提示', () => {
    seedLibrary([{ id: 'a1', scenario: makeScenario('a') }])
    const w = mountPanel()
    const runBtn = w.find('.sc-item .sc-run')
    expect(runBtn.attributes('disabled')).toBeDefined()
    expect(w.find('.sc-no-target').exists()).toBe(true)
  })

  it('删除/复制走 store；复制后出现「副本」', async () => {
    seedLibrary([{ id: 'a1', scenario: makeScenario('原版') }])
    const automation = useAutomationStore()
    const w = mountPanel()
    await w.find('.sc-item .sc-dup').trigger('click')
    expect(automation.entries.map((e) => e.scenario.name)).toEqual(['原版', '原版 副本'])
    await w.find('.sc-item .sc-del').trigger('click')
    expect(automation.entries).toHaveLength(1)
  })

  it('挂载订阅三个事件、卸载退订（attach/detach 接线）', async () => {
    const unl1 = vi.fn()
    ev.onScenarioProgress.mockResolvedValue(unl1)
    const unl2 = vi.fn()
    ev.onScenarioFinished.mockResolvedValue(unl2)
    const unl3 = vi.fn()
    ev.onSessionStatus.mockResolvedValue(unl3)
    const w = mountPanel()
    await flushPromises()
    expect(ev.onScenarioProgress).toHaveBeenCalledTimes(1)
    expect(ev.onScenarioFinished).toHaveBeenCalledTimes(1)
    expect(ev.onSessionStatus).toHaveBeenCalledTimes(1)
    w.unmount()
    expect(unl1).toHaveBeenCalled()
    expect(unl2).toHaveBeenCalled()
    expect(unl3).toHaveBeenCalled()
  })

  it('导入：解析文件 JSON 交 store.importScenarios，结果提示', async () => {
    const dialog = await import('@tauri-apps/plugin-dialog')
    const automation = useAutomationStore()
    const w = mountPanel()
    vi.mocked(dialog.open).mockResolvedValue('/tmp/scenarios.json')
    cmd.readTextFile.mockResolvedValue(
      JSON.stringify([makeScenario('文件A'), { bogus: true }]),
    )
    await w.find('.sc-import').trigger('click')
    await flushPromises()
    expect(cmd.readTextFile).toHaveBeenCalledWith('/tmp/scenarios.json')
    expect(automation.entries.some((e) => e.scenario.name === '文件A')).toBe(true)
    expect(w.find('.sc-msg').text()).toContain('已导入 1')
  })
})
