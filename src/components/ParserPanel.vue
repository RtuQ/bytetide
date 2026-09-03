<script setup lang="ts">
import { computed, ref } from 'vue'
import { useParserEngine } from '../composables/useParserEngine'
import { BUILTIN_EXAMPLE_SRC } from '../parser/builtinExample'

/** 协议解析面板（plan-parser-v1 §3）：脚本导入/拖拽/内置示例 + 脚本卡片 +
 *  启用开关 + 三格统计 + 查看器弹层。面板开合状态由 App.vue 持久化。 */
defineProps<{ open: boolean }>()
const emit = defineEmits<{ toggle: [Event] }>()

const { ui, importScript, reloadScript, unloadScript, setEnabled } = useParserEngine()

const fileInput = ref<HTMLInputElement | null>(null)
const dragOver = ref(false)
const viewerOpen = ref(false)

function pickFile() {
  fileInput.value?.click()
}

/** 读入 .js 文本并导入；非 .js 静默忽略（拖拽误拖其它文件常见） */
function importJsFile(file: File) {
  if (!/\.js$/i.test(file.name)) return
  const rd = new FileReader()
  rd.onload = () => void importScript(String(rd.result))
  rd.readAsText(file)
}

function onFileChange(e: Event) {
  const input = e.target as HTMLInputElement
  const f = input.files?.[0]
  if (f) importJsFile(f)
  input.value = ''
}

function onDrop(e: DragEvent) {
  dragOver.value = false
  const f = e.dataTransfer?.files?.[0]
  if (f) importJsFile(f)
}

function onToggleEnable(e: Event) {
  void setEnabled((e.target as HTMLInputElement).checked)
}

function openViewer() {
  viewerOpen.value = true
}

/** 脚本层标记：有 parse = Worker 沙箱脚本层；否则声明式字段表 */
const layerText = computed(() =>
  ui.hasParse ? '脚本层 · Worker 沙箱' : `声明式 · ${ui.fieldsCount ?? 0} 字段`,
)

/** 解析成功率：frames=0 显示 '--' */
const rateText = computed(() =>
  ui.stats.frames > 0 ? `${Math.round((ui.stats.ok / ui.stats.frames) * 100)}%` : '--',
)

/** 横幅配色：suspect=警告琥珀，tripped/timeout/error=错误红 */
const bannerTone = computed(() => (ui.banner?.kind === 'suspect' ? 'warn' : 'err'))

/** 试运行疑似但未自动启用时的兜底提示（引擎无更具体 banner 时） */
const showSuspectHint = computed(
  () => !ui.banner && ui.trialReport?.verdict === 'suspect' && !ui.enabled,
)

const VERDICT_TEXT = { ok: '通过', suspect: '疑似', 'no-data': '无数据' } as const

/** 试运行抽样（最多展示 5 条） */
const drySamples = computed(() => ui.trialReport?.samples.slice(0, 5) ?? [])
</script>

