export type Dir = 'rx' | 'tx'
export type SessionStatus = 'disconnected' | 'connecting' | 'connected' | 'error' | 'offline'
/** 会话来源：live=实时串口；offline=从日志文件离线载入（无后端连接） */
export type SessionKind = 'live' | 'offline'

export interface PortConfig {
  name: string
  baudRate: number
  dataBits: number
  parity: string
  stopBits: string
  flowControl: string
  /** 数据源类型：缺省 / 'serial'=串口；其余为网络源（与后端 Option 字段对应，可空） */
  transport?: 'serial' | 'tcp-client' | 'tcp-server' | 'udp' | null
  /** tcp-client 目标主机；tcp-server/udp 监听地址（空则 0.0.0.0） */
  tcpHost?: string | null
  /** tcp-client 目标端口 / tcp-server 监听端口 */
  tcpPort?: number | null
  /** udp 本地监听端口 */
  udpLocalPort?: number | null
}

export interface PortInfo {
  name: string
  portType: string
  vendor?: string | null
  product?: string | null
  serial?: string | null
}

/** 后端推送的原始日志行（行号由前端分配） */
export interface RawLogLine {
  ts: string
  dir: Dir
  text: string
  /** 仅当该行含无效 UTF-8 字节时后端携带原始字节；否则前端用 TextEncoder().encode(text) 恢复 */
  bytes?: number[] | null
  epochMillis: number
}

export interface LogLine extends RawLogLine {
  no: number
}

/** 搜索状态：仅驱动“只看命中 / 次数 / 命中行列表”（与高亮关键词分离） */
export interface SearchState {
  pattern: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
}

/** 高亮关键词：独立设置，多个，每个自带颜色 */
export interface Keyword {
  id: string
  pattern: string
  color: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
}

/** 自动回复规则：收到匹配命令即按规则回复 */
export interface AutoReplyRule {
  id: string
  trigger: string
  reply: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
  appendNewline: boolean
  replyMode: 'ascii' | 'hex'
  enabled: boolean
}

/** 自动回复状态：总开关 + 规则列表 */
export interface AutoReplyState {
  enabled: boolean
  rules: AutoReplyRule[]
}

export interface LogPayload {
  sessionId: string
  lines: RawLogLine[]
}

/** 离线日志解析结果：成功行 + 统计 */
export interface ParsedLog {
  lines: RawLogLine[]
  total: number
  errors: number
}

export interface StatusPayload {
  sessionId: string
  status: string
}

export interface ErrorPayload {
  sessionId: string
  error: string
}

export const DEFAULT_SEARCH: SearchState = {
  pattern: '',
  useRegex: false,
  caseSensitive: false,
  wholeWord: false,
}

/** 搜索命中使用的高亮色（与关键词色区分） */
export const SEARCH_COLOR = '#0a84ff'

/** 新增关键词时循环取用的调色板 */
export const KEYWORD_PALETTE = [
  '#ff79c6',
  '#8be9fd',
  '#bd93f9',
  '#50fa7b',
  '#ffb86c',
  '#ff5555',
  '#f1fa8c',
  '#f8f8f2',
]

/** 日志路径模板与每行时间戳格式配置（全局，连接/重连时生效） */
export interface LogConfig {
  logPathTemplate: string
  lineTsFormat: string
}

export const DEFAULT_LOG_CONFIG: LogConfig = {
  logPathTemplate: '',
  lineTsFormat: '%h:%m:%s.%t',
}

/** 连接配置预设：命名保存的 PortConfig，可一键回填到端口栏 */
export interface PortPreset {
  id: string
  name: string
  config: PortConfig
}

/** 绘图数据源：原始字节 / ASCII hex 文本 */
export type PlotSource = 'binary' | 'ascii-hex'
/** 帧校验方式：无 / 累加和(1B) / XOR(1B) */
export type PlotChecksum = 'none' | 'sum' | 'xor'
/** 端序 */
export type PlotEndian = 'big' | 'little'
/** 每通道字节数 */
export type PlotBytes = 1 | 2 | 4

/** 绘图配置：帧头/帧尾/校验/通道/字节/端序，按帧解析为多通道点 */
export interface PlotConfig {
  enabled: boolean
  source: PlotSource
  frameHead: string
  frameTail: string
  checksum: PlotChecksum
  channels: number
  bytesPerChannel: PlotBytes
  endian: PlotEndian
  signed: boolean
  maxPoints: number
}

/** 一个解析出的数据点：多通道值 + 所在 RX 行的时间/原始帧 */
export interface PlotPoint {
  /** 1-based 点序号（用于 X 轴刻度与提示） */
  idx: number
  /** 各通道十进制值 */
  values: number[]
  /** 帧所在 RX 行的墙钟毫秒 */
  epochMillis: number
  /** 帧所在 RX 行的时间戳串 */
  ts: string
  /** 帧原始字节 hex（空格分隔大写），用于提示展示 */
  rawHex: string
}

export const DEFAULT_PLOT_CONFIG: PlotConfig = {
  enabled: false,
  source: 'binary',
  frameHead: '',
  frameTail: '',
  checksum: 'none',
  channels: 2,
  bytesPerChannel: 2,
  endian: 'big',
  signed: false,
  maxPoints: 2000,
}

// ===================== REST 分析桥（与后端 bridge.rs camelCase 对齐） =====================

