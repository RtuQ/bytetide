import type { Session, SessionField } from './model'
import { createSession } from './model'

/**
 * 会话字段生命周期策略（逐字段必有一策，lifecycle.test.ts 穷举比对实现）：
 * - 'carry'         重连迁移（值带入新会话）+ 清屏保留——用户配置/偏好/终身计数
 * - 'resetReconnect' 重连重置为新会话默认（新 ring 纪元）+ 清屏同样归零——绑定
 *                    当前视图数据的游标/缺口/锚定状态
 * - 'resetClear'    重连迁移 + 清屏归零——行数据与锚定行号的状态（重连时日志行
 *                    本身被带走，缺口/书签/批注仍然有效；清屏后行号重计数则失义）
 * - 'runtime'       身份与传输运行态：重连=新会话新值（id 换新、status/error 从
 *                    默认起步、pullNo 随新 ring 归零），清屏一律不动（pullNo
 *                    单调不回退——后端 ring no 单调，清屏后旧 ringNo 不回灌）
 */
export const FIELD_POLICY: Record<SessionField, 'carry' | 'resetReconnect' | 'resetClear' | 'runtime'> = {
  // ---- runtime：身份 / 传输状态 / ring 游标（清屏不动） ----
  id: 'runtime',
  kind: 'runtime',
  status: 'runtime',
  error: 'runtime',
  pullNo: 'runtime',
  // ---- carry：重连迁移 + 清屏保留 ----
  config: 'carry',
  sendHistory: 'carry',
  search: 'carry',
  filters: 'carry',
  keywords: 'carry',
  autoReply: 'carry',
  alerts: 'carry',
  capture: 'carry',
  plot: 'carry',
  centerView: 'carry',
  followTail: 'carry',
  onlyMatches: 'carry',
  hexView: 'carry',
  showDelta: 'carry',
  showLineNo: 'carry',
  showDir: 'carry',
  recOn: 'carry',
  rxBytes: 'carry',
  txBytes: 'carry',
  rxLines: 'carry',
  txLines: 'carry',
  jump: 'carry',
  // ---- resetClear：重连迁移 + 清屏归零（绑定当前视图行数据） ----
  lines: 'resetClear',
  lineCounter: 'resetClear',
  droppedLines: 'resetClear',
  reconnectNo: 'resetClear',
  bookmarks: 'resetClear',
  aiNotes: 'resetClear',
  // ---- resetReconnect：重连重置（新 ring 纪元）+ 清屏归零 ----
  pulledThrough: 'resetReconnect',
  ringDropped: 'resetReconnect',
  evictedPending: 'resetReconnect',
  backfillTotal: 'resetReconnect',
  backfillExhausted: 'resetReconnect',
  backfillPending: 'resetReconnect',
  decoded: 'resetReconnect',
}

/**
 * 重连迁移（原 reconnectSession 的 carried 构造）：以新会话默认值为底，
 * 按 FIELD_POLICY 覆盖 carry/resetClear 字段。引用/拷贝语义与既有行为一致：
 * - 沿用原引用：lines/config/search/sendHistory/autoReply/capture/plot/jump
 * - 拷贝解耦：bookmarks（浅拷贝）/aiNotes/filters/keywords（逐元素浅拷贝）/
 *   alerts（外层重建+规则逐元素浅拷贝）
 * 特例：reconnectNo = 旧会话 lineCounter（新 ring 纪元下界）；kind 不迁移回落
 * 默认 'live'（重连仅 live 会话可达）。
 */
export function carrySessionForReconnect(previous: Session, newId: string): Session {
  return {
    ...createSession(newId, previous.config),
    lines: previous.lines,
    lineCounter: previous.lineCounter,
    droppedLines: previous.droppedLines,
    // 新 ring 纪元下界：迁移行携带旧 ring 的 rn，no <= 此值的行不可往前翻页回补
    reconnectNo: previous.lineCounter,
    bookmarks: [...previous.bookmarks],
    aiNotes: previous.aiNotes.map((n) => ({ ...n })),
    sendHistory: previous.sendHistory,
    search: previous.search,
    filters: previous.filters.map((f) => ({ ...f })),
    keywords: previous.keywords.map((k) => ({ ...k })),
    autoReply: previous.autoReply,
    alerts: { enabled: previous.alerts.enabled, rules: previous.alerts.rules.map((r) => ({ ...r })) },
    capture: previous.capture,
    plot: previous.plot,
    centerView: previous.centerView,
    followTail: previous.followTail,
    onlyMatches: previous.onlyMatches,
    hexView: previous.hexView,
    showDelta: previous.showDelta,
    showLineNo: previous.showLineNo,
    showDir: previous.showDir,
    recOn: previous.recOn,
    rxBytes: previous.rxBytes,
    txBytes: previous.txBytes,
    rxLines: previous.rxLines,
    txLines: previous.txLines,
    jump: previous.jump,
  }
}

/**
 * 清屏的字段纪律（原 clearLog 的本地字段重置部分，原地变更）：
 * resetClear/resetReconnect 字段按 createSession 默认值归零；carry/runtime 字段
 * 不动（pullNo 单调不回退、status/error/配置与视图偏好保留）。命令下发
 * （syncAnnotations/clearLog）与解析引擎复位钩子由门面编排，不在此处。
 */
export function clearSessionData(session: Session): void {
  session.lines = []
  session.lineCounter = 0
  session.pulledThrough = 0
  // 行号已从 1 重新计数，旧书签全部失义，一并清空
  session.bookmarks = []
  // 丢弃计数与"当前视图缺口"绑定：清屏后缓冲从头开始，旧缺口已无意义，
  // 归零避免让人误以为当前日志仍缺数据（重连迁移保留，因日志行本身被带走）
  session.droppedLines = 0
  // ring 覆盖缺口与锚定待补偿同理绑定“当前视图”，一并归零（pullNo 保持单调不重置）
  session.ringDropped = 0
  session.evictedPending = 0
  // 翻页补行状态同理绑定当前视图：ring 已清空无可回补，纪元下界与计数归零
  session.reconnectNo = 0
  session.backfillTotal = 0
  session.backfillExhausted = false
  session.backfillPending = []
  // AI 批注锚定行号，同样失义（后端镜像同步由门面负责）
  session.aiNotes = []
  // 解码帧同样锚定行号：清空并由解析引擎复位该会话切帧状态（gen+1，门面钩子）
  session.decoded = []
}
