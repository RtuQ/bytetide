import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { markRaw } from 'vue'
import {
  DEFAULT_ALERT_STATE,
  DEFAULT_LOG_CONFIG,
  DEFAULT_PLOT_CONFIG,
  DEFAULT_SEARCH,
  KEYWORD_PALETTE,
  type AiAnnotation,
  type AlertRule,
  type AlertState,
  type AutoReplyRule,
  type AutoReplyState,
  type ConfigPreset,
  type FilterStage,
  type LogConfig,
  type LogLine,
  type PlotConfig,
  type PortConfig,
  type PortInfo,
  type PortPreset,
  type PresetCategory,
  type RawLogLine,
  type SearchState,
  type SessionKind,
  type SessionStatus,
  type Keyword,
} from '../types'
import { parseLogFile } from '../composables/useLogParser'
import type { DecodedFrame } from '../types/parser'

/** 中心区视图模式：log=仅日志；split=日志+图表同屏；plot=仅图表 */
export type CenterView = 'log' | 'split' | 'plot'

export interface Session {
  id: string
  /** 会话来源：live=实时串口；offline=从日志文件离线载入 */
  kind: SessionKind
  config: PortConfig
  status: SessionStatus
  error: string
  lines: LogLine[]
  lineCounter: number
  /** 补拉水位（epochMillis）：自愈补拉插入到过的最新时刻。迟到事件中
   *  epoch <= 水位的行已被补拉过，appendLines 直接丢弃防重复。0=从未补拉。 */
  pulledThrough: number
  /** 后端 ring 游标（`no`，单调递增、清屏不回退）：拉模型视图通道的拉取位点。
   *  只随 appendPulled 前进；重连=新会话新 ring，随 makeSession 归零。 */
  pullNo: number
  /** 前端缓冲上限裁剪掉的行数（自连接或上次清屏起累计；重连迁移保留） */
  droppedLines: number
  /** 书签行号（升序；随 lines 环形淘汰自然失效——跳转前由 UI 校验行仍存在） */
  bookmarks: number[]
  /** AI 批注（REST 桥写入、事件实时同步；no 为行号，行被淘汰或清屏后标记自动隐藏） */
  aiNotes: AiAnnotation[]
  /** 解码帧（解析引擎产出，no 锚定 LogLine.no；markRaw + 1000 条 FIFO，重连不迁移） */
  decoded: DecodedFrame[]
  sendHistory: string[]
  search: SearchState
  /** 过滤链（与“搜索”独立）：include/exclude 依序作用于显示行集 */
  filters: FilterStage[]
  keywords: Keyword[]
  autoReply: AutoReplyState
  /** 告警规则（RX 行扫描，触发系统通知+历史） */
  alerts: AlertState
  plot: PlotConfig
  /** 中心区视图模式（会话级偏好，重连迁移；clearLog 不清——视图偏好非数据） */
  centerView: CenterView
  followTail: boolean
  onlyMatches: boolean
  hexView: boolean
  showDelta: boolean
  showLineNo: boolean
  showDir: boolean
  rxBytes: number
  txBytes: number
  rxLines: number
  txLines: number
  jump: { no: number; token: number } | null
}

/** 前端环形缓冲上限，超过则丢弃最旧行 */
const MAX_LINES = 50000
/** 解码帧环形上限（plan-parser-v1：1000 条/会话 FIFO） */
const MAX_DECODED = 1000

function makeSession(id: string, config: PortConfig): Session {
  return {
    id,
    kind: 'live',
    config,
    status: 'connecting',
    error: '',
    lines: [],
    lineCounter: 0,
    pulledThrough: 0,
    pullNo: 0,
    droppedLines: 0,
    bookmarks: [],
    aiNotes: [],
    decoded: [],
    sendHistory: [],
    search: { ...DEFAULT_SEARCH },
    filters: [],
    keywords: [],
    autoReply: { enabled: false, rules: [] },
    alerts: { ...DEFAULT_ALERT_STATE, rules: [] },
    plot: { ...DEFAULT_PLOT_CONFIG },
    centerView: 'log',
    followTail: true,
    onlyMatches: false,
    hexView: false,
    showDelta: false,
    showLineNo: true,
    showDir: true,
    rxBytes: 0,
    txBytes: 0,
    rxLines: 0,
    txLines: 0,
    jump: null,
  }
}

let kwSeq = 0
function newKeywordId(): string {
  kwSeq += 1
  return `k${Date.now().toString(36)}${kwSeq}`
}

