// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia, getActivePinia, type Pinia } from 'pinia'

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
  onScenarioProgress: vi.fn(async () => () => {}),
  onScenarioFinished: vi.fn(async () => () => {}),
  onSessionStatus: vi.fn(async () => () => {}),
}))
vi.mock('../../ipc/events', () => ev)

vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
}))

import ScenarioRunView from '../ScenarioRunView.vue'
import { useAutomationStore } from '../../stores/automation'
import type { AutomationRunView } from '../../stores/automation'

describe('ScenarioRunView（运行视图）', () => {
  let pinia: Pinia
  beforeEach(() => {
    pinia = createPinia()
    setActivePinia(pinia)
    for (const fn of Object.values(cmd)) {
      fn.mockReset()
      fn.mockImplementation(async () => null as unknown)
    }
  })

  function putRun(overrides: Partial<AutomationRunView>): string {
    const automation = useAutomationStore()
    const run: AutomationRunView = {
      runId: 'run1',
      sessionId: 's1',
      status: 'running',
      startedEpochMs: 1_700_000_000_000,
      ...overrides,
    }
    automation.runs[run.runId] = run
    automation.runOrder.unshift(run.runId)
    return run.runId
  }

  function mountView(runId: string) {
    return mount(ScenarioRunView, {
      props: { runId },
      global: { plugins: [getActivePinia() as Pinia] },
    })
  }

  it('运行中：进度 currentStep/totalSteps、当前步骤种类、停止按钮（aria-live 进度聚焦）', () => {
    putRun({ progress: { currentStep: 2, totalSteps: 5 }, kind: 'wait' })
    const w = mountView('run1')
    expect(w.find('.sc-status').classes()).toContain('st-running')
    expect(w.find('.sc-progress').text()).toContain('2 / 5')
    expect(w.find('.sc-progress').attributes('aria-live')).toBe('polite')
    expect(w.find('.sc-kind-now').text()).toContain('等待')
    expect(w.find('.sc-stop').exists()).toBe(true)
    expect(w.find('.sc-report-json').exists()).toBe(false) // 未完成无报告按钮
  })

  it('进度事件实时更新视图（progress 聚焦）', async () => {
    putRun({ progress: { currentStep: 2, totalSteps: 5 }, kind: 'wait' })
    const automation = useAutomationStore()
    const w = mountView('run1')
    automation.onProgress({
      runId: 'run1',
      sessionId: 's1',
      currentStep: 3,
      totalSteps: 5,
      kind: 'send',
    })
    await w.vm.$nextTick()
    expect(w.find('.sc-progress').text()).toContain('3 / 5')
    expect(w.find('.sc-kind-now').text()).toContain('发送')
  })

  it('停止按钮：调 scenario_stop_cmd，本地转 cancelled', async () => {
    putRun({})
    const automation = useAutomationStore()
    cmd.scenarioStop.mockResolvedValue(undefined)
    const w = mountView('run1')
    await w.find('.sc-stop').trigger('click')
    await flushPromises()
    expect(cmd.scenarioStop).toHaveBeenCalledWith('run1')
    expect(automation.runs['run1'].status).toBe('cancelled')
  })

  it('失败终态：展示错误与耗时，提供 JSON/JUnit 报告保存', async () => {
    putRun({
      status: 'failed',
      durationMs: 250,
      error: 'wait_timeout: no matching line within 10 ms',
    })
    const dialog = await import('@tauri-apps/plugin-dialog')
    const w = mountView('run1')
    expect(w.find('.sc-status').classes()).toContain('st-failed')
    expect(w.text()).toContain('wait_timeout: no matching line within 10 ms')
    expect(w.find('.sc-stop').exists()).toBe(false)

    cmd.scenarioReport.mockResolvedValue('{"status":"failed"}')
    vi.mocked(dialog.save).mockResolvedValue('/tmp/run1.json')
    await w.find('.sc-report-json').trigger('click')
    await flushPromises()
    expect(cmd.scenarioReport).toHaveBeenCalledWith('run1', 'json')
    expect(cmd.exportText).toHaveBeenCalledWith('/tmp/run1.json', '{"status":"failed"}')

    // JUnit：xml 扩展名
    vi.mocked(dialog.save).mockClear()
    cmd.exportText.mockClear()
    vi.mocked(dialog.save).mockResolvedValue('/tmp/run1.xml')
    await w.find('.sc-report-junit').trigger('click')
    await flushPromises()
    expect(cmd.scenarioReport).toHaveBeenCalledWith('run1', 'junit')
    expect(cmd.exportText).toHaveBeenCalledWith('/tmp/run1.xml', '{"status":"failed"}')
  })

  it('报告对话框取消（路径为空）不写盘', async () => {
    putRun({ status: 'passed', durationMs: 5 })
    const dialog = await import('@tauri-apps/plugin-dialog')
    vi.mocked(dialog.save).mockResolvedValue(null as unknown as string)
    cmd.scenarioReport.mockResolvedValue('<testsuite/>')
    const w = mountView('run1')
    await w.find('.sc-report-json').trigger('click')
    await flushPromises()
    expect(cmd.exportText).not.toHaveBeenCalled()
  })

  it('未知 runId：空态渲染不崩', () => {
    const w = mountView('ghost')
    expect(w.find('.sc-runview-empty').exists()).toBe(true)
  })
})
