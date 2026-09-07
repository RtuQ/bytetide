import { reactive, watch } from 'vue'
import { useSessionStore, registerParserOnClear } from '../stores/session'
import { ParserEngine } from '../parser/engine'
import { createScriptHost } from '../parser/bootstrap'
import type { LogLine } from '../types'
import type { DecodedFrame, ParserBanner, ParserStats, TrialReport, ValidatedScript } from '../types/parser'

/**
 * 解析引擎组合层（模块级单例）：加载/卸载/启停/localStorage 恢复/reset 挂载。
 * 引擎（ParserEngine）是纯 TS 编排；这里负责 Vue 响应式映射、store 落表节流与
 * 生命周期挂钩（onClear 注入 + order diff watch），App 各组件共享同一实例。
 */

const STORAGE_KEY = 'serialtool.parserScript'
/** 解码落表节流：与拉取循环同节奏，批量 applyDecoded */
const FLUSH_MS = 200

export interface ParserUiState {
  loaded: boolean
  enabled: boolean
  hasParse: boolean
  script: BytetideParser.Meta | null
  framingSummary: string | null
  fieldsCount: number | null
  banner: ParserBanner | null
  stats: ParserStats
  trialReport: TrialReport | null
  /** 脚本源码（查看器弹层只读预览） */
  source: string | null
}

const ui = reactive<ParserUiState>({
  loaded: false,
  enabled: false,
  hasParse: false,
  script: null,
  framingSummary: null,
  fieldsCount: null,
  banner: null,
  stats: { frames: 0, ok: 0, crcFailed: 0, parseErrors: 0, dropped: 0, types: 0 },
  trialReport: null,
  source: null,
})

let engine: ParserEngine | null = null

/** framing 摘要一行（面板卡片用） */
function framingSummary(script: ValidatedScript): string {
  const f = script.framing
  const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, '0').toUpperCase()).join(' ')
  const len =
    f.length.kind === 'fixed'
      ? `定长 ${f.length.value}B`
      : f.length.kind === 'field'
        ? `长度域 ${f.length.fmt}@${f.length.at}+${f.length.add}`
        : f.length.kind === 'until'
          ? `分隔符 ${hex(f.length.tail)}`
          : '整行一帧'
  const parts = [
    f.source === 'ascii-hex' ? 'ASCII-Hex' : '二进制',
    len,
    f.sync.length ? `同步 ${hex(f.sync)}` : null,
    f.crc ? `CRC ${f.crc.algo}` : null,
  ]
  return parts.filter((x): x is string => !!x).join(' · ')
}

export function useParserEngine() {
  const store = useSessionStore()
  ensureEngine(store)
  return { ui, importScript, reloadScript, unloadScript, setEnabled }
}

/** 拉取循环/离线载入的数据入口：未加载脚本时零开销 no-op。
 *  引擎未初始化（面板从未挂载）同样静默——脚本都没启用，无需喂帧。 */
export function feedParser(sessionId: string, lines: LogLine[]) {
  engine?.feed(sessionId, lines)
}

// ---- 单例构建与全局挂钩 ----

let restoring = false
function ensureEngine(store: ReturnType<typeof useSessionStore>) {
  if (engine) return
  engine = new ParserEngine({
    hostFactory: () => createScriptHost((reqId) => engine?.notifyAck(reqId)),
    importModule: (url) => import(/* @vite-ignore */ url),
    makeBlobUrl: (src) => URL.createObjectURL(new Blob([src], { type: 'text/javascript' })),
    onDecoded: (id, frames, replace) => enqueueDecoded(store, id, frames, replace),
    onStateChange: () => syncUi(engine!),
    getSessionName: (id) => store.sessions[id]?.config.name ?? id,
  })

  // clearLog 钩子：store 不反向 import 引擎，经注册注入（避免循环依赖）
  registerParserOnClear((id) => engine?.resetSession(id))

  // order diff watch：关闭标签页/重连迁移旧 id 后，清引擎里该会话的 framer 与 gen
  watch(
    () => [...store.order],
    (next, prev) => {
      for (const id of prev) {
        if (!next.includes(id)) engine?.dropSession(id)
      }
    },
  )

  // 启动静默恢复（不阻塞首帧）
  void restore(store)
}

