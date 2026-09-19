import { defineStore } from 'pinia'
import { commands } from '../ipc/commands'
import { normalizeIpcError } from '../ipc/errors'
import { t } from '../i18n'
import { onScenarioFinished, onScenarioProgress, onSessionStatus } from '../ipc/events'
import type { Unlisten } from '../ipc/client'
import type { StatusPayload } from '../types'
import { scenarioLibraryCodec, SCENARIO_LIBRARY_SCHEMA } from '../persistence/schema'
import { loadStored, saveStored } from '../persistence/storage'
import {
  cloneScenario,
  parseScenario,
  type Scenario,
  type ScenarioErrorInfo,
  type ScenarioLibraryEntry,
  type ScenarioProgressPayload,
  type ScenarioReportFormat,
  type ScenarioRunView,
} from '../types/automation'

/**
 * 场景自动化 store（Stage 3 Task 4）：保存的场景库（localStorage `serialtool.scenarios`，
 * 信封 schema `bytetide.scenario-library` v1，cap 100）+ 运行视图簿记。
 *
 * 职责边界：只持有「库 + run 视图」，不持有会话对象（live 会话列表由组件从
 * session store 只读消费）。后端交互全部经 src/ipc 门面：
 * - 保存先走 `scenarioValidate` 预检，错误按 step path 定位（编辑器行内展示）；
 *   后端不可用（浏览器冒烟）时退化为结构校验放行，运行期仍有后端把关。
 * - start/stop/report 透传命令；进度按 runId reconcile——**未知/已终态 run 的
 *   迟到事件一律忽略**（后端保留最近 50 份 completed，早前的会被逐出）。
 * - 会话断开（session-status 事件 → onSessionStatus）：该会话全部 running 视图
 *   本地置 cancelled（后端 disconnect 亦会取消并补发 finished，事件到达时以
 *   权威视图覆盖）。
 * 事件订阅由 ScenarioPanel 挂载时 attach()（幂等）、卸载 detach()。
 */

export const SCENARIOS_KEY = 'serialtool.scenarios'
export const SCENARIO_LIBRARY_CAP = 100

/** 运行视图（后端 ScenarioRunView + 前端补充：最近一次进度事件的叶步种类） */
export interface AutomationRunView extends ScenarioRunView {
  /** scenario-progress 的 kind：send/signal/delay/wait/assert（叶步近似识别） */
  kind?: string
}

let idSeq = 0
/** 库条目 id（时间戳 + 模块级计数，防同毫秒撞号） */
export function newScenarioId(): string {
  idSeq += 1
  return `sc${Date.now().toString(36)}${idSeq}`
}

/** cap 执行：超出上限丢最旧（与 sendPresets/configPresets 纪律一致） */
function capLibrary(list: ScenarioLibraryEntry[]): ScenarioLibraryEntry[] {
  return list.length > SCENARIO_LIBRARY_CAP ? list.slice(list.length - SCENARIO_LIBRARY_CAP) : list
}

/** 事件订阅的退订函数（非响应式模块状态；attach/detach 交换） */
let unlistens: Unlisten[] = []

