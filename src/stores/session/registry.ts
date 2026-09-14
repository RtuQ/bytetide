import type { Session } from './model'
import type { ReplayState } from '../../types'

/**
 * 会话注册表（Task 6）：`Record<string, Session>` 规范持有、唯一真相。
 * 门面 Pinia store 的 state 与此形状同构（同一响应式对象），本模块是
 * 注册/移除/活动态的唯一写入口——其他模块不得直接改 sessions/order/activeId。
 * 函数均显式传入注册表状态，不内部调用 useSessionStore。
 */
export interface RegistryState {
  sessions: Record<string, Session>
  order: string[]
  activeId: string | null
}

export function getSession(st: RegistryState, id: string): Session | undefined {
  return st.sessions[id]
}

export function getActive(st: RegistryState): Session | null {
  if (!st.activeId) return null
  return st.sessions[st.activeId] ?? null
}

/** tab 序驱动（order 决定标签顺序，sessions 表只管账）；容错跳过孤儿 id */
export function listSessions(st: RegistryState): Session[] {
  const list: Session[] = []
  for (const id of st.order) {
    const s = st.sessions[id]
    if (s) list.push(s)
  }
  return list
}

/** 落账新会话：入表 + 追加 tab 序 + 置为活动（openTab/createLocalSession/loadOfflineSession 共用） */
export function registerSession(st: RegistryState, s: Session): void {
  st.sessions[s.id] = s
  st.order.push(s.id)
  st.activeId = s.id
}

/** 关闭移除：摘除 tab；若移除的是活动 tab 则落到最后一个（无则 null） */
export function removeSession(st: RegistryState, id: string): void {
  delete st.sessions[id]
  st.order = st.order.filter((x) => x !== id)
  if (st.activeId === id) {
    st.activeId = st.order[st.order.length - 1] ?? null
  }
}

/** 重连换账：旧 id 出表、新 id 入表，tab 序原位替换，活动态跟随新 id */
export function replaceSession(st: RegistryState, oldId: string, s: Session): void {
  delete st.sessions[oldId]
  st.sessions[s.id] = s
  st.order = st.order.map((x) => (x === oldId ? s.id : x))
  if (st.activeId === oldId) st.activeId = s.id
}

// ===================== 建账竞态缓冲（落账域） =====================

/** 状态事件先于会话落账到达时暂存（flushPendingTo 落账后回放） */
const pendingStatus = new Map<string, string>()
const pendingError = new Map<string, string>()
const PENDING_CAP = 64

/** 记录连接状态：未落账先暂存（容量上限防泄漏），已落账且值合法才写入 */
export function recordStatus(st: RegistryState, id: string, status: string): void {
  const s = st.sessions[id]
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
}

/** 记录错误：未落账先暂存；已落账写 error 且非空错误连带置 error 态 */
export function recordError(st: RegistryState, id: string, error: string): void {
  const s = st.sessions[id]
  if (!s) {
    if (pendingError.size < PENDING_CAP) pendingError.set(id, error)
    return
  }
  s.error = error
  if (error) s.status = 'error'
}

/** 会话落账后回放竞态期间积压的连接状态/错误（先状态后错误，error 优先） */
export function flushPendingTo(st: RegistryState, id: string): void {
  const st2 = pendingStatus.get(id)
  if (st2 !== undefined) {
    pendingStatus.delete(id)
    recordStatus(st, id, st2)
  }
  const err = pendingError.get(id)
  if (err !== undefined) {
    pendingError.delete(id)
    recordError(st, id, err)
  }
}

/** 重连换账后旧 id 的积压事件已无意义：一并丢弃 */
export function dropPending(id: string): void {
  pendingStatus.delete(id)
  pendingError.delete(id)
}

/** 回放控制面视图落账（Stage 3 Task 7）：replay-state 事件与 replayStatus 轮询
 *  共用一写入点。仅 replay 会话生效；未知/已移除/非回放会话的迟到事件忽略 */
export function applyReplayView(
  st: RegistryState,
  id: string,
  view: { state: ReplayState; speed: number; looped: boolean; line: number },
): void {
  const s = st.sessions[id]
  if (!s || s.kind !== 'replay') return
  s.replay = { state: view.state, speed: view.speed, looped: view.looped, line: view.line }
}
