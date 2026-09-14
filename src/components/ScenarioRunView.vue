<script setup lang="ts">
import { computed, ref } from 'vue'
import { save } from '@tauri-apps/plugin-dialog'
import { commands } from '../ipc/commands'
import { useAutomationStore } from '../stores/automation'
import type { ScenarioReportFormat } from '../types/automation'

/**
 * 单个场景运行视图（Stage 3 Task 4）：进度水位（currentStep/totalSteps + 当前
 * 叶步种类，aria-live 聚焦播报）、运行中停止按钮、终态错误展示与 JSON/JUnit
 * 报告保存（scenario_report_cmd → 另存对话框 → export_text 写盘）。
 * 数据源 = automation store 的 run 视图（runId 定位；未知 runId 渲染空态）。
 */
const props = defineProps<{ runId: string }>()
const automation = useAutomationStore()

const run = computed(() => automation.runs[props.runId] ?? null)

const STATUS_TEXT: Record<string, string> = {
  running: '运行中',
  passed: '通过',
  failed: '失败',
  cancelled: '已取消',
}

/** scenario-progress 的叶步种类 → 中文标签 */
const KIND_TEXT: Record<string, string> = {
  send: '发送',
  signal: '信号',
  delay: '延时',
  wait: '等待',
  assert: '断言',
}

const kindText = computed(() => {
  const k = run.value?.kind
  return k ? (KIND_TEXT[k] ?? k) : ''
})

async function stopRun() {
  await automation.stop(props.runId)
}

const savingReport = ref(false)
async function saveReport(format: ScenarioReportFormat) {
  if (savingReport.value) return
  savingReport.value = true
  try {
    const text = await automation.fetchReport(props.runId, format)
    if (!text) return
    const isXml = format === 'junit'
    const path = await save({
      defaultPath: `${props.runId}.${isXml ? 'xml' : 'json'}`,
      filters: [{ name: isXml ? 'JUnit XML' : 'JSON', extensions: [isXml ? 'xml' : 'json'] }],
    })
    if (!path) return
    await commands.exportText(path, text)
  } catch (e) {
    automation.lastError = `报告保存失败：${String(e instanceof Error ? e.message : e)}`
  } finally {
    savingReport.value = false
  }
}
</script>

<template>
  <div class="sc-runview">
    <div v-if="!run" class="sc-runview-empty panel-hint">无运行记录</div>
    <template v-else>
      <div class="sc-runview-head">
        <span class="sc-status" :class="`st-${run.status}`">{{ STATUS_TEXT[run.status] ?? run.status }}</span>
        <span class="sc-run-meta">{{ run.runId }}</span>
        <span class="sc-run-meta">{{ run.sessionId }}</span>
        <span v-if="run.durationMs !== undefined" class="sc-run-meta">{{ (run.durationMs / 1000).toFixed(1) }}s</span>
      </div>
      <div v-if="run.status === 'running'" class="sc-runview-progress">
        <span class="sc-progress" aria-live="polite">
          {{ run.progress ? `${run.progress.currentStep} / ${run.progress.totalSteps} 步` : '启动中…' }}
        </span>
        <span v-if="kindText" class="sc-kind-now">当前：{{ kindText }}</span>
        <span class="send-spacer"></span>
        <button class="btn btn-danger btn-sm sc-stop" type="button" title="停止该场景运行" @click="stopRun">停止</button>
      </div>
      <div v-if="run.error" class="sc-run-err">{{ run.error }}</div>
      <div v-if="run.status !== 'running'" class="sc-runview-reports">
        <button class="btn btn-sm sc-report-json" type="button" title="获取 JSON 报告并另存为文件" @click="saveReport('json')">保存 JSON 报告</button>
        <button class="btn btn-sm sc-report-junit" type="button" title="获取 JUnit XML 报告并另存为文件" @click="saveReport('junit')">保存 JUnit 报告</button>
      </div>
    </template>
  </div>
</template>

<style scoped>
.sc-runview {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
}
.sc-runview-head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.sc-status {
  font-weight: 600;
}
.sc-status.st-running {
  color: var(--warn);
}
.sc-status.st-passed {
  color: var(--ok);
}
.sc-status.st-failed {
  color: var(--err);
}
.sc-status.st-cancelled {
  color: var(--text-dim);
}
.sc-run-meta {
  color: var(--text-dim);
  font-family: var(--font-mono);
  font-size: 11px;
}
.sc-runview-progress {
  display: flex;
  align-items: center;
  gap: 8px;
}
.sc-progress {
  color: var(--text-muted);
  font-family: var(--font-mono);
}
.sc-kind-now {
  color: var(--text-muted);
}
.sc-run-err {
  color: var(--err);
  word-break: break-all;
}
.sc-runview-reports {
  display: flex;
  gap: 6px;
}
</style>