<template>
  <details class="panel" :open="open" @toggle="emit('toggle', $event)">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m8 7-5 5 5 5m8-10 5 5-5 5M14 4l-4 16" /></svg>
      <span class="panel-title">协议解析</span>
      <span v-if="ui.stats.frames > 0" class="badge">{{ ui.stats.frames }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6" /></svg>
    </summary>

    <div
      class="panel-body parser-body"
      :class="{ drag: dragOver }"
      @dragover.prevent="dragOver = true"
      @dragenter.prevent="dragOver = true"
      @dragleave.prevent="dragOver = false"
      @drop.prevent="onDrop"
    >
      <!-- 横幅区：引擎横幅（suspect=琥珀 / 其余=红） -->
      <div v-if="ui.banner" class="parser-banner" :class="bannerTone">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z" /><path d="M12 9v4" /><path d="M12 17h.01" /></svg>
        <span>{{ ui.banner.msg }}</span>
      </div>
      <div v-else-if="showSuspectHint" class="parser-banner warn">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z" /><path d="M12 9v4" /><path d="M12 17h.01" /></svg>
        <span>framing 疑似配置错误：试运行 0 完整帧或 CRC 全败，未自动启用（可手动打开开关）</span>
      </div>

      <!-- 空态：尚未加载脚本 -->
      <div v-if="!ui.loaded" class="parser-empty">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1" /><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1" /></svg>
        <div class="parser-empty-title">还没有加载解析脚本</div>
        <div class="parser-empty-desc">
          导入 bytetide.parser v1 格式的 .js 脚本<br />
          RX 帧将实时翻译为自然语言
        </div>
        <button class="btn btn-primary btn-sm" @click="pickFile">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" /><polyline points="17 8 12 3 7 8" /><line x1="12" y1="3" x2="12" y2="15" /></svg>
          <span>选择脚本文件…</span>
        </button>
        <button class="parser-link" @click="importScript(BUILTIN_EXAMPLE_SRC)">
          没有脚本？加载内置示例体验
        </button>
        <span class="parser-hint">支持拖拽 .js 到此面板 · 试运行通过后自动启用</span>
      </div>

      <!-- 已载：脚本卡片 + 三格统计 -->
      <template v-else>
        <div class="parser-card">
          <div class="parser-card-head">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /><path d="M10 13H8" /><path d="M16 13h-2" /><path d="M10 17h6" /></svg>
            <span class="parser-name" :title="ui.script?.name">{{ ui.script?.name }}</span>
            <span class="parser-ver">v{{ ui.script?.version }}</span>
            <span class="parser-spacer"></span>
            <button class="btn btn-ghost btn-sm btn-icon" title="查看脚本" aria-label="查看脚本" @click="openViewer">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z" /><circle cx="12" cy="12" r="3" /></svg>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon" title="重新加载脚本" aria-label="重新加载脚本" @click="reloadScript()">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.6-6.3" /><path d="M21 3v6h-6" /></svg>
            </button>
            <button class="btn btn-ghost btn-sm btn-icon" title="卸载脚本" aria-label="卸载脚本" @click="unloadScript()">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18" /><path d="M8 6V4h8v2" /><path d="m19 6-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" /></svg>
            </button>
          </div>
          <div class="parser-meta">
            <code>{{ ui.framingSummary }}</code>
          </div>
          <div class="parser-meta">{{ layerText }}</div>
          <div class="parser-switch-row">
            <label class="switch">
              <input type="checkbox" :checked="ui.enabled" @change="onToggleEnable" />
              <span class="track"></span>
              <span class="thumb"></span>
            </label>
            <span>{{ ui.enabled ? '启用解析' : '已停用' }}</span>
          </div>
        </div>

        <div class="parser-stats">
          <div class="parser-stat">
            <b>{{ ui.stats.frames }}</b>
            <span>总帧数</span>
          </div>
          <div class="parser-stat">
            <b>{{ rateText }}</b>
            <span>解析成功率</span>
          </div>
          <div class="parser-stat">
            <b>{{ ui.stats.types }}</b>
            <span>消息类型数</span>
          </div>
        </div>
      </template>

      <input ref="fileInput" type="file" accept=".js,text/javascript" hidden @change="onFileChange" />
    </div>

    <!-- 查看器弹层：只读脚本预览 + 试运行报告 -->
    <div v-if="viewerOpen" class="parser-mask" @click.self="viewerOpen = false">
      <div class="parser-modal" role="dialog" aria-label="解析脚本">
        <div class="parser-modal-head">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /><path d="M10 13H8" /><path d="M16 13h-2" /><path d="M10 17h6" /></svg>
          <span>解析脚本 · {{ ui.script?.name }} v{{ ui.script?.version }}</span>
          <span class="parser-spacer"></span>
          <button class="btn btn-ghost btn-sm btn-icon" title="关闭" aria-label="关闭" @click="viewerOpen = false">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg>
          </button>
        </div>
        <div class="parser-modal-body">
          <div class="parser-pane-label">脚本内容（bytetide.parser v1 · 只读预览）</div>
          <pre class="parser-code">{{ ui.source }}</pre>
          <div v-if="ui.trialReport" class="parser-dry">
            <div class="parser-dry-head">
              <span class="parser-verdict" :class="ui.trialReport.verdict">
                {{ VERDICT_TEXT[ui.trialReport.verdict] }}
              </span>
              <span class="parser-dry-sum">
                {{ ui.trialReport.lines }} 行 → {{ ui.trialReport.frames }} 帧 · CRC 失败
                {{ ui.trialReport.crcFailed }} · 解析错误 {{ ui.trialReport.parseErrors }}
              </span>
            </div>
            <div v-for="(s, i) in drySamples" :key="i" class="parser-dry-row">
              <span class="parser-dry-hex" :title="s.hex">{{ s.hex }}</span>
              <span class="parser-dry-arr">→</span>
              <span class="parser-dry-text">{{ s.text }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </details>
</template>