export const useAutomationStore = defineStore('automation', {
  state: () => ({
    /** 保存的场景库（持久化域；顺序=库内顺序） */
    entries: [] as ScenarioLibraryEntry[],
    loaded: false,
    /** 最近一次后端交互错误（面板提示） */
    lastError: '',
    /** 编辑器当前选中的条目 id（null=列表视图） */
    editingId: null as string | null,
    /** 最近一次保存预检的错误（按 step path 定位展示；保存成功即清） */
    validationError: null as ScenarioErrorInfo | null,
    /** runId → 运行视图（后端保留最近 50 份 completed；前端只增不逐出——
     *  面板只展示最近几条，体量可控） */
    runs: {} as Record<string, AutomationRunView>,
    /** 展示顺序（最新在前） */
    runOrder: [] as string[],
    /** runId → 库条目 id（徽标回写用；run 启动时登记） */
    runScenarioIds: {} as Record<string, string>,
    /** 事件订阅在位（attach 幂等守卫） */
    subscribed: false,
  }),

  getters: {
    /** 运行视图列表（最新在前） */
    runList(s): AutomationRunView[] {
      return s.runOrder
        .map((id) => s.runs[id])
        .filter((r): r is AutomationRunView => r !== undefined)
    },
    latestRun(): AutomationRunView | null {
      return this.runList[0] ?? null
    },
    /** 编辑器选中的条目（未选/已删 → null） */
    editing(s): ScenarioLibraryEntry | null {
      return s.entries.find((e) => e.id === s.editingId) ?? null
    },
  },

  actions: {
    // ===================== 库持久化 =====================

    /** 从 localStorage 载入库（missing/invalid → 空库；旧裸数组自动迁移回写信封）。
     *  只在首次调用生效（loaded 守卫）：面板重挂载不重读盘、不冲掉运行期状态。 */
    load() {
      if (this.loaded) return
      const r = loadStored(SCENARIOS_KEY, scenarioLibraryCodec, [] as ScenarioLibraryEntry[])
      this.entries = r.kind === 'ok' || r.kind === 'migrated' ? capLibrary(r.data) : []
      this.loaded = true
    },

    /** 写回信封（cap 后落盘） */
    persist() {
      this.entries = capLibrary(this.entries)
      saveStored(SCENARIOS_KEY, SCENARIO_LIBRARY_SCHEMA, this.entries)
    },

    // ===================== 库条目增删改 =====================

    /** 新建入库（trim 名称；空名拒绝返回 null）；新条目进入编辑态 */
    addScenario(scenario: Scenario): string | null {
      const name = scenario.name.trim()
      if (!name) return null
      const entry: ScenarioLibraryEntry = {
        id: newScenarioId(),
        scenario: { ...cloneScenario(scenario), name },
      }
      this.entries = capLibrary([...this.entries, entry])
      this.persist()
      this.editingId = entry.id
      return entry.id
    },

    /** 改名（scenario.name 是唯一名称来源）；空名/未知 id 拒绝 */
    renameScenario(id: string, name: string): boolean {
      const trimmed = name.trim()
      if (!trimmed) return false
      const entry = this.entries.find((e) => e.id === id)
      if (!entry) return false
      entry.scenario.name = trimmed
      this.persist()
      return true
    },

    /** 复制（深拷贝 + 新 id + 「副本」后缀），插在原条目之后；未知 id 返回 null */
    duplicateScenario(id: string): string | null {
      const i = this.entries.findIndex((e) => e.id === id)
      if (i < 0) return null
      const dup: ScenarioLibraryEntry = {
        id: newScenarioId(),
        scenario: cloneScenario(this.entries[i].scenario),
      }
      dup.scenario.name = t('logic.scen.copySuffix', { name: this.entries[i].scenario.name })
      this.entries.splice(i + 1, 0, dup)
      this.persist()
      return dup.id
    },

    /** 删除条目；编辑中的被删则退出编辑态 */
    removeScenario(id: string): void {
      this.entries = this.entries.filter((e) => e.id !== id)
      if (this.editingId === id) this.editingId = null
      this.persist()
    },

    /**
     * 保存编辑（先走后端 scenarioValidate 预检）：失败把错误落
     * validationError（code/path/message，编辑器按 path 定位步骤行）且条目不动；
     * 后端不可用（浏览器冒烟）退化为结构校验，合法即放行。
     */
    async saveScenario(id: string, scenario: Scenario): Promise<boolean> {
      const name = scenario.name.trim()
      if (!name) {
        this.validationError = { code: 'empty_name', path: 'name', message: t('logic.scen.emptyName') }
        return false
      }
      const cleaned: Scenario = { ...cloneScenario(scenario), name }
      try {
        const summary = await commands.scenarioValidate(cleaned)
        if (!summary.ok) {
          this.validationError = summary.error ?? {
            code: 'invalid_schema',
            path: 'steps',
            message: t('logic.scen.validationFailed'),
          }
          return false
        }
      } catch (e) {
        this.lastError = normalizeIpcError(e)
        try {
          parseScenario(cleaned)
        } catch {
          return false
        }
      }
      const i = this.entries.findIndex((en) => en.id === id)
      if (i < 0) return false
      this.entries[i] = { ...this.entries[i], scenario: cleaned }
      this.persist()
      this.validationError = null
      return true
    },

    /** 清空保存预检错误（编辑器进入时复位，避免展示其他条目的陈旧错误） */
    clearValidation(): void {
      this.validationError = null
    },

    // ===================== 导入 / 导出 =====================

    /**
     * 导入场景 JSON（已 JSON.parse 的值）：支持数组 / 单个对象 /
     * `{ scenarios: [...] }` 三种形状；坏条目逐个跳过；全部无效返回 0 且不动
     * 状态；id 一律重生成防撞；cap 照常执行。
     */
    importScenarios(raw: unknown): number {
      let candidates: unknown[]
      if (Array.isArray(raw)) candidates = raw
      else if (raw !== null && typeof raw === 'object') {
        const envelope = raw as { scenarios?: unknown }
        if (Array.isArray(envelope.scenarios)) candidates = envelope.scenarios
        else candidates = [raw]
      } else candidates = [raw]
      const imported: ScenarioLibraryEntry[] = []
      for (const c of candidates) {
        try {
          imported.push({ id: newScenarioId(), scenario: parseScenario(c) })
        } catch {
          /* 坏条目跳过 */
        }
      }
      if (imported.length === 0) return 0
      this.entries = capLibrary([...this.entries, ...imported])
      this.persist()
      return imported.length
    },

    /** 导出单场景为 pretty JSON（未知 id 返回 null） */
    exportScenario(id: string): string | null {
      const entry = this.entries.find((e) => e.id === id)
      return entry ? JSON.stringify(entry.scenario, null, 2) : null
    },

    /** 导出整库为场景数组 pretty JSON（单文件可直接被 import 消化） */
    exportLibrary(): string {
      return JSON.stringify(
        this.entries.map((e) => e.scenario),
        null,
        2,
      )
    },

    // ===================== 运行生命周期 =====================

    /** 对指定会话启动库内场景：成功登记 running 视图并返回 runId；失败（后端
     *  拒绝：校验不过/会话不可用/同会话并发）落 lastError 返回 null */
    async start(sessionId: string, entryId: string): Promise<string | null> {
      const entry = this.entries.find((e) => e.id === entryId)
      if (!entry) return null
      try {
        const runId = await commands.scenarioStart(sessionId, entry.scenario)
        this.runs[runId] = { runId, sessionId, status: 'running', startedEpochMs: Date.now() }
        this.runOrder.unshift(runId)
        this.runScenarioIds[runId] = entryId
        entry.lastRunStatus = 'running'
        entry.lastRunAt = Date.now()
        return runId
      } catch (e) {
        this.lastError = normalizeIpcError(e)
        return null
      }
    },

    /** 本地取消登记（乐观）+ 条目徽标回写 */
    markRunCancelled(runId: string) {
      const run = this.runs[runId]
      if (!run) return
      run.status = 'cancelled'
      const entry = this.entries.find((e) => e.id === this.runScenarioIds[runId])
      if (entry) {
        entry.lastRunStatus = 'cancelled'
        entry.lastRunAt = Date.now()
      }
    },

    /** 停止运行：仅 running 态响应（未知/已终态 no-op——后端本就幂等）；
     *  本地先置 cancelled，scenario-finished 事件随后以权威视图覆盖 */
    async stop(runId: string): Promise<void> {
      const run = this.runs[runId]
      if (!run || run.status !== 'running') return
      try {
        await commands.scenarioStop(runId)
      } catch (e) {
        this.lastError = normalizeIpcError(e)
      }
      this.markRunCancelled(runId)
    },

    /** scenario-progress reconcile：更新水位与叶步种类；未知/已终态 run 的
     *  迟到事件忽略 */
    onProgress(p: ScenarioProgressPayload): void {
      const run = this.runs[p.runId]
      if (!run || run.status !== 'running') return
      run.progress = { currentStep: p.currentStep, totalSteps: p.totalSteps }
      run.kind = p.kind
    },

    /** scenario-finished reconcile：权威终态覆盖（含 stop 乐观值）；未知 run 忽略；
     *  条目「最近运行」徽标回写终态 */
    onFinished(view: ScenarioRunView): void {
      const prev = this.runs[view.runId]
      if (!prev) return
      this.runs[view.runId] = { ...view, kind: prev.kind }
      const entry = this.entries.find((e) => e.id === this.runScenarioIds[view.runId])
      if (entry) {
        entry.lastRunStatus = view.status
        entry.lastRunAt = Date.now()
      }
    },

    /** 会话状态 reconcile：仅断开/错误时把该会话全部 running 视图置 cancelled
     *  （后端 disconnect 会先取消场景再断开，finished 事件随后覆盖本地值） */
    onSessionStatus(p: StatusPayload): void {
      if (p.status !== 'disconnected' && p.status !== 'error') return
      for (const run of Object.values(this.runs)) {
        if (run.sessionId === p.sessionId && run.status === 'running') {
          this.markRunCancelled(run.runId)
        }
      }
    },

    /** 报告获取（json|junit）；失败落 lastError 返回 null */
    async fetchReport(runId: string, format: ScenarioReportFormat): Promise<string | null> {
      try {
        return await commands.scenarioReport(runId, format)
      } catch (e) {
        this.lastError = normalizeIpcError(e)
        return null
      }
    },

    // ===================== 事件接线（ScenarioPanel 挂载期调用） =====================

    /** 订阅 scenario-progress / scenario-finished / session-status（幂等） */
    async attach(): Promise<void> {
      if (this.subscribed) return
      this.subscribed = true
      unlistens.push(await onScenarioProgress((p) => this.onProgress(p)))
      unlistens.push(await onScenarioFinished((v) => this.onFinished(v)))
      unlistens.push(await onSessionStatus((p) => this.onSessionStatus(p)))
    },

    /** 退订全部（同步调用） */
    detach(): void {
      unlistens.forEach((f) => f())
      unlistens = []
      this.subscribed = false
    },
  },
})