// ---- 解码落表（200ms 节流批量入 store） ----

const pendingDecoded = new Map<string, { frames: DecodedFrame[]; replace: boolean }>()
let flushTimer: ReturnType<typeof setTimeout> | null = null

function enqueueDecoded(
  store: ReturnType<typeof useSessionStore>,
  id: string,
  frames: DecodedFrame[],
  replace: boolean,
) {
  const cur = pendingDecoded.get(id)
  if (replace || !cur) pendingDecoded.set(id, { frames, replace })
  else cur.frames = cur.frames.concat(frames)
  if (flushTimer !== null) return
  flushTimer = setTimeout(() => {
    flushTimer = null
    for (const [sid, p] of pendingDecoded) {
      if (p.replace) store.resetDecoded(sid)
      store.applyDecoded(sid, p.frames)
    }
    pendingDecoded.clear()
  }, FLUSH_MS)
}

// ---- UI 状态同步 ----

function syncUi(e: ParserEngine) {
  const script = e.script
  ui.loaded = e.loaded
  ui.enabled = e.enabled
  ui.hasParse = script?.hasParse ?? false
  ui.script = script?.meta ?? null
  ui.framingSummary = script ? framingSummary(script) : null
  ui.fieldsCount = script?.fields ? script.fields.length : null
  ui.banner = e.banner
  ui.stats = e.stats
  ui.trialReport = e.trialReport
}

// ---- localStorage 持久化（源码 + 启用态；meta 由加载时派生） ----

function persist(src: string | null, enabled: boolean) {
  try {
    if (!src) localStorage.removeItem(STORAGE_KEY)
    else localStorage.setItem(STORAGE_KEY, JSON.stringify({ src, enabled }))
  } catch {
    /* ignore */
  }
}

function sessionsSnapshot(store: ReturnType<typeof useSessionStore>) {
  return store.sessionList.map((s) => ({ id: s.id, lines: s.lines as LogLine[] }))
}

async function restore(store: ReturnType<typeof useSessionStore>) {
  if (restoring) return
  restoring = true
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return
    const saved = JSON.parse(raw) as { src?: unknown; enabled?: unknown }
    if (typeof saved.src !== 'string') return
    const r = await engine!.load(saved.src)
    if (!r.ok) {
      persist(null, false)
      return
    }
    ui.source = saved.src
    if (saved.enabled === true) {
      await engine!.setEnabled(true, sessionsSnapshot(store))
      persist(saved.src, true)
    }
  } catch {
    /* 恢复失败静默（脚本内容损坏等） */
  } finally {
    restoring = false
  }
}

// ---- 面板动作 ----

/** 导入脚本：装载 → 试运行三分类（suspect 停在已导入未启用；否则自动启用） */
async function importScript(src: string): Promise<{ ok: boolean; error?: string }> {
  const store = useSessionStore()
  ensureEngine(store)
  const e = engine!
  const r = await e.load(src)
  if (!r.ok) return r
  ui.source = src
  persist(src, false)
  const report = await e.trialRun(store.activeId, sessionsSnapshot(store))
  if (report.verdict !== 'suspect') {
    await e.setEnabled(true, sessionsSnapshot(store))
    persist(src, true)
  }
  return { ok: true }
}

/** 重新加载当前脚本（重跑装载+试运行） */
async function reloadScript(): Promise<{ ok: boolean; error?: string }> {
  const src = ui.source
  if (!src) return { ok: false, error: '没有已加载的脚本' }
  return importScript(src)
}

async function unloadScript() {
  const store = useSessionStore()
  engine?.unload(sessionsSnapshot(store))
  ui.source = null
  for (const s of store.sessionList) store.resetDecoded(s.id)
  persist(null, false)
}

async function setEnabled(v: boolean) {
  const store = useSessionStore()
  if (!engine?.loaded) return
  await engine.setEnabled(v, sessionsSnapshot(store))
  persist(ui.source ?? '', v)
}
