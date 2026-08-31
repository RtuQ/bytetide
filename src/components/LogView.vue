<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, onScopeDispose, ref, watch } from 'vue'
import { RecycleScroller } from 'vue-virtual-scroller'
import { useThrottleFn } from '@vueuse/core'
import { save } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore } from '../stores/session'
import { HIGHLIGHTER_KEY, buildTestMatcher, hlStyle } from '../composables/useHighlighter'
import { parseAnsi, stripAnsi, type AnsiStyle } from '../composables/useAnsi'
import { useRate, humanizeBytes, humanizeMs } from '../composables/useRate'
import type { LogLine } from '../types'

const props = defineProps<{ sessionId: string }>()
const store = useSessionStore()
const session = computed(() => store.sessions[props.sessionId] ?? null)

const { stats, segmentsFor } = inject(HIGHLIGHTER_KEY)!

// 行渲染分段：先按 ANSI SGR 样式切游程，游程内再叠加搜索/关键词高亮
// （高亮优先：hlStyle 自带前景+背景+加粗；无高亮时用 ANSI 的 fg/bg/bold）
interface RowSeg {
  text: string
  hl: string | null
  ansi: AnsiStyle | null
}
function rowSegments(text: string): RowSeg[] {
  const runs = parseAnsi(text)
  const out: RowSeg[] = []
  for (const run of runs) {
    for (const cs of segmentsFor(run.text)) {
      out.push({ text: cs.text, hl: cs.color, ansi: run.style })
    }
  }
  return out
}
function segStyle(seg: RowSeg) {
  if (seg.hl) return hlStyle(seg.hl)
  const a = seg.ansi
  if (!a) return undefined
  const st: Record<string, string | number> = {}
  if (a.fg) st.color = a.fg
  if (a.bg) st.background = a.bg
  if (a.bold) st.fontWeight = 700
  return Object.keys(st).length ? st : undefined
}
const matchSet = computed(() => new Set(stats.value.matchLines))
// 显示行集：先过过滤链（include/exclude 与“搜索”独立），再叠加“只看命中”
const viewItems = computed(() => {
  const s = session.value
  if (!s) return []
  let arr = s.lines
  for (const f of s.filters) {
    if (!f.enabled || !f.text) continue
    const re = buildTestMatcher({
      pattern: f.text,
      useRegex: f.useRegex,
      caseSensitive: f.caseSensitive,
      wholeWord: f.wholeWord,
    })
    arr = arr.filter((l) => {
      if (f.dir !== 'any' && l.dir !== f.dir) {
        return f.mode === 'exclude'
      }
      const hit = re ? re.test(l.text) : false
      return f.mode === 'include' ? hit : !hit
    })
  }
  if (s.onlyMatches) {
    const ms = matchSet.value
    arr = arr.filter((l) => ms.has(l.no))
  }
  return arr
})

const hexView = computed(() => session.value?.hexView ?? false)
const showDelta = computed(() => session.value?.showDelta ?? false)
const showLineNo = computed(() => session.value?.showLineNo ?? true)
const showDir = computed(() => session.value?.showDir ?? true)

// 日志区最小宽度：等宽字体按最长行字符数×ch 估宽，超出视口时 scroller 横向滚动。
// 宽度经 --log-min-w 撑在库的 item-wrapper 上（其自带 overflow:hidden 会裁掉
// 行级溢出，行本身无法产生滚动范围），全部行共用保证滚动条稳定不跳。
// 固定列预算：252px 覆盖 gutter(12)/no(66)/ts(~70)/dir(30)/Δ(~45)+内边距；随列隐藏扣减；
// HEX 视图每字节 "XX " 约 ×3；封顶防极端长行。
// 行最小宽度与日志区统计一样走 300ms 节流：每批次 O(n) 扫描在持续高吞吐下
// 会累积成吞吐赤字（积压数小时仍在不实时），不能挂在每批都变的 computed 上。
const rowMinWidth = ref('calc(40ch + 252px)')
const recomputeRowMinWidth = useThrottleFn(
  () => {
    let m = 0
    const items = viewItems.value
    for (let i = 0; i < items.length; i++) {
      const t = items[i]!.text
      // 含 ANSI 序列的行按剥离后的显示长度估宽（守卫先行走快速路径，避免全量正则）
      const len = t.indexOf('\x1b') === -1 ? t.length : stripAnsi(t).length
      if (len > m) m = len
    }
    const chars = Math.min(hexView.value ? m * 3 + 2 : m, 20000)
    const fixedPx = 252 - (showLineNo.value ? 0 : 66) - (showDir.value ? 0 : 30)
    rowMinWidth.value = `calc(${Math.max(chars, 40)}ch + ${fixedPx}px)`
  },
  300,
  true,
)
watch([() => session.value?.lineCounter ?? 0, hexView, showLineNo, showDir], recomputeRowMinWidth, {
  immediate: true,
})

