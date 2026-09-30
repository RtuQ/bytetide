import { describe, expect, it, beforeEach } from 'vitest'
import {
  AUTO_RETRY_STEPS_MS, autoReconnectStateForTest, backoffMs, blockAutoReconnect,
  consumeAutoReconnected, dropRetry, isAutoReconnectBlocked, isAutoReconnectCandidate,
  isSerialTransport, nextRetryWake, planRetry, rekeyRetry, retryDueAt, sweepBlocked,
  sweepRetry, unblockAutoReconnect, _resetAutoReconnectForTest,
} from '../autoReconnect'
import type { PortConfig, SessionKind, SessionStatus } from '../../types'

/** 构造候选判定所需的最小会话形状（CandidateSession） */
function sess(over: Partial<{ kind: SessionKind; status: SessionStatus; config: PortConfig }> = {}) {
  return {
    kind: 'live' as SessionKind,
    status: 'error' as SessionStatus,
    config: { name: 'COM3', autoReconnect: true } as PortConfig,
    ...over,
  }
}

beforeEach(() => _resetAutoReconnectForTest())

describe('isSerialTransport', () => {
  it('transport 缺省/null/serial 均为串口', () => {
    expect(isSerialTransport({ name: 'COM3' } as PortConfig)).toBe(true)
    expect(isSerialTransport({ name: 'COM3', transport: null } as PortConfig)).toBe(true)
    expect(isSerialTransport({ name: 'COM3', transport: 'serial' } as PortConfig)).toBe(true)
  })

  it('网络源不是串口', () => {
    expect(isSerialTransport({ name: 'h', transport: 'tcp-client' } as PortConfig)).toBe(false)
    expect(isSerialTransport({ name: 'h', transport: 'udp' } as PortConfig)).toBe(false)
  })
})

describe('isAutoReconnectCandidate', () => {
  it('live 串口 + 开关开 + error/disconnected 为候选', () => {
    expect(isAutoReconnectCandidate(sess())).toBe(true)
    expect(isAutoReconnectCandidate(sess({ status: 'disconnected' }))).toBe(true)
  })

  it('connecting/connected/offline 不算候选', () => {
    expect(isAutoReconnectCandidate(sess({ status: 'connecting' }))).toBe(false)
    expect(isAutoReconnectCandidate(sess({ status: 'connected' }))).toBe(false)
    expect(isAutoReconnectCandidate(sess({ status: 'offline' }))).toBe(false)
  })

  it('开关关 / 网络源 / 非 live 会话不算候选', () => {
    expect(isAutoReconnectCandidate(sess({ config: { name: 'COM3', autoReconnect: false } as PortConfig }))).toBe(false)
    expect(isAutoReconnectCandidate(sess({ config: { name: 'h', transport: 'tcp-client', autoReconnect: true } as PortConfig }))).toBe(false)
    expect(isAutoReconnectCandidate(sess({ kind: 'replay' }))).toBe(false)
    expect(isAutoReconnectCandidate(sess({ kind: 'offline' }))).toBe(false)
  })
})

describe('backoffMs', () => {
  it('按阶梯取值，走完封顶 30s', () => {
    expect(backoffMs(1)).toBe(AUTO_RETRY_STEPS_MS[0])
    expect(backoffMs(2)).toBe(AUTO_RETRY_STEPS_MS[1])
    expect(backoffMs(AUTO_RETRY_STEPS_MS.length)).toBe(30000)
    expect(backoffMs(AUTO_RETRY_STEPS_MS.length + 5)).toBe(30000)
  })

  it('非法入参（<1）钳到首档', () => {
    expect(backoffMs(0)).toBe(AUTO_RETRY_STEPS_MS[0])
  })
})

describe('退避账本', () => {
  it('planRetry 首次 attempts=1，之后递增且 nextAt 按阶梯拉开', () => {
    planRetry('s1', 1000)
    expect(autoReconnectStateForTest('s1')).toEqual({ attempts: 1, nextAt: 1000 + 1000, blocked: false })
    planRetry('s1', 2000)
    expect(autoReconnectStateForTest('s1')).toEqual({ attempts: 2, nextAt: 2000 + 2000, blocked: false })
  })

  it('retryDueAt：无账目 null，有账目为 nextAt', () => {
    expect(retryDueAt('s1')).toBeNull()
    planRetry('s1', 5000)
    expect(retryDueAt('s1')).toBe(5000 + AUTO_RETRY_STEPS_MS[0])
  })

  it('rekeyRetry 迁移账目保退避连续；无账目 no-op', () => {
    planRetry('s1', 1000)
    planRetry('s1', 2000)
    rekeyRetry('s1', 's2')
    expect(retryDueAt('s1')).toBeNull()
    expect(autoReconnectStateForTest('s2').attempts).toBe(2)
    rekeyRetry('s1', 's3') // 旧键已空，不误建账
    expect(retryDueAt('s3')).toBeNull()
  })

  it('consumeAutoReconnected：有账目取走返回 true，再取 false', () => {
    planRetry('s1', 1000)
    expect(consumeAutoReconnected('s1')).toBe(true)
    expect(consumeAutoReconnected('s1')).toBe(false)
    expect(retryDueAt('s1')).toBeNull()
  })

  it('dropRetry 同时清退避与停靠', () => {
    planRetry('s1', 1000)
    blockAutoReconnect('s1')
    dropRetry('s1')
    expect(retryDueAt('s1')).toBeNull()
    expect(isAutoReconnectBlocked('s1')).toBe(false)
  })
})

describe('用户停止停靠', () => {
  it('block 后 isAutoReconnectBlocked，unblock 解除', () => {
    blockAutoReconnect('s1')
    expect(isAutoReconnectBlocked('s1')).toBe(true)
    unblockAutoReconnect('s1')
    expect(isAutoReconnectBlocked('s1')).toBe(false)
  })

  it('停靠不影响退避账目（两者独立，清扫各走各的）', () => {
    planRetry('s1', 1000)
    blockAutoReconnect('s1')
    expect(autoReconnectStateForTest('s1')).toEqual({ attempts: 1, nextAt: 2000, blocked: true })
  })
})

describe('失义清扫', () => {
  it('sweepRetry 丢弃不在表会话的条目，保留在表的', () => {
    planRetry('s1', 1000)
    planRetry('s2', 1000)
    sweepRetry(new Set(['s1']))
    expect(retryDueAt('s1')).not.toBeNull()
    expect(retryDueAt('s2')).toBeNull()
  })

  it('sweepBlocked 丢弃不在表会话的停靠，保留在表的', () => {
    blockAutoReconnect('s1')
    blockAutoReconnect('s2')
    sweepBlocked(new Set(['s1']))
    expect(isAutoReconnectBlocked('s1')).toBe(true)
    expect(isAutoReconnectBlocked('s2')).toBe(false)
  })
})

describe('nextRetryWake', () => {
  it('空账目为 null；取最近的未来到期（已过期的忽略）', () => {
    expect(nextRetryWake(0)).toBeNull()
    planRetry('s1', 0) // nextAt = 1000
    planRetry('s2', 3000) // nextAt = 4000
    expect(nextRetryWake(500)).toBe(1000)
    expect(nextRetryWake(1500)).toBe(4000) // s1 已过期不再计时
    expect(nextRetryWake(5000)).toBeNull() // 全部到期，交给当下评估
  })
})
