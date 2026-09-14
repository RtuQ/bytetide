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

import ScenarioEditor from '../ScenarioEditor.vue'
import { useAutomationStore } from '../../stores/automation'
import { makeScenario, MAX_NESTING_DEPTH, type Scenario, type ScenarioStep } from '../../types/automation'

/** 构造 n 层嵌套 Repeat（最内层一个 delay 叶） */
function nest(n: number): ScenarioStep {
  let step: ScenarioStep = { kind: 'delay', ms: 1 }
  for (let i = 0; i < n; i++) step = { kind: 'repeat', times: 1, steps: [step] }
  return step
}

describe('ScenarioEditor（步骤树编辑）', () => {
  let pinia: Pinia
  beforeEach(() => {
    pinia = createPinia()
    setActivePinia(pinia)
    for (const fn of Object.values(cmd)) {
      fn.mockReset()
      fn.mockImplementation(async () => null as unknown)
    }
  })

  function mountEditor(entryId: string) {
    return mount(ScenarioEditor, {
      props: { entryId },
      global: { plugins: [getActivePinia() as Pinia] },
    })
  }

  function addEntry(sc: Scenario): string {
    const automation = useAutomationStore()
    const id = automation.addScenario(sc)
    automation.editingId = null // 编辑态由测试挂载显式控制
    return id as string
  }

  it('渲染步骤行与种类标签（发送/延时）', () => {
    const id = addEntry(
      makeScenario('demo', {
        steps: [
          { kind: 'send', mode: 'ascii', text: 'hi', appendNewline: true },
          { kind: 'delay', ms: 100 },
        ],
      }),
    )
    const w = mountEditor(id)
    const rows = w.findAll('.sc-row')
    expect(rows).toHaveLength(2)
    expect(rows[0].find('.sc-kind').text()).toBe('发送')
    expect(rows[1].find('.sc-kind').text()).toBe('延时')
  })

  it('顶层调色板添加步骤（延时/信号），保存经校验落库', async () => {
    const automation = useAutomationStore()
    const id = addEntry(makeScenario('demo'))
    const w = mountEditor(id)
    const topAdd = w.findAll('.sc-addrow')[0] // 首个 addrow = 顶层
    await topAdd.find('[data-kind="delay"]').trigger('click')
    await topAdd.find('[data-kind="signal"]').trigger('click')
    expect(w.findAll('.sc-row')).toHaveLength(3)
    cmd.scenarioValidate.mockResolvedValue({ ok: true, stepCount: 3, name: 'demo' })
    await w.find('.sc-save').trigger('click')
    await flushPromises()
    const saved = automation.entries.find((e) => e.id === id)?.scenario
    expect(saved?.steps.map((s) => s.kind)).toEqual(['send', 'delay', 'signal'])
    expect(cmd.scenarioValidate).toHaveBeenCalled()
    expect(w.emitted('close')).toBeTruthy() // 保存成功回列表
  })

  it('上移/下移重排（按钮=键盘可达）', async () => {
    const id = addEntry(
      makeScenario('demo', {
        steps: [
          { kind: 'send', mode: 'ascii', text: 'a', appendNewline: false },
          { kind: 'delay', ms: 1 },
          { kind: 'signal', pin: 'rts', level: false },
        ],
      }),
    )
    const w = mountEditor(id)
    let rows = w.findAll('.sc-row')
    // 第二行（延时）上移 → [延时, 发送, 信号]
    await rows[1].find('.sc-up').trigger('click')
    rows = w.findAll('.sc-row')
    expect(rows[0].find('.sc-kind').text()).toBe('延时')
    // 首行上移按钮在顶（禁用），下移恢复
    expect((rows[0].find('.sc-up').element as HTMLButtonElement).disabled).toBe(true)
    await rows[0].find('.sc-down').trigger('click')
    rows = w.findAll('.sc-row')
    expect(rows[0].find('.sc-kind').text()).toBe('发送')
  })

  it('删除步骤', async () => {
    const id = addEntry(
      makeScenario('demo', {
        steps: [
          { kind: 'send', mode: 'ascii', text: 'a', appendNewline: false },
          { kind: 'delay', ms: 1 },
        ],
      }),
    )
    const w = mountEditor(id)
    await w.findAll('.sc-row')[0].find('.sc-del').trigger('click')
    expect(w.findAll('.sc-row')).toHaveLength(1)
    expect(w.findAll('.sc-row')[0].find('.sc-kind').text()).toBe('延时')
  })

  it('嵌套 Repeat：体内调色板添加、视觉缩进（depth）', async () => {
    const id = addEntry(makeScenario('demo'))
    const w = mountEditor(id)
    const topAdd = w.findAll('.sc-addrow')[0] // 初始只有顶层 addrow
    await topAdd.find('[data-kind="repeat"]').trigger('click')
    await w.vm.$nextTick()
    // 出现两层 addrow：DOM 顺序 = repeat 体在前、顶层在后
    expect(w.findAll('.sc-addrow')).toHaveLength(2)
    const bodyAdd = w.findAll('.sc-addrow')[0]
    // 缩进层级区分：体 addrow（depth 1，20px）在前、顶层（6px）在后
    expect(bodyAdd.attributes('style')).toContain('20px')
    await bodyAdd.find('[data-kind="wait"]').trigger('click')
    await w.vm.$nextTick()
    const rows = w.findAll('.sc-row')
    expect(rows).toHaveLength(3) // 初始 send + repeat 头 + 体内 wait
    expect(rows[1].find('.sc-kind').text()).toBe('循环')
    expect(Number(rows[2].attributes('data-depth'))).toBe(1)
  })

  it('嵌套深度 ≤4：第 4 层体内的「循环」按钮禁用', async () => {
    const id = addEntry(makeScenario('demo', { steps: [nest(MAX_NESTING_DEPTH)] }))
    const w = mountEditor(id)
    const addrows = w.findAll('.sc-addrow')
    expect(addrows).toHaveLength(MAX_NESTING_DEPTH + 1) // 顶层 + 每层 repeat 体
    // DOM 顺序：最内层 addrow 紧随最内层叶步，故为第一个
    const innermost = addrows[0]
    const repeatBtn = innermost.find('[data-kind="repeat"]')
    expect((repeatBtn.element as HTMLButtonElement).disabled).toBe(true)
    expect(repeatBtn.attributes('title')).toContain('嵌套')
    // 顶层调色板的「循环」仍可用（新循环为第 1 层）
    const topRepeat = addrows[addrows.length - 1].find('[data-kind="repeat"]')
    expect((topRepeat.element as HTMLButtonElement).disabled).toBe(false)
  })

  it('校验错误按 step path 定位到行（steps[1] → 第二行高亮）', async () => {
    const id = addEntry(
      makeScenario('demo', {
        steps: [
          { kind: 'send', mode: 'ascii', text: 'a', appendNewline: false },
          { kind: 'delay', ms: 999999999 },
        ],
      }),
    )
    const w = mountEditor(id)
    cmd.scenarioValidate.mockResolvedValue({
      ok: false,
      error: { code: 'delay_too_long', path: 'steps[1]', message: 'delay 999999999 ms exceeds maximum 600000 ms' },
      stepCount: 0,
      name: 'demo',
    })
    await w.find('.sc-save').trigger('click')
    await flushPromises()
    expect(w.emitted('close')).toBeFalsy() // 校验失败留在编辑器
    const rows = w.findAll('.sc-row')
    expect(rows[0].classes()).not.toContain('sc-row-error')
    expect(rows[1].classes()).toContain('sc-row-error')
    expect(rows[1].find('.sc-row-err-msg').text()).toContain('delay 999999999')
    expect(w.find('.sc-val-error').text()).toContain('delay_too_long')
  })

  it('变量表：添加/删除变量行', async () => {
    const id = addEntry(makeScenario('demo'))
    const w = mountEditor(id)
    expect(w.findAll('.sc-var-row')).toHaveLength(0)
    await w.find('.sc-var-add').trigger('click')
    expect(w.findAll('.sc-var-row')).toHaveLength(1)
    await w.find('.sc-var-row .sc-var-del').trigger('click')
    expect(w.findAll('.sc-var-row')).toHaveLength(0)
  })

  it('可访问性：全部按钮 type=button；图标按钮带 aria-label', () => {
    const id = addEntry(
      makeScenario('demo', {
        steps: [
          { kind: 'send', mode: 'ascii', text: 'a', appendNewline: false },
          { kind: 'repeat', times: 1, steps: [{ kind: 'delay', ms: 1 }] },
        ],
      }),
    )
    const w = mountEditor(id)
    for (const b of w.findAll('button')) {
      expect(b.attributes('type')).toBe('button')
      const label = b.attributes('aria-label')
      const hasText = b.text().trim().length > 0
      expect(label !== undefined || hasText).toBe(true) // 图标按钮必须有 aria-label
    }
  })
})