// 行选中与书签：点击行选中；工具栏★ 或 Ctrl+F2 / Ctrl+B 切换书签
const selectedNo = ref<number | null>(null)
const bookmarkSet = computed(() => new Set(session.value?.bookmarks ?? []))
// AI 批注行号集合（REST 桥写入，实时同步）
const aiNoteSet = computed(() => new Set(session.value?.aiNotes.map((n) => n.no) ?? []))
function toggleSelectedBookmark() {
  const s = session.value
  if (!s || selectedNo.value == null) return
  store.toggleBookmark(s.id, selectedNo.value)
}
function onKeydown(e: KeyboardEvent) {
  // 分屏时多个 LogView 实例共存，仅活动会话所在实例响应快捷键
  if (props.sessionId !== store.activeId) return
  if (e.ctrlKey && (e.key === 'F2' || e.key === 'b' || e.key === 'B')) {
    e.preventDefault()
    toggleSelectedBookmark()
  }
}
window.addEventListener('keydown', onKeydown)
onScopeDispose(() => window.removeEventListener('keydown', onKeydown))
const totalBytes = computed(() => {
  const s = session.value
  return s ? (s.rxBytes ?? 0) + (s.txBytes ?? 0) : 0
})
const bps = useRate(() => totalBytes.value)

function deltaMs(item: LogLine, index: number): string {
  if (index <= 0) return '-'
  const prev = viewItems.value[index - 1]
  if (!prev) return '-'
  return humanizeMs(item.epochMillis - prev.epochMillis)
}

// HEX 视图：把行文本按 UTF-8 字节转成大写十六进制串（封顶 512 字节，超出显示省略号）
function hexDump(text: string): string {
  const bytes = new TextEncoder().encode(text)
  const n = Math.min(bytes.length, 512)
  let out = ''
  for (let i = 0; i < n; i++) out += bytes[i].toString(16).padStart(2, '0').toUpperCase() + ' '
  return out.trim() + (bytes.length > n ? ' …' : '')
}

// 导出当前会话可见行到用户通过对话框选择的文件
async function exportLog() {
  const s = session.value
  if (!s) return
  const path = await save({
    defaultPath: `session-${props.sessionId}.txt`,
    filters: [{ name: 'Text', extensions: ['txt'] }],
  })
  if (!path) return
  const content =
    s.lines.map((l) => `${l.ts}\t${l.dir === 'rx' ? 'RX' : 'TX'}\t${l.text}`).join('\n') + '\n'
  try {
    await invoke('export_text_cmd', { path, content })
  } catch (e) {
    alert(String(e))
  }
}

// RecycleScroller 实例（库未带类型，按 any 处理）
const scroller = ref<any>(null)
let scrollEl: HTMLElement | null = null

function onScroll() {
  if (!scrollEl || !session.value) return
  const atBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 30
  store.setFollowTail(props.sessionId, atBottom)
}

function bindScroll() {
  const el: HTMLElement | undefined = scroller.value?.$el
  if (el && el !== scrollEl) {
    scrollEl?.removeEventListener('scroll', onScroll)
    el.addEventListener('scroll', onScroll, { passive: true })
    scrollEl = el
  }
}

watch(session, () => {
  nextTick(bindScroll)
})

// 跟随尾部：有新行且开启跟随时滚动到底
watch(
  () => session.value?.lineCounter,
  async () => {
    if (session.value?.followTail) {
      await nextTick()
      scroller.value?.scrollToItem?.(viewItems.value.length - 1)
    }
  },
)

// 从 MatchStats 点击行号跳转
watch(
  () => session.value?.jump,
  async (j) => {
    if (!j) return
    await nextTick()
    const idx = viewItems.value.findIndex((l) => l.no === j.no)
    if (idx >= 0) scroller.value?.scrollToItem?.(idx)
  },
)

onBeforeUnmount(() => {
  scrollEl?.removeEventListener('scroll', onScroll)
})
</script>

