import type {
  AiAnnotation,
  AlertState,
  AutoReplyState,
  CaptureCfg,
  FilterStage,
  Keyword,
  LogLine,
  PlotConfig,
  PortConfig,
  SearchState,
  SessionKind,
  SessionStatus,
} from '../../types'
import {
  DEFAULT_ALERT_STATE,
  DEFAULT_PLOT_CONFIG,
  DEFAULT_SEARCH,
  makeCaptureCfg,
} from '../../types'
import type { DecodedFrame } from '../../types/parser'

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
  /** 后端 ring 覆盖丢行：拉取游标检测到的缺口（ring 容量窗口内未来得及拉取就被
   *  覆盖的行；重连不迁移——新 ring 从零计；清屏归零） */
  ringDropped: number
  /** 视口锚定待补偿：已被裁剪但尚未被 LogView 消费的行数（takeEvicted 取走即清零；
   *  重连不迁移；清屏归零） */
  evictedPending: number
  /** 新 ring 纪元的行号下界：重连时记录迁移自旧会话的 lineCounter——迁移行携带
   *  旧 ring 的 rn，而新 ring no 从 1 重新计数，no <= reconnectNo 的行不可往前
   *  翻页回补（旧 ring 已销毁）。makeSession=0；clearLog 重置 0。 */
  reconnectNo: number
  /** 翻页补旧行累计（方案 B）：视图缓冲裁掉的行从 ring 回补的总行数（兼作
   *  useHighlighter/useLineStats/usePlotData 的 prepend 重建信号）；
   *  重连不迁移（新 ring 无旧史可补）；清屏归零 */
  backfillTotal: number
  /** ring 最早行已翻到（上滑无可再补），重连/清屏归 false */
  backfillExhausted: boolean
  /** 本批已回补但尚未被 LogView 消费的行（takeBackfilled 取走即清空；
   *  重连不迁移；清屏清空） */
  backfillPending: LogLine[]
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
  /** 触发式现场捕获配置（RX 行命中规则/告警联动/断连 → ring 回溯转储档案；
   *  会话级，重连迁移；评估在后端读线程） */
  capture: CaptureCfg
  plot: PlotConfig
  /** 中心区视图模式（会话级偏好，重连迁移；clearLog 不清——视图偏好非数据） */
  centerView: CenterView
  followTail: boolean
  onlyMatches: boolean
  hexView: boolean
  showDelta: boolean
  showLineNo: boolean
  showDir: boolean
  /** 落盘录制开关（会话级，重连迁移）：false=暂停写日志文件（视图不受影响）；
   *  重新开启时另起新分段文件继续录制 */
  recOn: boolean
  rxBytes: number
  txBytes: number
  rxLines: number
  txLines: number
  jump: { no: number; token: number } | null
  /** 离线源文件元信息（Task 8 流式打开写入）：数据行数与首/末行 epoch 毫秒。
   *  live 会话恒 0；描述源文件本身（清屏/重连均不抹）。现状无 UI 消费者
   *  （旧全量链路 parseLogFile 的 total 直接丢弃），预留给状态栏/对比视图 */
  offlineLineCount: number
  offlineFirstEpoch: number
  offlineLastEpoch: number
}

/**
 * 会话字段全集（单一事实来源）：与 createSession 工厂的键一一对应。
 * 生命周期策略见 lifecycle.ts 的 FIELD_POLICY（每字段必有一策，测试穷举）。
 */
export const SESSION_FIELDS = [
  'id',
  'kind',
  'config',
  'status',
  'error',
  'lines',
  'lineCounter',
  'pulledThrough',
  'pullNo',
  'droppedLines',
  'ringDropped',
  'evictedPending',
  'reconnectNo',
  'backfillTotal',
  'backfillExhausted',
  'backfillPending',
  'bookmarks',
  'aiNotes',
  'decoded',
  'sendHistory',
  'search',
  'filters',
  'keywords',
  'autoReply',
  'alerts',
  'capture',
  'plot',
  'centerView',
  'followTail',
  'onlyMatches',
  'hexView',
  'showDelta',
  'showLineNo',
  'showDir',
  'recOn',
  'rxBytes',
  'txBytes',
  'rxLines',
  'txLines',
  'jump',
  'offlineLineCount',
  'offlineFirstEpoch',
  'offlineLastEpoch',
] as const

export type SessionField = (typeof SESSION_FIELDS)[number]

/** 会话默认值工厂（原 makeSession）：rules 数组各会话独立持有，不共享引用 */
export function createSession(id: string, config: PortConfig): Session {
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
    ringDropped: 0,
    evictedPending: 0,
    reconnectNo: 0,
    backfillTotal: 0,
    backfillExhausted: false,
    backfillPending: [],
    bookmarks: [],
    aiNotes: [],
    decoded: [],
    sendHistory: [],
    search: { ...DEFAULT_SEARCH },
    filters: [],
    keywords: [],
    autoReply: { enabled: false, rules: [] },
    alerts: { ...DEFAULT_ALERT_STATE, rules: [] },
    capture: makeCaptureCfg(),
    plot: { ...DEFAULT_PLOT_CONFIG },
    centerView: 'log',
    followTail: true,
    onlyMatches: false,
    hexView: false,
    showDelta: false,
    showLineNo: true,
    showDir: true,
    recOn: true,
    rxBytes: 0,
    txBytes: 0,
    rxLines: 0,
    txLines: 0,
    jump: null,
    offlineLineCount: 0,
    offlineFirstEpoch: 0,
    offlineLastEpoch: 0,
  }
}
