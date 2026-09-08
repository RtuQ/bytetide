<script setup lang="ts">
import { computed, inject, nextTick, onBeforeUnmount, onScopeDispose, ref, watch } from 'vue'
import LogScroller from './LogScroller.vue'
import { useThrottleFn } from '@vueuse/core'
import { save } from '@tauri-apps/plugin-dialog'
import { invoke } from '@tauri-apps/api/core'
import { useSessionStore, type Session } from '../stores/session'
import { HIGHLIGHTER_KEY, buildTestMatcher, hlStyle } from '../composables/useHighlighter'
import { parseAnsi, stripAnsi, type AnsiStyle } from '../composables/useAnsi'
import { anchoredTop } from '../composables/useScrollAnchor'
import { lineHexDump, lineHexLen } from '../composables/useHexDump'
import { lineBytes } from '../parser/lineBytes'
import { useRate, humanizeBytes, humanizeMs } from '../composables/useRate'
import { requestBackfill } from '../composables/useTauriEvents'
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

// ---------- 行右键菜单：重发此帧 / 复制 ----------
const ctx = ref<{ x: number; y: number; line: LogLine } | null>(null)
const canSendCtx = computed(() => {
  const s = session.value
  return !!s && s.kind === 'live' && s.status === 'connected'
})
function openCtx(e: MouseEvent, line: LogLine) {
  ctx.value = {
    x: Math.min(e.clientX, window.innerWidth - 170),
    y: Math.min(e.clientY, window.innerHeight - 120),
    line,
  }
}
function closeCtx() {
  ctx.value = null
}
const onCtxKey = (e: KeyboardEvent) => {
  if (e.key === 'Escape') closeCtx()
}
window.addEventListener('keydown', onCtxKey)
onBeforeUnmount(() => window.removeEventListener('keydown', onCtxKey))
function ctxLineHex(line: LogLine): string {
  const s = session.value
  const bytes = lineBytes(line, s?.plot.source === 'ascii-hex' ? 'ascii-hex' : 'binary')
  return [...bytes].map((b) => b.toString(16).toUpperCase().padStart(2, '0')).join(' ')
}
function resendLine() {
  const c = ctx.value
  if (!c || !canSendCtx.value) return
  store.send(props.sessionId, ctxLineHex(c.line), 'hex').catch((e: unknown) => alert(String(e)))
  closeCtx()
}
async function copyCtx(kind: 'text' | 'hex') {
  const c = ctx.value
  if (!c) return
  const t = kind === 'text' ? c.line.text : ctxLineHex(c.line)
  try {
    await navigator.clipboard.writeText(t)
  } catch {
    /* 剪贴板不可用时静默（无感失败好过报错打断） */
  }
  closeCtx()
}
// 过滤链（include/exclude 与“搜索”独立，再叠加“只看命中”）作用于任意行集：
// viewItems 与回补行的渲染计数共用，保证 scrollTop 补偿口径与实际渲染一致
function filterLines(s: Session, lines: LogLine[]): LogLine[] {
  let arr = lines
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
}
// 显示行集：头部裁剪/回补都会改变行集几何，scrollTop 补偿统一走跟随 watcher
const viewItems = computed(() => {
  const s = session.value
  if (!s) return []
  return filterLines(s, s.lines)
})

const hexView = computed(() => session.value?.hexView ?? false)
const showDelta = computed(() => session.value?.showDelta ?? false)
const showLineNo = computed(() => session.value?.showLineNo ?? true)
const showDir = computed(() => session.value?.showDir ?? true)

