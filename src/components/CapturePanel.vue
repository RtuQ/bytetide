<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { openPath } from '@tauri-apps/plugin-opener'
import { useSessionStore } from '../stores/session'
import type { CaptureRule } from '../types'

const store = useSessionStore()
const active = computed(() => store.active)
const capDir = ref('')
onMounted(async () => {
  void store.loadCaptures()
  try {
    capDir.value = await invoke<string>('captures_dir_cmd')
  } catch {
    /* 浏览器冒烟无后端 */
  }
})

const editable = computed(() => !!active.value && active.value.kind === 'live')
const disabledHint = computed(() =>
  active.value ? '现场捕获仅实时会话可用（离线会话无数据流入）' : '无活动会话',
)

let ruleSeq = 0
function addRule() {
  if (!active.value) return
  ruleSeq += 1
  const rule: CaptureRule = {
    id: `c${Date.now().toString(36)}${ruleSeq}`,
    pattern: '',
    useRegex: false,
    caseSensitive: false,
    wholeWord: false,
    enabled: true,
  }
  store.updateCapture(active.value.id, { rules: [...active.value.capture.rules, rule] })
}
function updRule(rid: string, patch: Partial<CaptureRule>) {
  if (!active.value) return
  store.updateCapture(active.value.id, {
    rules: active.value.capture.rules.map((r) => (r.id === rid ? { ...r, ...patch } : r)),
  })
}
function delRule(rid: string) {
  if (!active.value) return
  store.updateCapture(active.value.id, {
    rules: active.value.capture.rules.filter((r) => r.id !== rid),
  })
}

// 前/后窗口以秒编辑（内部 ms，后端钳制 1s–30min）
const preSec = ref(120)
const postSec = ref(60)
watch(
  () => [active.value?.id, active.value?.capture.preMs, active.value?.capture.postMs] as const,
  () => {
    if (!active.value) return
    preSec.value = Math.round(active.value.capture.preMs / 1000)
    postSec.value = Math.round(active.value.capture.postMs / 1000)
  },
  { immediate: true },
)
const clampSec = (v: number) => Math.min(Math.max(v || 10, 10), 1800)
function setPre() {
  if (active.value) store.updateCapture(active.value.id, { preMs: clampSec(preSec.value) * 1000 })
}
function setPost() {
  if (active.value) store.updateCapture(active.value.id, { postMs: clampSec(postSec.value) * 1000 })
}

function openCapture(path: string) {
  void store.loadOfflineSession(path)
}
function fmtSize(n: number): string {
  return n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`
}
function fmtTime(ms: number): string {
  const d = new Date(ms)
  const p = (x: number) => String(x).padStart(2, '0')
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`
}
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12h3l2-7 4 14 3-9 2 4h6"/></svg>
      <span class="panel-title">现场捕获</span>
      <span v-if="store.captures.length" class="badge">{{ store.captures.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>

    <div class="panel-body cap-body">
      <template v-if="editable && active">
        <div class="cap-line">
          <label class="check">
            <input
              type="checkbox"
              :checked="active.capture.enabled"
              @change="store.updateCapture(active.id, { enabled: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>启用捕获</span>
          </label>
          <span class="send-spacer"></span>
          <label class="check" title="断开/读错误时也回溯抓一段（设备重启/掉线现场）">
            <input
              type="checkbox"
              :checked="active.capture.onDisconnect"
              @change="store.updateCapture(active.id, { onDisconnect: ($event.target as HTMLInputElement).checked })"
            />
            <span class="box">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
            </span>
            <span>断连也抓</span>
          </label>
        </div>

        <div class="cap-line">
          <span class="cap-lbl">前置</span>
          <input class="input input-mono cap-sec" type="number" v-model.number="preSec" min="10" max="1800" step="10" aria-label="前置窗口秒数" @change="setPre" />
          <span class="cap-unit">s</span>
          <span class="send-spacer"></span>
          <span class="cap-lbl">后续</span>
          <input class="input input-mono cap-sec" type="number" v-model.number="postSec" min="10" max="1800" step="10" aria-label="后续窗口秒数" @change="setPost" />
          <span class="cap-unit">s</span>
        </div>
        <div class="cap-hint">命中即把前 N 秒 + 后 M 秒转储为档案（告警命中自动捕获）</div>

        <div class="cap-sub">触发规则</div>
        <div v-for="r in active.capture.rules" :key="r.id" class="cap-rule">
          <input
            class="input input-mono cap-pattern"
            :value="r.pattern"
            placeholder="关键词或正则"
            @change="updRule(r.id, { pattern: ($event.target as HTMLInputElement).value })"
          />
          <select
            class="select cap-mode"
            :value="r.useRegex ? 're' : 'text'"
            aria-label="匹配方式"
            @change="updRule(r.id, { useRegex: ($event.target as HTMLSelectElement).value === 're' })"
          >
            <option value="text">文本</option>
            <option value="re">正则</option>
          </select>
          <button class="btn btn-ghost btn-sm" :title="`删除规则 ${r.pattern || r.id}`" :aria-label="`删除规则 ${r.pattern || r.id}`" @click="delRule(r.id)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
          </button>
        </div>
        <button class="btn btn-ghost btn-sm cap-add" @click="addRule">＋添加规则</button>
      </template>
      <div v-else class="cap-hint">{{ disabledHint }}</div>

      <div class="cap-sub">现场档案</div>
      <div v-if="store.captures.length" class="cap-list">
        <div v-for="f in store.captures" :key="f.path" class="cap-item" :title="f.path">
          <span class="cap-fi">
            <span class="cap-fname">{{ f.fileName }}</span>
            <span class="cap-fmeta">{{ fmtTime(f.modifiedMs) }} · {{ fmtSize(f.size) }}</span>
          </span>
          <span class="cap-acts">
            <button class="btn btn-sm" title="作为离线会话打开" @click="openCapture(f.path)">打开</button>
            <button class="btn btn-ghost btn-sm" title="删除档案文件" :aria-label="`删除 ${f.fileName}`" @click="store.deleteCapture(f.path)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>
            </button>
          </span>
        </div>
      </div>
      <div v-else class="cap-hint">暂无档案——命中触发条件后自动生成</div>
      <div class="cap-line">
        <button class="btn btn-sm" title="重新扫描档案目录" @click="store.loadCaptures()">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>
          <span>刷新</span>
        </button>
        <button class="btn btn-sm" title="在文件管理器中打开档案目录" @click="capDir && openPath(capDir).catch(() => {})">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>
          <span>打开目录</span>
        </button>
      </div>
    </div>
  </details>
</template>