/** 建账竞态缓冲：状态事件先于会话落账到达时暂存（落账后回放） */
const pendingStatus = new Map<string, string>()
const pendingError = new Map<string, string>()
const PENDING_CAP = 64

let ruleSeq = 0
function newRuleId(): string {
  ruleSeq += 1
  return `r${Date.now().toString(36)}${ruleSeq}`
}

/** 解析引擎的 clearLog 钩子（useParserEngine 注册）：清屏时复位 framer 并 gen+1，
 *  经注册注入避免 store→engine 循环依赖 */
let parserOnClear: ((id: string) => void) | null = null
export function registerParserOnClear(fn: (id: string) => void) {
  parserOnClear = fn
}

const LOG_CONFIG_KEY = 'serialtool.logConfig'
function loadLogConfig(): LogConfig {
  try {
    const raw = localStorage.getItem(LOG_CONFIG_KEY)
    if (!raw) return { ...DEFAULT_LOG_CONFIG }
    const parsed = JSON.parse(raw) as Partial<LogConfig>
    return {
      logPathTemplate: parsed.logPathTemplate ?? '',
      lineTsFormat: parsed.lineTsFormat || DEFAULT_LOG_CONFIG.lineTsFormat,
    }
  } catch {
    return { ...DEFAULT_LOG_CONFIG }
  }
}