<template>
  <div class="logview">
    <div v-if="session" class="logview-bar">
      <div class="bar-group">
        <label class="check">
          <input
            type="checkbox"
            :checked="session.followTail"
            @change="store.setFollowTail(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>跟随</span>
        </label>
        <label class="check">
          <input
            type="checkbox"
            :checked="session.onlyMatches"
            @change="store.setOnlyMatches(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>只看命中</span>
        </label>
        <label class="check">
          <input
            type="checkbox"
            :checked="session.hexView"
            @change="store.setHexView(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>HEX</span>
        </label>
        <label class="check" title="显示相邻行的时间差">
          <input
            type="checkbox"
            :checked="showDelta"
            @change="store.setShowDelta(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>间隔</span>
        </label>
        <label class="check" title="显示/隐藏行号列">
          <input
            type="checkbox"
            :checked="showLineNo"
            @change="store.setShowLineNo(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>行号</span>
        </label>
        <label class="check" title="显示/隐藏收发方向列">
          <input
            type="checkbox"
            :checked="showDir"
            @change="store.setShowDir(props.sessionId, ($event.target as HTMLInputElement).checked)"
          />
          <span class="box">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
          </span>
          <span>RX/TX</span>
        </label>
        <button
          class="btn btn-ghost btn-sm star-btn"
          :class="{ 'bm-on': selectedNo != null && bookmarkSet.has(selectedNo) }"
          :disabled="selectedNo == null"
          :title="selectedNo == null
            ? '书签当前行（先点击选中一行）'
            : bookmarkSet.has(selectedNo)
              ? '取消该书签（Ctrl+F2 / Ctrl+B）'
              : '书签当前行（Ctrl+F2 / Ctrl+B）'"
          aria-label="切换选中行的书签"
          @click="toggleSelectedBookmark"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/></svg>
          <span>书签</span>
        </button>
      </div>

      <div class="bar-spacer"></div>

      <div class="bar-group">
        <button
          v-if="session.kind !== 'offline' && (session.status === 'connected' || session.status === 'connecting')"
          class="btn btn-sm btn-danger"
          title="断开串口（保留标签页与日志）"
          @click="store.stopSession(props.sessionId)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.8 0"/></svg>
          <span>停止</span>
        </button>
        <button
          v-else-if="session.kind !== 'offline'"
          class="btn btn-sm btn-primary"
          title="重新连接该串口"
          @click="store.reconnectSession(props.sessionId)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h0a4 4 0 0 1-4-4V8Z"/></svg>
          <span>重连</span>
        </button>
        <button class="btn btn-ghost btn-sm" title="清屏" @click="store.clearLog(props.sessionId)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21"/><path d="M22 21H7"/><path d="m5 11 9 9"/></svg>
          <span>清屏</span>
        </button>
        <button class="btn btn-ghost btn-sm" title="导出日志" @click="exportLog">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
          <span>导出</span>
        </button>
        <button v-if="session.kind !== 'offline'" class="btn btn-ghost btn-sm" title="打开日志文件路径" @click="store.openLogPath(props.sessionId)">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/></svg>
          <span>日志</span>
        </button>
      </div>

      <span v-if="session.error" class="logview-err">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/></svg>
        {{ session.error }}
      </span>
    </div>

    <RecycleScroller
      v-if="session"
      ref="scroller"
      class="scroller"
      :style="{ '--log-min-w': rowMinWidth }"
      :items="viewItems"
      :item-size="22"
      key-field="no"
      v-slot="{ item, index }"
    >
      <div
        class="log-row"
        :class="[item.dir, { selected: item.no === selectedNo }]"
        @click="selectedNo = item.no"
      >
        <span class="bm-gutter">
          <svg v-if="bookmarkSet.has(item.no)" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m19 21-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v16z"/></svg>
          <svg v-if="aiNoteSet.has(item.no)" class="ai-note-ic" viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l1.9 5.7a2 2 0 0 0 1.4 1.4L21 12l-5.7 1.9a2 2 0 0 0-1.4 1.4L12 21l-1.9-5.7a2 2 0 0 0-1.4-1.4L3 12l5.7-1.9a2 2 0 0 0 1.4-1.4L12 3z"/></svg>
        </span>
        <span v-if="showLineNo" class="col-no">{{ item.no }}</span>
        <span class="col-ts">{{ item.ts }}</span>
        <span v-if="showDelta" class="col-dt">{{ deltaMs(item, index) }}</span>
        <span v-if="showDir" class="col-dir">{{ item.dir === 'rx' ? 'RX' : 'TX' }}</span>
        <span class="col-tx">
          <span v-if="hexView" class="hex">{{ hexDump(item.text) }}</span>
          <template v-else>
            <span
              v-for="(seg, i) in rowSegments(item.text)"
              :key="i"
              :style="segStyle(seg)"
              >{{ seg.text }}</span
            >
          </template>
        </span>
      </div>
    </RecycleScroller>

    <div v-if="session" class="logview-foot">
      <span class="stats" :title="`RX ${session.rxLines ?? 0} 行 / TX ${session.txLines ?? 0} 行`">
        RX {{ humanizeBytes(session.rxBytes) }} · TX {{ humanizeBytes(session.txBytes) }} · {{ humanizeBytes(bps) }}/s
      </span>
      <span
        v-if="session.droppedLines"
        class="drop-note"
        title="前端缓冲上限 50000 行，超出即从最旧行开始丢弃（本会话累计）"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>
        已丢弃 {{ session.droppedLines.toLocaleString() }} 行
      </span>
    </div>

    <div v-else class="logview-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h0a4 4 0 0 1-4-4V8Z"/></svg>
      <span>打开一个串口开始</span>
    </div>
  </div>
</template>
