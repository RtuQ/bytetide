/**
 * 串口自动重连的纯控制模块（候选判定 + 退避账本 + 用户停止停靠，零 store 依赖）。
 *
 * 触发判定是「惰性评估」而非事件记账：任何唤醒点（端口列表变化 / 会话状态
 * 迁移 / 退避定时器 / 开关切换）由 useTauriEvents 的 evalAutoReconnect 扫描
 * 会话表，凡候选且未被停靠、退避已到、端口在场即发起重连。需要跨唤醒点
 * 记住的只有两样：
 * - **退避账本**（键=会话 id）：重连会换新 id，调用方在 reconnectSession
 *   返回新 id 后 rekeyRetry 迁移账目保退避连续；连接成功由
 *   consumeAutoReconnected 取走（返回值供 toast 区分「已自动重连」）。
 * - **用户停止停靠**（blocked）：意外断开与用户主动停止的 status 终值无法
 *   区分（EOF 型断开都是 disconnected），靠「谁先声明」划界——stopSession
 *   在下发 disconnect **之前**停靠（断开状态事件会在 await 期间到达），
 *   之后的状态事件不再误触发；重开开关（setAutoReconnect(true)）= 改主意，
 *   解除停靠。重连产生的新 id 天然无停靠（新会话新语义）。
 *
 * 时间基：Date.now() 墙钟（退避是秒级语义，与 alert 冷却同口径）。
 */

import type { PortConfig, SessionKind, SessionStatus } from '../types'

/** 退避阶梯：第 N 次尝试失败后，第 N+1 次至少间隔 STEPS[N-1]；走完封顶 30s */
export const AUTO_RETRY_STEPS_MS = [1000, 2000, 4000, 8000, 15000, 30000]

/** isAutoReconnectCandidate 的最小结构（Session 结构满足；避免反向依赖 store） */
export interface CandidateSession {
  kind: SessionKind
  status: SessionStatus
  config: PortConfig
}

/** 网络源无 port-changed 触发源，V1 只做串口（transport 缺省即串口） */
export function isSerialTransport(c: PortConfig): boolean {
  return c.transport === null || c.transport === undefined || c.transport === 'serial'
}

/** 自动重连候选：live 串口会话 + 开关开 + 断开/出错态（connecting/connected 不算） */
export function isAutoReconnectCandidate(s: CandidateSession): boolean {
  return (
    s.kind === 'live' &&
    s.config.autoReconnect === true &&
    isSerialTransport(s.config) &&
    (s.status === 'error' || s.status === 'disconnected')
  )
}

/** 第 attempts 次尝试后的下次最小间隔（attempts≥1；超出阶梯封顶） */
export function backoffMs(attempts: number): number {
  const i = Math.min(Math.max(attempts, 1), AUTO_RETRY_STEPS_MS.length) - 1
  return AUTO_RETRY_STEPS_MS[i]!
}

export interface RetryEntry {
  /** 已计划的尝试次数（第 1 次失败后按 backoffMs(1)=1s 排下次） */
  attempts: number
  /** 下次允许尝试的墙钟时刻（Date.now()） */
  nextAt: number
}

const retry = new Map<string, RetryEntry>()
const blocked = new Set<string>()

/** 发起重连前记账：attempts+1 并拉开下次间隔——无论本次成败，失败路径的
 *  下次重试已被排定；成功路径由 consume/rekey 收尾 */
export function planRetry(id: string, now: number): void {
  const attempts = (retry.get(id)?.attempts ?? 0) + 1
  retry.set(id, { attempts, nextAt: now + backoffMs(attempts) })
}

/** reconnectSession 成功换 id 后迁移账目（退避连续）；无账目时 no-op */
export function rekeyRetry(oldId: string, newId: string): void {
  const e = retry.get(oldId)
  if (!e) return
  retry.delete(oldId)
  retry.set(newId, e)
}

/** 连接成功时取走账目：true=这次连接来自自动重连（toast 文案分流） */
export function consumeAutoReconnected(id: string): boolean {
  return retry.delete(id)
}

/** 用户主动停止：停靠该会话的自动重连（须在下发 disconnect 前调用） */
export function blockAutoReconnect(id: string): void {
  blocked.add(id)
}

/** 解除停靠：重开开关=改主意（setAutoReconnect(true) / 评估器顺手清扫） */
export function unblockAutoReconnect(id: string): void {
  blocked.delete(id)
}

export function isAutoReconnectBlocked(id: string): boolean {
  return blocked.has(id)
}

/** 丢弃该会话的全部自动重连状态（关开关 / 会话移除时） */
export function dropRetry(id: string): void {
  retry.delete(id)
  blocked.delete(id)
}

/** 该会话下次允许尝试的时刻；无账目 = null（首次尝试可立即，由调用方判定） */
export function retryDueAt(id: string): number | null {
  return retry.get(id)?.nextAt ?? null
}

/** 清扫失义退避条目：会话已从表中移除（关闭标签页 / 重连换 id 后的旧键）。
 *  在表会话的条目不在此清——connected 由 consume 取走、error 回到候选继续用 */
export function sweepRetry(existingIds: ReadonlySet<string>): void {
  for (const id of [...retry.keys()]) if (!existingIds.has(id)) retry.delete(id)
}

/** 清扫失义停靠：会话已移除（在表会话的停靠持续有效，直到解除或换 id） */
export function sweepBlocked(existingIds: ReadonlySet<string>): void {
  for (const id of [...blocked]) if (!existingIds.has(id)) blocked.delete(id)
}

/** 账本里最近的到期时刻（无账目=null）；供定时链排下一跳 */
export function nextRetryWake(now: number): number | null {
  let wake: number | null = null
  for (const e of retry.values()) {
    if (e.nextAt > now && (wake === null || e.nextAt < wake)) wake = e.nextAt
  }
  return wake
}

/** 测试探针（勿在生产代码消费）：会话的自动重连状态快照 */
export function autoReconnectStateForTest(id: string): {
  attempts: number
  nextAt: number
  blocked: boolean
} {
  const e = retry.get(id)
  return { attempts: e?.attempts ?? 0, nextAt: e?.nextAt ?? 0, blocked: blocked.has(id) }
}

/** 每个测试用例前复位模块级账本（单例状态，参考 _resetPortWatchForTest） */
export function _resetAutoReconnectForTest(): void {
  retry.clear()
  blocked.clear()
}