// 日志区最小宽度：等宽字体按最长行字符数×ch 估宽，超出视口时 scroller 横向滚动。
// 宽度经 --log-min-w 撑在 LogScroller 的 ls-sizer 上产生横向滚动范围（行级
// overflow:hidden 只裁自身溢出），全部行共用保证滚动条稳定不跳。
// 估算口径与实际渲染对齐（styles.css 列宽）：ts 与 text 同为等宽字体，
// 一并按字符数计入（自定义长时间戳模板自动跟随）；像素列只含
// gutter(12) + padding(20) + no(66) + dir(30)，Δ 列(66) 仅打开时计入；
// HEX 视图每字节 "XX " 约 ×3；封顶防极端长行。
// 行最小宽度与日志区统计一样走 300ms 节流：每批次 O(n) 扫描在持续高吞吐下
// 会累积成吞吐赤字（积压数小时仍在不实时），不能挂在每批都变的 computed 上。
const rowMinWidth = ref('calc(40ch + 128px)')
const recomputeRowMinWidth = useThrottleFn(
  () => {
    // 每行占宽 = (ts 字符 + text 字符(+hex 放大)) × ch + 像素列
    let m = 0
    const items = viewItems.value
    for (let i = 0; i < items.length; i++) {
      const it = items[i]!
      const t = it.text
      // 含 ANSI 序列的行按剥离后的显示长度估宽（守卫先行走快速路径，避免全量正则）
      const disp = t.indexOf('\x1b') === -1 ? t.length : stripAnsi(t).length
      // HEX 视图按实际渲染字节数估宽：有原始字节用字节长（lossy 文本的 U+FFFD 长度不准）
      const len = it.ts.length + (hexView.value ? lineHexLen(t, it.bytes) * 3 + 2 : disp)
      if (len > m) m = len
    }
    const chars = Math.min(m, 20000)
    const fixedPx = 128 - (showLineNo.value ? 0 : 66) - (showDir.value ? 0 : 30) + (showDelta.value ? 66 : 0)
    rowMinWidth.value = `calc(${Math.max(chars, 40)}ch + ${fixedPx}px)`
  },
  300,
  true,
)
watch([() => session.value?.lineCounter ?? 0, () => session.value?.backfillTotal ?? 0, hexView, showLineNo, showDir, showDelta], recomputeRowMinWidth, {
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

// 落盘录制/分段仅对读线程存活的会话可用（已连接或连接中），未连接时命令通道已关
const recLive = computed(() => {
  const st = session.value?.status
  return st === 'connected' || st === 'connecting'
})

function deltaMs(item: LogLine, index: number): string {
  if (index <= 0) return '-'
  const prev = viewItems.value[index - 1]
  if (!prev) return '-'
  return humanizeMs(item.epochMillis - prev.epochMillis)
}

// HEX 视图行渲染已抽为纯函数 lineHexDump（composables/useHexDump.ts）：
// 有原始字节（后端对非法 UTF-8 行随行附带）优先用字节，纯文本行才按 UTF-8 编码 text。

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

// LogScroller 实例（泛型 SFC 不能用 InstanceType，直接声明 expose 的公开面）
const scroller = ref<{ el: HTMLElement | null; scrollToItem: (index: number) => void } | null>(
  null,
)
let scrollEl: HTMLElement | null = null

function onScroll() {
  if (!scrollEl || !session.value) return
  const atBottom = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 30
  store.setFollowTail(props.sessionId, atBottom)
  // 上滑近顶（方案 B）：向前翻页回补仍在 ring 里的被裁旧行；
  // 仅未跟随尾部时触发（跟随时无回看语义），requestBackfill 内部有在途守卫
  if (!atBottom && scrollEl.scrollTop < 40 && !session.value.backfillExhausted) {
    void requestBackfill(props.sessionId)
  }
}

function bindScroll() {
  const el: HTMLElement | null | undefined = scroller.value?.el
  if (el && el !== scrollEl) {
    scrollEl?.removeEventListener('scroll', onScroll)
    el.addEventListener('scroll', onScroll, { passive: true })
    scrollEl = el
  }
}

watch(session, () => {
  nextTick(bindScroll)
})

// 跟随尾部：有新行且开启跟随时滚动到底。
// 取消跟随时做视口锚定（plan-buffer-logging-v1 §2.3）：头部行被滑动窗口裁剪
// 会让内容高度收缩、浏览器钳制 scrollTop，视口整体上移——watcher 默认 pre-flush，
// 此刻 DOM 还是旧几何，先记 oldTop；nextTick 后按被裁行数等量回补（双向钳制）。
// takeEvicted 取走即清零，防同 tick 多批次漏计。
// 方案 B 同一 watcher 统一补偿：上滑回补把行插到头部，内容高度增长会把视口
// 内容相对下推 N 行，按回补行数（仅计通过过滤链、真正渲染占高的）等量下移抵消。
const ROW_HEIGHT = 22 // 与模板 LogScroller 的 item-size 联动；改行高须同步 .log-row height（AGENTS 红线）
watch(
  // backfillTotal 入列：补行不推进 lineCounter，需独立触发源
  [() => session.value?.lineCounter, () => session.value?.backfillTotal],
  async () => {
    const s = session.value
    if (!s) return
    const oldTop = scrollEl?.scrollTop ?? 0 // pre-flush：DOM 还是旧几何
    const evicted = store.takeEvicted(props.sessionId)
    const backfilled = store.takeBackfilled(props.sessionId)
    if (s.followTail) {
      await nextTick()
      scroller.value?.scrollToItem(viewItems.value.length - 1)
    } else if (evicted > 0 || backfilled.length > 0) {
      // 补行中只有通过过滤链的才真正渲染占高，补偿按渲染行数计
      const rendered = backfilled.length > 0 ? filterLines(s, backfilled).length : 0
      await nextTick()
      if (scrollEl) {
        scrollEl.scrollTop = anchoredTop(oldTop, evicted, rendered, ROW_HEIGHT, scrollEl.scrollHeight, scrollEl.clientHeight)
      }
    }
  },
)

// 从 MatchStats / 解码列表点击行号跳转：滚动定位并闪烁目标行 1.2s
const flashNo = ref<number | null>(null)
watch(
  () => session.value?.jump,
  async (j) => {
    if (!j) return
    await nextTick()
    const idx = viewItems.value.findIndex((l) => l.no === j.no)
    if (idx >= 0) {
      scroller.value?.scrollToItem(idx)
      flashNo.value = j.no
      setTimeout(() => {
        if (flashNo.value === j.no) flashNo.value = null
      }, 1200)
    }
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
        <button
          v-if="session.kind !== 'offline'"
          class="btn btn-ghost btn-sm rec-btn"
          :class="{ 'rec-on': session.recOn }"
          :disabled="!recLive"
          :title="session.recOn
            ? '落盘录制中，点击暂停写入日志文件（日志视图不受影响）'
            : '录制已暂停，点击另起新文件继续落盘'"
          aria-label="切换日志落盘录制"
          @click="store.setRec(props.sessionId, !session.recOn)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/></svg>
          <span>录制</span>
        </button>
        <button
          v-if="session.kind !== 'offline'"
          class="btn btn-ghost btn-sm"
          :disabled="!recLive"
          :title="session.recOn
            ? '日志分段：关闭当前文件，从当前时刻另起带时间戳的新文件继续落盘（旧文件保留）'
            : '日志分段：另起带时间戳的新文件并恢复落盘'"
          aria-label="另起新日志分段文件"
          @click="store.rotateLog(props.sessionId)"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M15 18v-6"/><path d="M12 15h6"/></svg>
          <span>分段</span>
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

    <LogScroller
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
        :class="[item.dir, { selected: item.no === selectedNo, flash: item.no === flashNo }]"
        @click="selectedNo = item.no"
        @contextmenu.prevent="openCtx($event, item)"
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
          <span v-if="hexView" class="hex">{{ lineHexDump(item.text, item.bytes) }}</span>
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
    </LogScroller>

    <div v-if="session" class="logview-foot">
      <span class="stats" :title="`RX ${session.rxLines ?? 0} 行 / TX ${session.txLines ?? 0} 行`">
        RX {{ humanizeBytes(session.rxBytes) }} · TX {{ humanizeBytes(session.txBytes) }} · {{ humanizeBytes(bps) }}/s
      </span>
      <span
        v-if="session.droppedLines"
        class="drop-note"
        :title="`前端缓冲上限 ${store.logConfig.viewBufCap.toLocaleString()} 行，超出即从最旧行开始丢弃（自连接或上次清屏起累计）`"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>
        已丢弃 {{ session.droppedLines.toLocaleString() }} 行
      </span>
      <span
        v-if="session.backfillTotal"
        class="drop-note"
        :title="`上滑到顶时已从 ring 回补 ${session.backfillTotal.toLocaleString()} 行（仍在 ring 窗口内的被裁旧行）。更早的行已被 ring 覆盖或属上一连接，无法回补`"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m12 19V5"/><path d="m5 12 7 7 7-7"/></svg>
        已回补 {{ session.backfillTotal.toLocaleString() }} 行
      </span>
    </div>

    <div v-else class="logview-empty">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h0a4 4 0 0 1-4-4V8Z"/></svg>
      <span>打开一个串口开始</span>
    </div>

    <Teleport to="body">
      <template v-if="ctx">
        <div class="ctx-backdrop" @click="closeCtx" @contextmenu.prevent="closeCtx"></div>
        <div class="ctx-menu" :style="{ left: ctx.x + 'px', top: ctx.y + 'px' }" role="menu">
          <button
            class="ctx-item"
            :disabled="!canSendCtx"
            :title="canSendCtx ? '按原始字节以 HEX 模式重发该行' : session?.kind === 'offline' ? '离线会话不可发送' : '会话未连接'"
            @click="resendLine"
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/></svg>
            <span>重发此帧</span>
          </button>
          <div class="ctx-sep"></div>
          <button class="ctx-item" @click="copyCtx('text')">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>
            <span>复制文本</span>
          </button>
          <button class="ctx-item" @click="copyCtx('hex')">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>
            <span>复制 HEX</span>
          </button>
        </div>
      </template>
    </Teleport>
  </div>
</template>