const SEARCH_HISTORY_KEY = 'serialtool.searchHistory'
const SEARCH_HISTORY_MAX = 20
function loadSearchHistory(): string[] {
  try {
    const raw = localStorage.getItem(SEARCH_HISTORY_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw)
    return Array.isArray(arr) ? arr.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

const PRESETS_KEY = 'serialtool.portPresets'
function loadPresets(): PortPreset[] {
  try {
    const raw = localStorage.getItem(PRESETS_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw)
    if (!Array.isArray(arr)) return []
    return arr
      .filter(
        (x): x is { id: string; name: string; config: PortConfig } =>
          !!x && typeof x.id === 'string' && typeof x.name === 'string' && !!x.config,
      )
      .map((x) => ({ id: x.id, name: x.name, config: { ...x.config } }))
  } catch {
    return []
  }
}
function savePresets(list: PortPreset[]) {
  try {
    localStorage.setItem(PRESETS_KEY, JSON.stringify(list))
  } catch {
    /* ignore */
  }
}

const CONFIG_PRESETS_KEY = 'serialtool.configPresets'
const CONFIG_PRESET_CATEGORIES: PresetCategory[] = ['filters', 'keywords', 'autoReply', 'plots']
/** 类别名→预设数量上限（防 localStorage 膨胀） */
const CONFIG_PRESETS_CAP = 50

function loadConfigPresets(): ConfigPreset[] {
  try {
    const raw = localStorage.getItem(CONFIG_PRESETS_KEY)
    if (!raw) return []
    const arr = JSON.parse(raw)
    if (!Array.isArray(arr)) return []
    return arr.filter(
      (x): x is ConfigPreset =>
        !!x &&
        typeof x.id === 'string' &&
        typeof x.name === 'string' &&
        typeof x.createdAt === 'number' &&
        CONFIG_PRESET_CATEGORIES.includes(x.category as PresetCategory) &&
        x.data != null,
    )
  } catch {
    return []
  }
}
function saveConfigPresets(list: ConfigPreset[]) {
  try {
    localStorage.setItem(CONFIG_PRESETS_KEY, JSON.stringify(list))
  } catch {
    /* ignore */
  }
}

let presetSeq = 0

export const useSessionStore = defineStore('session', {
  state: () => ({
    sessions: {} as Record<string, Session>,
    order: [] as string[],
    activeId: null as string | null,
    ports: [] as PortInfo[],
    logConfig: loadLogConfig(),
    searchHistory: loadSearchHistory(),
    configPresets: loadConfigPresets(),
    presets: loadPresets(),
    splitMode: false,
    compareMode: false,
    columns: [] as (string | null)[],
  }),
  getters: {
    active(state): Session | null {
      if (!state.activeId) return null
      return state.sessions[state.activeId] ?? null
    },
    sessionList(state): Session[] {
      const list: Session[] = []
      for (const id of state.order) {
        const s = state.sessions[id]
        if (s) list.push(s)
      }
      return list
    },
  },
  actions: {
    setPorts(ports: PortInfo[]) {
      this.ports = ports
    },
    setLogConfig(patch: Partial<LogConfig>) {
      this.logConfig = { ...this.logConfig, ...patch }
      try {
        localStorage.setItem(LOG_CONFIG_KEY, JSON.stringify(this.logConfig))
      } catch {
        /* ignore */
      }
    },
    addPreset(name: string, config: PortConfig) {
      const trimmed = name.trim()
      if (!trimmed) return
      presetSeq += 1
      this.presets = [
        ...this.presets,
        { id: `p${Date.now().toString(36)}${presetSeq}`, name: trimmed, config: { ...config } },
      ]
      savePresets(this.presets)
    },
    removePreset(id: string) {
      this.presets = this.presets.filter((p) => p.id !== id)
      savePresets(this.presets)
    },
    renamePreset(id: string, name: string) {
      const trimmed = name.trim()
      if (!trimmed) return
      this.presets = this.presets.map((p) => (p.id === id ? { ...p, name: trimmed } : p))
      savePresets(this.presets)
    },
    pushSearchHistory(pattern: string) {
      const p = pattern.trim()
      if (!p) return
      const next = [p, ...this.searchHistory.filter((x) => x !== p)].slice(0, SEARCH_HISTORY_MAX)
      this.searchHistory = next
      try {
        localStorage.setItem(SEARCH_HISTORY_KEY, JSON.stringify(next))
      } catch {
        /* ignore */
      }
    },
    removeSearchHistory(pattern: string) {
      this.searchHistory = this.searchHistory.filter((x) => x !== pattern)
      try {
        localStorage.setItem(SEARCH_HISTORY_KEY, JSON.stringify(this.searchHistory))
      } catch {
        /* ignore */
      }
    },
    /** 进入分屏：用已打开会话填充前两列（不足则留空），列数 2~4 */
    enterSplit() {
      const ids = [...this.order]
      this.columns = [ids[0] ?? null, ids[1] ?? null]
      this.splitMode = true
    },
    exitSplit() {
      this.splitMode = false
      this.columns = []
    },
    /** 双会话时间对齐对比：全局布局态，占中心区（与 splitMode 同级） */
    toggleCompareMode() {
      this.compareMode = !this.compareMode
    },
    /** 设置某列绑定的会话；若该会话已在别列，则两列互换（避免同会话出现两次） */
    setColumnSession(i: number, id: string | null) {
      if (i < 0 || i >= this.columns.length) return
      const cols = [...this.columns]
      if (id) {
        const j = cols.findIndex((c, idx) => idx !== i && c === id)
        if (j >= 0) cols[j] = cols[i]
      }
      cols[i] = id
      this.columns = cols
    },
    /** 增加一列：优先填充未占用的已打开会话，最多 4 列 */
    addColumn() {
      if (this.columns.length >= 4) return
      const used = new Set(this.columns.filter((c): c is string => !!c))
      const next = this.order.find((id) => !used.has(id)) ?? null
      this.columns = [...this.columns, next]
    },
    /** 删除一列：至少保留 2 列 */
    removeColumn(i: number) {
      if (this.columns.length <= 2) return
      this.columns = this.columns.filter((_, idx) => idx !== i)
    },
    async refreshPorts() {
      try {
        this.ports = await invoke<PortInfo[]>('list_ports_cmd')
      } catch {
        this.ports = []
      }
    },
    async openTab(config: PortConfig) {
      const id = await invoke<string>('connect_cmd', {
        config,
        logSettings: this.logConfig,
      })
      const s = makeSession(id, config)
      this.sessions[id] = s
      this.order.push(id)
      this.activeId = id
      this.flushPending(id)
      this.pushLiveRules(id)
      return id
    },
    /** 建本地会话（不经 invoke/后端）：测试与无后端冒烟环境用。
     *  只创建前端态，不启动任何读线程；id 由调用方指定避免与后端 s{N}/o{N} 冲突。 */
    createLocalSession(id: string, config: PortConfig): string {
      if (this.sessions[id]) return id
      const s = makeSession(id, config)
      s.status = 'offline'
      this.sessions[id] = s
      this.order.push(id)
      this.activeId = id
      this.flushPending(id)
      return id
    },
    /** 从日志文件离线载入：后端建 ring 会话（o{N}），前端仍灌 UI ring；REST 桥可见 */
    async loadOfflineSession(path: string) {
      const content = await invoke<string>('read_text_file_cmd', { path })
      const parsed = parseLogFile(content)
      const baseName = path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') || '离线日志'
      const config: PortConfig = {
        name: baseName,
        baudRate: 0,
        dataBits: 8,
        parity: 'none',
        stopBits: '1',
        flowControl: 'none',
      }
      const id = await invoke<string>('create_offline_session_cmd', {
        config,
        path,
        lines: parsed.lines,
      })
      const s = makeSession(id, config)
      s.kind = 'offline'
      s.status = 'offline'
      this.sessions[id] = s
      this.order.push(id)
      this.activeId = id
      if (parsed.lines.length) this.appendLines(id, parsed.lines)
      return id
    },
    async closeTab(id: string) {
      try {
        await invoke('disconnect_cmd', { sessionId: id })
      } catch {
        /* ignore */
      }
      delete this.sessions[id]
      this.order = this.order.filter((x) => x !== id)
      if (this.activeId === id) {
        this.activeId = this.order[this.order.length - 1] ?? null
      }
    },
    /** 断开串口但保留标签页与日志，便于稍后重连（区别于 closeTab 的彻底关闭） */
    async stopSession(id: string) {
      const s = this.sessions[id]
      if (!s) return
      if (s.kind === 'offline') return
      try {
        await invoke('disconnect_cmd', { sessionId: id })
      } catch {
        /* ignore */
      }
      s.status = 'disconnected'
      s.error = ''
    },
    /** 用原配置重连：后端生成新会话 id，前端把原会话数据迁移到新 id 下 */
    async reconnectSession(id: string) {
      const s = this.sessions[id]
      if (!s) return
      const config = s.config
      let newId: string
      try {
        newId = await invoke<string>('connect_cmd', {
          config,
          logSettings: this.logConfig,
        })
      } catch (e: unknown) {
        s.error = String(e instanceof Error ? e.message : e)
        s.status = 'error'
        return
      }
      const carried: Session = {
        ...makeSession(newId, config),
        lines: s.lines,
        lineCounter: s.lineCounter,
        droppedLines: s.droppedLines,
        bookmarks: [...s.bookmarks],
        aiNotes: s.aiNotes.map((n) => ({ ...n })),
        sendHistory: s.sendHistory,
        search: s.search,
        filters: s.filters.map((f) => ({ ...f })),
        keywords: s.keywords.map((k) => ({ ...k })),
        autoReply: s.autoReply,
        alerts: { enabled: s.alerts.enabled, rules: s.alerts.rules.map((r) => ({ ...r })) },
        plot: s.plot,
        centerView: s.centerView,
        followTail: s.followTail,
        onlyMatches: s.onlyMatches,
        hexView: s.hexView,
        showDelta: s.showDelta,
        showLineNo: s.showLineNo,
        showDir: s.showDir,
        rxBytes: s.rxBytes,
        txBytes: s.txBytes,
        rxLines: s.rxLines,
        txLines: s.txLines,
        jump: s.jump,
      }
      delete this.sessions[id]
      this.sessions[newId] = carried
      this.order = this.order.map((x) => (x === id ? newId : x))
      if (this.activeId === id) this.activeId = newId
      // 旧 id 的积压事件已无意义，新 id 回放建账竞态期间的状态
      pendingStatus.delete(id)
      pendingError.delete(id)
      this.flushPending(newId)
      this.pushLiveRules(newId)
    },
    async send(id: string, text: string, mode: 'ascii' | 'hex') {
      const s = this.sessions[id]
      if (!s) return
      await invoke('send_cmd', { sessionId: id, mode, text })
      s.sendHistory = [text, ...s.sendHistory.filter((t) => t !== text)].slice(0, 20)
    },
    async clearLog(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.lines = []
      s.lineCounter = 0
      s.pulledThrough = 0
      // 行号已从 1 重新计数，旧书签全部失义，一并清空
      s.bookmarks = []
      // 丢弃计数与"当前视图缺口"绑定：清屏后缓冲从头开始，旧缺口已无意义，
      // 归零避免让人误以为当前日志仍缺数据（重连迁移保留，因日志行本身被带走）
      s.droppedLines = 0
      // AI 批注锚定行号，同样失义：本地清空并同步后端镜像
      s.aiNotes = []
      // 解码帧同样锚定行号：清空并由解析引擎复位该会话切帧状态（gen+1）
      s.decoded = []
      invoke('bridge_sync_annotations_cmd', { sessionId: id, annotations: [] }).catch(() => {})
      try {
        await invoke('clear_log_cmd', { sessionId: id })
      } catch {
        /* ignore */
      }
      parserOnClear?.(id)
    },
    /** 推送实时规则到后端（拉模型：评估在读线程，规则变更/重连后整体覆盖） */
    pushLiveRules(id: string) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      invoke('set_live_rules_cmd', {
        sessionId: id,
        autoReply: { enabled: s.autoReply.enabled, rules: s.autoReply.rules },
        alerts: { enabled: s.alerts.enabled, rules: s.alerts.rules },
      }).catch(() => {})
    },
    async openLogPath(id: string) {
      try {
        const p = await invoke<string>('session_log_path_cmd', { sessionId: id })
        alert(p)
      } catch (e) {
        alert(String(e))
      }
    },
    /** 追加日志并返回本次带行号的新行（供告警/回复等后续处理拿到 no）。
     *  行对象 markRaw：日志行创建后不可变，跳过 Vue 深层 Proxy 包装——
     *  长跑时多条全量扫描（高亮/统计/过滤）才能以原生对象速度进行。 */
    appendLines(id: string, raw: RawLogLine[]): LogLine[] {
      const s = this.sessions[id]
      if (!s || raw.length === 0) return []
      // 补拉水位去重：事件队列晚到的行若已被自愈补拉插入（epoch <= 水位），
      // 直接丢弃，防止同一行出现两次（否则缓冲加速膨胀、赤字自我强化）
      const arr =
        s.pulledThrough > 0 ? raw.filter((r) => r.epochMillis > s.pulledThrough) : raw
      if (arr.length === 0) return []
      // 用 concat 产生新数组引用，保证虚拟滚动器感知变化
      const fresh: LogLine[] = arr.map((r) => markRaw({ no: ++s.lineCounter, ...r }))
      const total = s.lines.length + fresh.length
      s.lines = s.lines.concat(fresh)
      if (total > MAX_LINES) {
        s.lines = s.lines.slice(total - MAX_LINES)
        s.droppedLines += total - MAX_LINES
      }
      return fresh
    },
    /** 拉模型摄取：后端 ring 按 `no` 游标拉到的行一次性入表。
     *  游标（pullNo）是不重不漏的唯一真相——调用方（拉取循环）保证行序按 no
     *  升序；防御性过滤 ringNo <= pullNo 的重复行，其余全部入表并推进游标。
     *  返回本次插入的行（供 autoReply/alerts/tally 拿 no 与内容）。 */
    appendPulled(
      id: string,
      lines: (RawLogLine & { ringNo: number })[],
    ): LogLine[] {
      const s = this.sessions[id]
      if (!s || lines.length === 0) return []
      const arr = lines.filter((l) => l.ringNo > s.pullNo)
      if (arr.length === 0) return []
      // 用 concat 产生新数组引用，保证虚拟滚动器感知变化
      // rn=后端 ring no（告警命中事件回查 UI 行号用；不入显示列）
      const fresh: LogLine[] = arr.map((r) =>
        markRaw({ no: ++s.lineCounter, ts: r.ts, dir: r.dir, text: r.text, bytes: r.bytes, epochMillis: r.epochMillis, rn: r.ringNo }),
      )
      const total = s.lines.length + fresh.length
      s.lines = s.lines.concat(fresh)
      if (total > MAX_LINES) {
        s.lines = s.lines.slice(total - MAX_LINES)
        s.droppedLines += total - MAX_LINES
      }
      s.pullNo = arr[arr.length - 1]!.ringNo
      return fresh
    },
    /** 解析引擎落表：解码帧追加（元素 markRaw + 1000 条 FIFO；replace=true 用于回溯整表替换）。
     *  200ms 节流批量由调用方（useParserEngine）负责，这里只管入表。 */
    applyDecoded(id: string, frames: DecodedFrame[], replace = false) {
      const s = this.sessions[id]
      if (!s) return
      const fresh = frames.map((f) => markRaw({ ...f }))
      s.decoded = replace ? fresh : s.decoded.concat(fresh)
      if (s.decoded.length > MAX_DECODED) {
        s.decoded = s.decoded.slice(s.decoded.length - MAX_DECODED)
      }
    },
    /** 清空解码帧（卸载/停用回溯前重置） */
    resetDecoded(id: string) {
      const s = this.sessions[id]
      if (s) s.decoded = []
    },
    /** 累计 RX/TX 字节与行数（lifetime，随缓冲裁剪不回退）；与 appendLines 分离，不改其行为 */
    tallyBytes(id: string, raw: RawLogLine[]) {
      const s = this.sessions[id]
      if (!s || raw.length === 0) return
      const enc = new TextEncoder()
      for (const r of raw) {
        const bytes = enc.encode(r.text).length
        if (r.dir === 'rx') {
          s.rxBytes = (s.rxBytes ?? 0) + bytes
          s.rxLines = (s.rxLines ?? 0) + 1
        } else {
          s.txBytes = (s.txBytes ?? 0) + bytes
          s.txLines = (s.txLines ?? 0) + 1
        }
      }
    },
    setStatus(id: string, status: string) {
      const s = this.sessions[id]
      if (!s) {
        if (pendingStatus.size < PENDING_CAP) pendingStatus.set(id, status)
        return
      }
      if (
        status === 'connecting' ||
        status === 'connected' ||
        status === 'disconnected' ||
        status === 'error' ||
        status === 'offline'
      ) {
        s.status = status
      }
    },
    setError(id: string, error: string) {
      const s = this.sessions[id]
      if (!s) {
        if (pendingError.size < PENDING_CAP) pendingError.set(id, error)
        return
      }
      s.error = error
      if (error) s.status = 'error'
    },
    /** 会话落账后回放竞态期间积压的连接状态/错误（先状态后错误，error 优先） */
    flushPending(id: string) {
      const st = pendingStatus.get(id)
      if (st !== undefined) {
        pendingStatus.delete(id)
        this.setStatus(id, st)
      }
      const err = pendingError.get(id)
      if (err !== undefined) {
        pendingError.delete(id)
        this.setError(id, err)
      }
    },
    setActive(id: string) {
      this.activeId = id
    },
    updateSearch(id: string, patch: Partial<SearchState>) {
      const s = this.sessions[id]
      if (s) s.search = { ...s.search, ...patch }
    },
    addKeyword(id: string) {
      const s = this.sessions[id]
      if (!s) return
      const color = KEYWORD_PALETTE[s.keywords.length % KEYWORD_PALETTE.length]
      s.keywords.push({
        id: newKeywordId(),
        pattern: '',
        color,
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
      })
    },
    updateKeyword(id: string, kid: string, patch: Partial<Keyword>) {
      const s = this.sessions[id]
      if (!s) return
      const k = s.keywords.find((x) => x.id === kid)
      if (k) Object.assign(k, patch)
    },
    removeKeyword(id: string, kid: string) {
      const s = this.sessions[id]
      if (!s) return
      s.keywords = s.keywords.filter((x) => x.id !== kid)
    },
    setFollowTail(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.followTail = v
    },
    setOnlyMatches(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.onlyMatches = v
    },
    setHexView(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.hexView = v
    },
    setShowDelta(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showDelta = v
    },
    setShowLineNo(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showLineNo = v
    },
    setShowDir(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showDir = v
    },
    /** 切换书签：存在则移除，否则按行号升序插入 */
    toggleBookmark(id: string, no: number) {
      const s = this.sessions[id]
      if (!s) return
      const i = s.bookmarks.indexOf(no)
      if (i >= 0) s.bookmarks.splice(i, 1)
      else {
        let at = s.bookmarks.length
        for (let k = 0; k < s.bookmarks.length; k++) {
          if (s.bookmarks[k]! > no) {
            at = k
            break
          }
        }
        s.bookmarks.splice(at, 0, no)
      }
    },
    removeBookmark(id: string, no: number) {
      const s = this.sessions[id]
      if (!s) return
      const i = s.bookmarks.indexOf(no)
      if (i >= 0) s.bookmarks.splice(i, 1)
    },
    clearBookmarks(id: string) {
      const s = this.sessions[id]
      if (s) s.bookmarks = []
    },
    /** 开启绘图：同时强制 HEX 视图（绘图仅支持 hex 格式接收模式）；关闭时不改动 hexView。
     *  视图耦合（布局重构 V1）：显式开图自动切到图表视图、关图时图表类视图回落 log，
     *  维持直觉「启用开关看得见效果」；不变式：centerView !== 'log' ⟹ plot.enabled */
    setPlotEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (!s) return
      s.plot = { ...s.plot, enabled: v }
      if (v) {
        s.hexView = true
        if (s.centerView === 'log') s.centerView = 'plot'
      } else if (s.centerView !== 'log') {
        s.centerView = 'log'
      }
      this._pushPlot(id)
    },
    /** 切换中心区视图模式；进入 split/plot 时若图表未启用则顺带启用（一次点击即出图） */
    setCenterView(id: string, view: CenterView) {
      const s = this.sessions[id]
      if (!s || s.centerView === view) return
      s.centerView = view
      if (view !== 'log' && !s.plot.enabled) this.setPlotEnabled(id, true)
    },
    /** 更新绘图解析配置（帧头/帧尾/校验/通道等），enabled 经 setPlotEnabled 单独控制 */
    updatePlot(id: string, patch: Partial<PlotConfig>) {
      const s = this.sessions[id]
      if (!s) return
      s.plot = { ...s.plot, ...patch }
      this._pushPlot(id)
    },
    /** 仅 live 会话：把绘图配置同步到后端供 REST `/decode` 复用；失败静默，不打断前端绘图 */
    _pushPlot(id: string) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      invoke('set_plot_config_cmd', { sessionId: id, config: { ...s.plot } }).catch(() => {})
    },
    /** 采纳 REST 桥写回的绘图文法（bridge-annotations-updated 同源通道）：整包替换本地状态。
     *  后端 manager 已持有该配置，无需回推 _pushPlot；缺省字段用默认值回填。 */
    adoptBridgePlot(id: string, cfg: PlotConfig) {
      const s = this.sessions[id]
      if (!s) return
      s.plot = { ...DEFAULT_PLOT_CONFIG, ...cfg }
    },
    /** 采纳 AI 批注（bridge-annotations-updated 事件）：整包替换 */
    applyBridgeAnnotations(id: string, notes: AiAnnotation[]) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = notes
    },
    /** 删除单条 AI 批注并同步后端镜像 */
    removeAiNote(id: string, noteId: string) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = s.aiNotes.filter((n) => n.id !== noteId)
      this._pushAiNotes(id)
    },
    /** 清空 AI 批注并同步后端镜像 */
    clearAiNotes(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = []
      this._pushAiNotes(id)
    },
    /** 前端 → 后端镜像的整包回写（仅删除/清空方向会用到；AI 写入方向由 bridge.rs 推送） */
    _pushAiNotes(id: string) {
      const s = this.sessions[id]
      if (!s) return
      invoke('bridge_sync_annotations_cmd', { sessionId: id, annotations: [...s.aiNotes] }).catch(
        () => {},
      )
    },
    setAutoReplyEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.autoReply.enabled = v
      this.pushLiveRules(id)
    },
    addAutoReplyRule(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.autoReply.rules.push({
        id: newRuleId(),
        trigger: '',
        reply: '',
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
        appendNewline: false,
        replyMode: 'ascii',
        enabled: true,
      })
      this.pushLiveRules(id)
    },
    updateAutoReplyRule(id: string, rid: string, patch: Partial<AutoReplyRule>) {
      const s = this.sessions[id]
      if (!s) return
      const r = s.autoReply.rules.find((x) => x.id === rid)
      if (r) Object.assign(r, patch)
      this.pushLiveRules(id)
    },
    removeAutoReplyRule(id: string, rid: string) {
      const s = this.sessions[id]
      if (!s) return
      s.autoReply.rules = s.autoReply.rules.filter((x) => x.id !== rid)
      this.pushLiveRules(id)
    },
    setAlertsEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.alerts.enabled = v
      this.pushLiveRules(id)
    },
    addAlertRule(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.alerts.rules.push({
        id: newRuleId(),
        pattern: '',
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
        minCount: 1,
        windowSec: 0,
        cooldownSec: 30,
        level: 'warn',
        enabled: true,
      })
      this.pushLiveRules(id)
    },
    updateAlertRule(id: string, rid: string, patch: Partial<AlertRule>) {
      const s = this.sessions[id]
      if (!s) return
      const r = s.alerts.rules.find((x) => x.id === rid)
      if (r) Object.assign(r, patch)
      this.pushLiveRules(id)
    },
    removeAlertRule(id: string, rid: string) {
      const s = this.sessions[id]
      if (!s) return
      s.alerts.rules = s.alerts.rules.filter((x) => x.id !== rid)
      this.pushLiveRules(id)
    },

    /** 过滤链：添加一级（默认 include 单行匹配） */
    addFilterStage(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.filters.push({
        id: newRuleId(),
        text: '',
        mode: 'include',
        dir: 'any',
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
        enabled: true,
      })
    },
    updateFilterStage(id: string, fid: string, patch: Partial<FilterStage>) {
      const s = this.sessions[id]
      if (!s) return
      const f = s.filters.find((x) => x.id === fid)
      if (f) Object.assign(f, patch)
    },
    removeFilterStage(id: string, fid: string) {
      const s = this.sessions[id]
      if (!s) return
      s.filters = s.filters.filter((x) => x.id !== fid)
    },
    clearFilterStages(id: string) {
      const s = this.sessions[id]
      if (s) s.filters = []
    },

    /** 保存命名预设到库（data 形状由调用方保证），同类超过上限丢弃最旧 */
    saveConfigPreset(category: PresetCategory, name: string, data: unknown) {
      const next: ConfigPreset = {
        id: `cp${Date.now().toString(36)}${presetSeq++}`,
        name: name.trim() || `${category} 预设`,
        category,
        createdAt: Date.now(),
        data,
      }
      const merged = [next, ...this.configPresets]
      const counts = new Map<string, number>()
      const out: ConfigPreset[] = []
      for (const p of merged) {
        const n = counts.get(p.category) ?? 0
        if (n >= CONFIG_PRESETS_CAP) continue
        counts.set(p.category, n + 1)
        out.push(p)
      }
      this.configPresets = out
      saveConfigPresets(out)
    },
    removeConfigPreset(pid: string) {
      this.configPresets = this.configPresets.filter((p) => p.id !== pid)
      saveConfigPresets(this.configPresets)
    },
    /**
     * 套用预设到当前活动会话：按类别写入对应配置。
     * data 来自导入文件时可能畸形，各分支做最小形状校验后再落地。
     */
    applyConfigPreset(pid: string): boolean {
      const p = this.configPresets.find((x) => x.id === pid)
      const activeId = this.activeId
      const s = activeId ? this.sessions[activeId] : null
      if (!p || !s) return false
      if (p.category === 'filters' && Array.isArray(p.data)) {
        s.filters = p.data.map((f) => ({ ...(f as FilterStage), id: newRuleId() }))
      } else if (p.category === 'keywords' && Array.isArray(p.data)) {
        s.keywords = p.data.map((k) => ({ ...(k as Keyword), id: newKeywordId() }))
      } else if (p.category === 'autoReply' && p.data && typeof p.data === 'object') {
        const d = p.data as Partial<typeof s.autoReply>
        s.autoReply = {
          enabled: !!d.enabled,
          rules: Array.isArray(d.rules)
            ? d.rules.map((r) => ({ ...r, id: newRuleId() }))
            : [],
        }
      } else if (p.category === 'plots' && p.data && typeof p.data === 'object') {
        s.plot = { ...DEFAULT_PLOT_CONFIG, ...(p.data as object) } as PlotConfig
        if (s.kind !== 'offline') this._pushPlot(activeId!)
      } else {
        return false
      }
      return true
    },
    /** 导入预设包 JSON（整库合并，id 冲突重生成）；返回导入条数 */
    importConfigPresets(raw: unknown): number {
      const arr = raw && typeof raw === 'object' ? (raw as { presets?: unknown }).presets : null
      if (!Array.isArray(arr)) return 0
      let n = 0
      const existing = new Set(this.configPresets.map((p) => p.id))
      const merged = [...this.configPresets]
      for (const x of arr) {
        if (
          !x ||
          typeof x !== 'object' ||
          typeof (x as ConfigPreset).category !== 'string' ||
          !CONFIG_PRESET_CATEGORIES.includes((x as ConfigPreset).category)
        )
          continue
        const p = x as ConfigPreset
        const id = existing.has(p.id) ? `cp${Date.now().toString(36)}${presetSeq++}` : p.id
        existing.add(id)
        merged.push({
          id,
          name: typeof p.name === 'string' ? p.name : `${p.category} 预设`,
          category: p.category,
          createdAt: typeof p.createdAt === 'number' ? p.createdAt : Date.now(),
          data: p.data,
        })
        n += 1
      }
      this.configPresets = merged
      saveConfigPresets(this.configPresets)
      return n
    },
    /**
     * 自动回复：对每条 RX 行按规则（启用且 trigger 非空）做布尔匹配，
     * 命中则直接调 send_cmd 回复（TX，仅匹配 RX 故不会循环）。
     * 一条 RX 行最多命中第一条匹配规则，避免一收多发。
     */
    requestJump(id: string, no: number) {
      const s = this.sessions[id]
      if (s) s.jump = { no, token: Date.now() }
    },
  },
})