/** REST 桥配置（持久化于 bridge.json） */
export interface BridgeConfig {
  enabled: boolean
  /** 绑定地址：127.0.0.1=仅本机；0.0.0.0=远程/虚拟机可达 */
  bind: string
  port: number
  /** Bearer 令牌；空串=未设置（启用时后端自动生成） */
  token: string
  /** 是否允许 AI 发送/交换（默认关；关闭时 /send、/exchange 返 403） */
  allowSend: boolean
}

export const DEFAULT_BRIDGE_CONFIG: BridgeConfig = {
  enabled: false,
  bind: '127.0.0.1',
  port: 8765,
  token: '',
  allowSend: false,
}

/** 桥单行：no 为后端独立序号（与前端 lineCounter 无关，环形淘汰后继续递增） */
export interface BridgeMatchHit {
  offset: number
  length: number
  field: 'text' | 'bytes'
}

export interface BridgeLine {
  no: number
  ts: string
  dir: Dir
  text: string
  bytes?: number[] | null
  epochMillis: number
  match?: BridgeMatchHit | null
}

export interface BridgeSessionSnap {
  id: string
  config: PortConfig
  status: string
  lineCount: number
  ringCap: number
}

export interface BridgeStats {
  rxLines: number
  txLines: number
  rxBytes: number
  txBytes: number
  firstNo: number
  lastNo: number
  firstTs: string
  lastTs: string
  firstEpoch: number
  lastEpoch: number
  ringCap: number
  size: number
}

export interface LinesPage {
  lines: BridgeLine[]
  total: number
  firstNo: number
  lastNo: number
  size: number
  truncated: boolean
}

export interface FollowPage {
  lines: BridgeLine[]
  lastNo: number
  timedOut: boolean
}

export interface HistBucket {
  bucketStart: number
  count: number
}

export interface TimingGap {
  fromNo: number
  toNo: number
  durationMs: number
  fromTs: string
  toTs: string
}

export interface TimingPage {
  count: number
  minGap: number
  maxGap: number
  avgGap: number
  p95Gap: number
  gaps: TimingGap[]
}

export interface DecodeFrame {
  no: number
  idx: number
  values: number[]
  rawHex: string
  ts: string
  epochMillis: number
  valid: boolean
  error?: string | null
}

export interface DecodePage {
  frames: DecodeFrame[]
  frameCount: number
  lastError: string
  scanned: number
}

export interface ValueDist {
  value: number
  count: number
}

export interface ValueHistPage {
  channel: number
  samples: number
  distinct: number
  min: number
  max: number
  mean: number
  distribution: ValueDist[]
}

export interface HexCount {
  hex: string
  count: number
}

export interface ChecksumCount {
  kind: 'sum' | 'xor'
  count: number
}

export interface InferPage {
  heads: HexCount[]
  tails: HexCount[]
  checksums: ChecksumCount[]
  suggestedFrameLen?: number | null
}

export interface ExchangePage {
  sent: boolean
  response: BridgeLine | null
  waitedMs: number
}

// ===================== 告警 =====================

export type AlertLevel = 'info' | 'warn' | 'err'

/** 告警规则：RX 行命中 pattern，满足窗口/冷却条件时触发系统通知 + 历史 */
export interface AlertRule {
  id: string
  pattern: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
  /** 触发阈值：窗口内命中次数（1=即中即报） */
  minCount: number
  /** 计数窗口秒数（0=不聚合，单行即计） */
  windowSec: number
  /** 触发后冷却秒数（防刷屏；0=不冷却） */
  cooldownSec: number
  level: AlertLevel
  enabled: boolean
}

/** 会话级告警状态（镜像 autoReply 形状） */
export interface AlertState {
  enabled: boolean
  rules: AlertRule[]
}

export const DEFAULT_ALERT_STATE: AlertState = { enabled: false, rules: [] }

/** 一条告警历史（内存环形，最多 100 条，不入库不持久化） */
export interface AlertHit {
  id: string
  sessionId: string
  sessionName: string
  ruleId: string
  pattern: string
  level: AlertLevel
  no: number
  ts: string
  text: string
  at: number
}

/** 一条 AI 批注（REST 桥写入，事件实时同步到界面；no 为行号） */
export interface AiAnnotation {
  id: string
  no: number
  ts: string
  text: string
  note: string
  at: number
}

// ===================== 过滤链 =====================

export type FilterMode = 'include' | 'exclude'
export type FilterDir = 'any' | 'rx' | 'tx'

/**
 * 过滤链单级：依序对行集做包含/排除（集合语义下与顺序无关）。
 * 与“搜索”独立——搜索负责命中高亮/统计，过滤链只决定哪些行参与显示。
 */
export interface FilterStage {
  id: string
  text: string
  mode: FilterMode
  dir: FilterDir
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
  enabled: boolean
}

// ===================== 配置预设库 =====================

export type PresetCategory = 'filters' | 'keywords' | 'autoReply' | 'plots'

/** 命名预设：payload 形状随类别不同（见 session.ts 存取处校验） */
export interface ConfigPreset {
  id: string
  name: string
  category: PresetCategory
  createdAt: number
  /** 类别对应的数据：filters=FilterStage[] / keywords=Keyword[] / autoReply=AlertState 同构的 AutoReplyState / plots=PlotConfig */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  data: any
}

