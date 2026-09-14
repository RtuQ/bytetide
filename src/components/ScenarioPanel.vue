<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { open, save } from '@tauri-apps/plugin-dialog'
import { commands } from '../ipc/commands'
import { useSessionStore } from '../stores/session'
import { useAutomationStore } from '../stores/automation'
import { countScenarioLeaves, makeScenario, type ScenarioRunStatus } from '../types/automation'
import ScenarioEditor from './ScenarioEditor.vue'
import ScenarioRunView from './ScenarioRunView.vue'

/**
 * 场景面板（侧栏「规则」组，details.panel 默认收起）：场景库列表（名称 + 静态
 * 叶步数 + 最近运行状态徽标）与「新建 / 导入 / 导出」；条目点击进入编辑器
 * （ScenarioEditor 整区替换）；每项带运行快捷按钮——目标会话从下拉选择
 * （仅 live + connected 会话，来自 session store 只读消费）。
 * 运行区（最近若干 run 的 ScenarioRunView）常驻列表下方。
 * 挂载期 load() 库 + attach() 订阅 scenario-progress/finished/session-status，
 * 卸载 detach()（进度 reconcile 与断开清理在 store）。
 */
const sessionStore = useSessionStore()
const automation = useAutomationStore()

onMounted(async () => {
  automation.load()
  await automation.attach()
})
onBeforeUnmount(() => automation.detach())

/** 可运行目标：live 且已连接的会话（离线/回放/未连接后端都会拒绝跑场景） */
const runTargets = computed(() =>
  sessionStore.sessionList.filter((s) => s.kind === 'live' && s.status === 'connected'),
)
const targetId = ref('')
watch(
  runTargets,
  (list) => {
    if (!list.some((s) => s.id === targetId.value)) targetId.value = list[0]?.id ?? ''
  },
  { immediate: true },
)

function targetLabel(id: string): string {
  return sessionStore.sessions[id]?.config.name ?? id
}

const editing = computed(() => automation.editing)

const RUN_BADGE: Record<ScenarioRunStatus, { text: string; cls: string }> = {
  running: { text: '运行中', cls: 'sc-st-running' },
  passed: { text: '通过', cls: 'sc-st-passed' },
  failed: { text: '失败', cls: 'sc-st-failed' },
  cancelled: { text: '已取消', cls: 'sc-st-cancelled' },
}

const hasRunning = computed(() => automation.runList.some((r) => r.status === 'running'))

/** 运行区展示：全部运行中 + 最近一个已完成（封顶 4 条，防无限增长） */
const shownRunIds = computed(() => {
  const running = automation.runList.filter((r) => r.status === 'running').map((r) => r.runId)
  const finished = automation.runList.filter((r) => r.status !== 'running').map((r) => r.runId)
  return [...running, ...finished.slice(0, 1)].slice(0, 4)
})

function newScenario(): void {
  automation.addScenario(makeScenario(`新场景 ${automation.entries.length + 1}`))
}

function runEntry(id: string): void {
  if (!targetId.value) return
  void automation.start(targetId.value, id)
}

const ioMsg = ref('')

async function importFile(): Promise<void> {
  const path = await open({
    multiple: false,
    filters: [{ name: 'JSON', extensions: ['json'] }],
  })
  if (!path || typeof path !== 'string') return
  try {
    const raw: unknown = JSON.parse(await commands.readTextFile(path))
    const n = automation.importScenarios(raw)
    ioMsg.value = n > 0 ? `已导入 ${n} 个场景` : '未发现可导入的场景（形状不符）'
  } catch (e) {
    ioMsg.value = `导入失败：${e instanceof Error ? e.message : String(e)}`
  }
}

