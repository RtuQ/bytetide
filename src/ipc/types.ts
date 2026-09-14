// 跨 IPC 边界的 DTO 契约（camelCase 对齐 Rust serde rename_all）。
// 领域类型一律从 src/types/index.ts 再导出（单一事实来源，禁止重复定义）；
// 仅跨边界新形状（PulledLine/RingBounds/BridgeBookmark/BridgeAlert/补丁等）在此新建。
export type {
  AiAnnotation,
  AlertState,
  AutoReplyState,
  BridgeConfig,
  BridgeRuntime,
  BridgeView,
  CaptureCfg,
  CaptureMeta,
  Dir,
  ErrorPayload,
  LogConfig,
  PlotConfig,
  PortConfig,
  PortInfo,
  RawLogLine,
  ReplayState,
  StatusPayload,
} from '../types'

import type {
  AlertState,
  AutoReplyState,
  BridgeConfig,
  CaptureCfg,
  Dir,
  ReplayState,
} from '../types'

/** 拉模型游标拉取的单行（ring_lines_no_cmd / ring_lines_before_cmd 返回，Rust BridgeLine）。
 *  `no` 是 ring 游标（前端 appendPulled 摄取时映射为 UI 行号 no + ring 行号 rn）。 */
export interface PulledLine {
  no: number
  ts: string
  dir: Dir
  text: string
  /** 仅当该行含无效 UTF-8 字节时后端携带原始字节；否则为 null */
  bytes: number[] | null
  epochMillis: number
}

/** ring 现存行号边界（ring_bounds_cmd）：前端「翻页补旧行」判断还能不能往前翻 */
export interface RingBounds {
  firstNo: number
  lastNo: number
  size: number
  ringCap: number
}

/** 流式打开离线日志（Task 8）的返回：core 一次顺序扫描建稀疏索引建会话（ring 恒空），
 *  行经 offlineLinesAfter 分页拉取（no=文件内第 N 数据行，1 起连续）；epoch 为毫秒 */
export interface OfflineOpenResult {
  sessionId: string
  lineCount: number
  firstEpoch: number
  lastEpoch: number
}

/** 打开时序回放会话（Stage 3 Task 7）的返回：durationMs=源文件首末行 epoch 差
 *  （当日毫秒口径，跨午夜日志低估——与回放调度同源同偏差，仅作时长展示） */
export interface ReplayOpenResult {
  sessionId: string
  lineCount: number
  durationMs: number
}

/** 回放控制面视图（replay_control/replay_status 返回与 replay-state 事件共同载荷）：
 *  line=当前文件行号水位（最后已入库源文件行；seek 后未恢复=目标-1） */
export interface ReplayView {
  sessionId: string
  state: ReplayState
  speed: number
  looped: boolean
  line: number
}

/** replay_control_cmd 的 action（value：seek=行号、speed=倍速、loop=1/0；
 *  pause/resume/stop 不带值） */
export type ReplayAction = 'pause' | 'resume' | 'seek' | 'speed' | 'loop' | 'stop'

/** 后端 BridgeBookmark 镜像（camelCase）：行被淘汰后 text 为空、只保留行号 */
export interface BridgeBookmark {
  no: number
  ts: string
  text: string
}

/** 后端 BridgeAlert 镜像（camelCase；sessionName 冗余不推） */
export interface BridgeAlert {
  id: string
  ruleId: string
  pattern: string
  level: string
  no: number
  ts: string
  text: string
  at: number
}

/** bridge_set_config_cmd 的补丁（Rust BridgeConfigPatch）；confirmRemote 为远程绑定一次性确认 */
export type BridgeConfigPatch = Partial<BridgeConfig> & { confirmRemote?: boolean }

/** set_live_rules_cmd 的实时规则整包（拉模型：评估在后端读线程） */
export interface LiveRulesPayload {
  autoReply: AutoReplyState
  alerts: AlertState
  capture: CaptureCfg
}

/** append_perf_diag_cmd 的单条前端诊断（取证旁路，失败静默） */
export interface PerfDiagnostic {
  kind: string
  sessionId: string
  lagMs: number
  batchMs: number
  lines: number
  vis: string
}

/** send_cmd 的发送模式（Rust SendMode：hex→Hex，其余→Ascii） */
export type SendMode = 'ascii' | 'hex'