async function exportFile(id: string | null): Promise<void> {
  const content = id ? automation.exportScenario(id) : automation.exportLibrary()
  if (!content) return
  const path = await save({
    defaultPath: id ? 'scenario.json' : 'scenarios.json',
    filters: [{ name: 'JSON', extensions: ['json'] }],
  })
  if (!path) return
  try {
    await commands.exportText(path, content)
    ioMsg.value = ''
  } catch (e) {
    ioMsg.value = `导出失败：${e instanceof Error ? e.message : String(e)}`
  }
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 12h-4l-3 9L9 3l-3 9H2"/></svg>
      <span class="panel-title">场景</span>
      <span v-if="automation.entries.length" class="badge">{{ automation.entries.length }}</span>
      <span v-if="hasRunning" class="badge badge-hot" title="有场景运行中"><span class="dot-ic"></span></span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>

    <div class="panel-body sc-body">
      <ScenarioEditor
        v-if="editing"
        :key="editing.id"
        :entry-id="editing.id"
        @close="automation.editingId = null"
      />
      <template v-else>
        <div class="sc-toolbar">
          <select
            v-if="runTargets.length"
            v-model="targetId"
            class="select sc-target"
            aria-label="目标会话"
            title="选择运行场景的目标会话（仅实时已连接会话）"
          >
            <option v-for="s in runTargets" :key="s.id" :value="s.id">{{ s.config.name }}</option>
          </select>
          <span v-else class="sc-no-target panel-hint">无已连接会话</span>
          <span class="send-spacer"></span>
          <button class="btn btn-sm sc-new" type="button" title="新建场景" @click="newScenario">新建</button>
          <button class="btn btn-ghost btn-sm sc-import" type="button" title="从 JSON 文件导入场景（数组或单个）" @click="importFile">导入</button>
          <button
            v-if="automation.entries.length"
            class="btn btn-ghost btn-sm sc-export-all"
            type="button"
            title="导出全部场景为 JSON 文件"
            @click="exportFile(null)"
          >导出</button>
        </div>
        <div v-if="ioMsg" class="sc-msg">{{ ioMsg }}</div>

        <div v-if="automation.entries.length" class="sc-list">
          <div v-for="e in automation.entries" :key="e.id" class="sc-item">
            <button
              class="sc-name"
              type="button"
              :title="`编辑 ${e.scenario.name}`"
              @click="automation.editingId = e.id"
            >{{ e.scenario.name }}</button>
            <span class="sc-meta">{{ countScenarioLeaves(e.scenario.steps) }} 步</span>
            <span
              v-if="e.lastRunStatus"
              class="badge sc-runbadge"
              :class="RUN_BADGE[e.lastRunStatus]?.cls"
            >{{ RUN_BADGE[e.lastRunStatus]?.text }}</span>
            <span class="send-spacer"></span>
            <button
              class="btn btn-sm sc-run"
              type="button"
              :disabled="!targetId"
              :title="targetId ? `运行到会话 ${targetLabel(targetId)}` : '无已连接会话可运行'"
              @click="runEntry(e.id)"
            >运行</button>
            <button
              class="btn btn-ghost btn-sm sc-dup"
              type="button"
              :title="`复制场景 ${e.scenario.name}`"
              :aria-label="`复制场景 ${e.scenario.name}`"
              @click="automation.duplicateScenario(e.id)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>
            </button>
            <button
              class="btn btn-ghost btn-sm sc-export"
              type="button"
              :title="`导出 ${e.scenario.name} 为 JSON`"
              :aria-label="`导出场景 ${e.scenario.name} JSON`"
              @click="exportFile(e.id)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/></svg>
            </button>
            <button
              class="btn btn-ghost btn-sm sc-del"
              type="button"
              :title="`删除场景 ${e.scenario.name}`"
              :aria-label="`删除场景 ${e.scenario.name}`"
              @click="automation.removeScenario(e.id)"
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
            </button>
          </div>
        </div>
        <div v-else class="panel-empty">暂无场景——点「新建」创建，或「导入」场景 JSON 文件</div>

        <template v-if="shownRunIds.length">
          <div class="sc-sub">最近运行</div>
          <div class="sc-runs">
            <ScenarioRunView v-for="rid in shownRunIds" :key="rid" :run-id="rid" />
          </div>
        </template>
      </template>
    </div>
  </details>
</template>

<style scoped>
.sc-body {
  display: flex;
  flex-direction: column;
  gap: 6px;
  max-height: 520px;
  overflow-y: auto;
}
.sc-toolbar {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-wrap: wrap;
}
.sc-target {
  max-width: 150px;
}
.sc-no-target {
  white-space: nowrap;
}
.sc-msg {
  color: var(--text-muted);
  font-size: 11px;
}
.sc-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.sc-item {
  display: flex;
  align-items: center;
  gap: 4px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: 3px 6px;
}
.sc-name {
  background: none;
  border: none;
  padding: 0;
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
  text-align: left;
  max-width: 140px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sc-name:hover {
  color: var(--accent);
}
.sc-meta {
  color: var(--text-dim);
  font-size: 11px;
  white-space: nowrap;
}
.sc-runbadge.sc-st-running {
  color: var(--warn);
  border-color: var(--warn);
}
.sc-runbadge.sc-st-passed {
  color: var(--ok);
  border-color: var(--ok);
}
.sc-runbadge.sc-st-failed {
  color: var(--err);
  border-color: var(--err);
}
.sc-runbadge.sc-st-cancelled {
  color: var(--text-dim);
}
.sc-sub {
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.5px;
  color: var(--text-dim);
  text-transform: uppercase;
}
.sc-runs {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
</style>
