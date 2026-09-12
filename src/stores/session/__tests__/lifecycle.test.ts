import { describe, it, expect } from 'vitest'
import { createSession, SESSION_FIELDS, type Session } from '../model'
import { FIELD_POLICY, carrySessionForReconnect, clearSessionData } from '../lifecycle'
import {
  DEFAULT_PLOT_CONFIG,
  makeCaptureCfg,
  type LogLine,
  type PortConfig,
} from '../../../types'

// 本地 makeConfig：串口默认值（不经 store，纯函数层测试）
function makeConfig(): PortConfig {
  return {
    transport: 'serial',
    name: 'COM-TEST',
    baudRate: 115200,
    dataBits: 8,
    parity: 'none',
    stopBits: '1',
    flowControl: 'none',
  }
}

function mkLine(no: number, rn?: number): LogLine {
  return { no, ts: '00:00:01.000', dir: 'rx', text: `l${no}`, bytes: null, epochMillis: no, rn }
}

/** 每个字段都填上与默认值不同的哨兵值，供重连/清屏逐键比对 */
function sentinelSession(): Session {
  const s = createSession('old', { ...makeConfig(), name: 'COM-OLD' })
  s.kind = 'offline' // 重连机制上不带 kind（回落默认 'live'）；离线会话实际不可达重连
  s.status = 'connected'
  s.error = 'boom'
  s.lines = [mkLine(1, 1), mkLine(2, 2)]
  s.lineCounter = 7
  s.pulledThrough = 5000
  s.pullNo = 42
  s.droppedLines = 3
  s.ringDropped = 9
  s.evictedPending = 4
  s.reconnectNo = 2
  s.backfillTotal = 11
  s.backfillExhausted = true
  s.backfillPending = [mkLine(9, 9)]
  s.bookmarks = [1, 3, 5]
  s.aiNotes = [{ id: 'n1', no: 2, ts: '00:00:02.000', text: 'ERR', note: 'note', at: 42 }]
  s.decoded = [
    {
      no: 1,
      ts: '00:00:01.000',
      dir: 'rx',
      type: '状态上报',
      text: '温度=25',
      fields: [],
      warn: null,
      frameHex: 'AA 55',
      frameLen: 2,
      crcOk: true,
    },
  ]
  s.sendHistory = ['A', 'B']
  s.search = { pattern: 'ERR', useRegex: true, caseSensitive: true, wholeWord: true }
  s.filters = [
    {
      id: 'f1',
      text: 'ERR',
      mode: 'exclude',
      dir: 'tx',
      useRegex: true,
      caseSensitive: true,
      wholeWord: true,
      enabled: false,
    },
  ]
  s.keywords = [
    { id: 'k1', pattern: 'ERR', color: '#ff5555', useRegex: true, caseSensitive: true, wholeWord: false },
  ]
  s.autoReply = {
    enabled: true,
    rules: [
      {
        id: 'r1',
        trigger: 'PING',
        reply: 'PONG',
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
        appendNewline: true,
        replyMode: 'hex',
        enabled: true,
      },
    ],
  }
  s.alerts = {
    enabled: true,
    rules: [
      {
        id: 'a1',
        pattern: 'Fault',
        useRegex: false,
        caseSensitive: false,
        wholeWord: false,
        minCount: 2,
        windowSec: 10,
        cooldownSec: 60,
        level: 'err',
        enabled: false,
      },
    ],
  }
  s.capture = { ...makeCaptureCfg(), enabled: true, preMs: 30000 }
  s.plot = { ...DEFAULT_PLOT_CONFIG, enabled: true, channels: 4 }
  s.centerView = 'split'
  s.followTail = false
  s.onlyMatches = true
  s.hexView = true
  s.showDelta = true
  s.showLineNo = false
  s.showDir = false
  s.recOn = false
  s.rxBytes = 100
  s.txBytes = 200
  s.rxLines = 10
  s.txLines = 20
  s.jump = { no: 5, token: 12345 }
  return s
}

describe('SESSION_FIELDS 与 createSession 工厂', () => {
  it('createSession 的键集合与 SESSION_FIELDS 深相等（排序比对，不重不漏）', () => {
    const s = createSession('s1', makeConfig())
    expect(Object.keys(s).sort()).toEqual([...SESSION_FIELDS].sort())
  })

  it('createSession 给出会话默认值（live/connecting/空数据/视图偏好）', () => {
    const s = createSession('s1', makeConfig())
    expect(s.id).toBe('s1')
    expect(s.kind).toBe('live')
    expect(s.status).toBe('connecting')
    expect(s.error).toBe('')
    expect(s.lines).toEqual([])
    expect(s.lineCounter).toBe(0)
    expect(s.pulledThrough).toBe(0)
    expect(s.pullNo).toBe(0)
    expect(s.droppedLines).toBe(0)
    expect(s.ringDropped).toBe(0)
    expect(s.evictedPending).toBe(0)
    expect(s.reconnectNo).toBe(0)
    expect(s.backfillTotal).toBe(0)
    expect(s.backfillExhausted).toBe(false)
    expect(s.backfillPending).toEqual([])
    expect(s.bookmarks).toEqual([])
    expect(s.aiNotes).toEqual([])
    expect(s.decoded).toEqual([])
    expect(s.sendHistory).toEqual([])
    expect(s.search).toEqual({ pattern: '', useRegex: false, caseSensitive: false, wholeWord: false })
    expect(s.filters).toEqual([])
    expect(s.keywords).toEqual([])
    expect(s.autoReply).toEqual({ enabled: false, rules: [] })
    expect(s.alerts).toEqual({ enabled: false, rules: [] })
    expect(s.capture).toEqual(makeCaptureCfg())
    expect(s.plot).toEqual(DEFAULT_PLOT_CONFIG)
    expect(s.centerView).toBe('log')
    expect(s.followTail).toBe(true)
    expect(s.onlyMatches).toBe(false)
    expect(s.hexView).toBe(false)
    expect(s.showDelta).toBe(false)
    expect(s.showLineNo).toBe(true)
    expect(s.showDir).toBe(true)
    expect(s.recOn).toBe(true)
    expect(s.rxBytes).toBe(0)
    expect(s.txBytes).toBe(0)
    expect(s.rxLines).toBe(0)
    expect(s.txLines).toBe(0)
    expect(s.jump).toBeNull()
  })

  it('工厂隔离性：rules 数组等嵌套引用各自独立，不共享常量', () => {
    const a = createSession('a', makeConfig())
    const b = createSession('b', makeConfig())
    expect(a.capture).not.toBe(b.capture)
    expect(a.capture.rules).not.toBe(b.capture.rules)
    expect(a.autoReply.rules).not.toBe(b.autoReply.rules)
    expect(a.alerts.rules).not.toBe(b.alerts.rules)
    expect(a.search).not.toBe(b.search)
    expect(a.plot).not.toBe(b.plot)
    expect(a.alerts).not.toBe(b.alerts)
    expect(a.autoReply).not.toBe(b.autoReply)
  })
})

describe('FIELD_POLICY 穷举', () => {
  it('每个会话字段都有策略：键集合与 SESSION_FIELDS 深相等', () => {
    expect(Object.keys(FIELD_POLICY).sort()).toEqual([...SESSION_FIELDS].sort())
  })

  it('每个策略值都落在四类之一', () => {
    const allowed = ['carry', 'resetReconnect', 'resetClear', 'runtime']
    for (const f of SESSION_FIELDS) {
      expect(allowed, `${String(f)} 的策略`).toContain(FIELD_POLICY[f])
    }
  })
})

describe('carrySessionForReconnect（重连迁移）', () => {
  it('哨兵会话重连后与显式期望快照逐键相等', () => {
    const prev = sentinelSession()
    const carried = carrySessionForReconnect(prev, 'new')
    const expected: Session = {
      id: 'new',
      // kind 不在迁移清单：回落 makeSession 默认 'live'（重连仅 live 会话可达）
      kind: 'live',
      config: prev.config,
      status: 'connecting',
      error: '',
      lines: prev.lines,
      lineCounter: 7,
      pulledThrough: 0,
      pullNo: 0,
      droppedLines: 3,
      ringDropped: 0,
      evictedPending: 0,
      // 新 ring 纪元下界 = 旧会话 lineCounter
      reconnectNo: 7,
      backfillTotal: 0,
      backfillExhausted: false,
      backfillPending: [],
      bookmarks: [1, 3, 5],
      aiNotes: [{ id: 'n1', no: 2, ts: '00:00:02.000', text: 'ERR', note: 'note', at: 42 }],
      decoded: [],
      sendHistory: ['A', 'B'],
      search: { pattern: 'ERR', useRegex: true, caseSensitive: true, wholeWord: true },
      filters: [
        {
          id: 'f1',
          text: 'ERR',
          mode: 'exclude',
          dir: 'tx',
          useRegex: true,
          caseSensitive: true,
          wholeWord: true,
          enabled: false,
        },
      ],
      keywords: [
        { id: 'k1', pattern: 'ERR', color: '#ff5555', useRegex: true, caseSensitive: true, wholeWord: false },
      ],
      autoReply: {
        enabled: true,
        rules: [
          {
            id: 'r1',
            trigger: 'PING',
            reply: 'PONG',
            useRegex: false,
            caseSensitive: false,
            wholeWord: false,
            appendNewline: true,
            replyMode: 'hex',
            enabled: true,
          },
        ],
      },
      alerts: {
        enabled: true,
        rules: [
          {
            id: 'a1',
            pattern: 'Fault',
            useRegex: false,
            caseSensitive: false,
            wholeWord: false,
            minCount: 2,
            windowSec: 10,
            cooldownSec: 60,
            level: 'err',
            enabled: false,
          },
        ],
      },
      capture: { ...makeCaptureCfg(), enabled: true, preMs: 30000 },
      plot: { ...DEFAULT_PLOT_CONFIG, enabled: true, channels: 4 },
      centerView: 'split',
      followTail: false,
      onlyMatches: true,
      hexView: true,
      showDelta: true,
      showLineNo: false,
      showDir: false,
      recOn: false,
      rxBytes: 100,
      txBytes: 200,
      rxLines: 10,
      txLines: 20,
      jump: { no: 5, token: 12345 },
    }
    expect(carried).toEqual(expected)
  })

  it('迁移行为与 FIELD_POLICY 逐字段一致：carry/resetClear 带值，其余回落默认', () => {
    const prev = sentinelSession()
    const carried = carrySessionForReconnect(prev, 'new')
    const fresh = createSession('new', makeConfig())
    for (const f of SESSION_FIELDS) {
      const policy = FIELD_POLICY[f]
      if (f === 'reconnectNo') {
        // resetClear 字段的特例：重连时取旧会话 lineCounter 作新纪元下界
        expect(carried.reconnectNo, String(f)).toBe(prev.lineCounter)
        continue
      }
      if (policy === 'carry' || policy === 'resetClear') {
        expect(carried[f], String(f)).toEqual(prev[f])
      } else {
        expect(carried[f], String(f)).toEqual(fresh[f])
      }
    }
  })

  it('引用语义：lines/config/search/sendHistory/autoReply/capture/plot/jump 沿用原引用', () => {
    const prev = sentinelSession()
    const carried = carrySessionForReconnect(prev, 'new')
    expect(carried.lines).toBe(prev.lines)
    expect(carried.config).toBe(prev.config)
    expect(carried.search).toBe(prev.search)
    expect(carried.sendHistory).toBe(prev.sendHistory)
    expect(carried.autoReply).toBe(prev.autoReply)
    expect(carried.capture).toBe(prev.capture)
    expect(carried.plot).toBe(prev.plot)
    expect(carried.jump).toBe(prev.jump)
  })

  it('拷贝语义：bookmarks/aiNotes/filters/keywords/alerts 复制后与原会话解耦', () => {
    const prev = sentinelSession()
    const carried = carrySessionForReconnect(prev, 'new')
    expect(carried.bookmarks).not.toBe(prev.bookmarks)
    expect(carried.aiNotes).not.toBe(prev.aiNotes)
    expect(carried.aiNotes[0]).not.toBe(prev.aiNotes[0])
    expect(carried.filters[0]).not.toBe(prev.filters[0])
    expect(carried.keywords[0]).not.toBe(prev.keywords[0])
    expect(carried.alerts).not.toBe(prev.alerts)
    expect(carried.alerts.rules[0]).not.toBe(prev.alerts.rules[0])
    // 改旧会话不影响新会话（迁移快照语义）
    prev.bookmarks.push(99)
    prev.aiNotes.push({ id: 'n2', no: 3, ts: '', text: '', note: '', at: 0 })
    prev.alerts.rules.push(prev.alerts.rules[0]!)
    expect(carried.bookmarks).toEqual([1, 3, 5])
    expect(carried.aiNotes).toHaveLength(1)
    expect(carried.alerts.rules).toHaveLength(1)
  })
})

describe('clearSessionData（清屏纪律）', () => {
  it('哨兵会话清屏后：数据/游标/锚定状态归零，配置与偏好保留', () => {
    const s = sentinelSession()
    const before: Session = JSON.parse(JSON.stringify(s))
    clearSessionData(s)
    const expected: Session = {
      ...before,
      lines: [],
      lineCounter: 0,
      pulledThrough: 0,
      bookmarks: [],
      droppedLines: 0,
      ringDropped: 0,
      evictedPending: 0,
      reconnectNo: 0,
      backfillTotal: 0,
      backfillExhausted: false,
      backfillPending: [],
      aiNotes: [],
      decoded: [],
      // 保留（carry / runtime）：游标单调不回退、状态与偏好非数据
      pullNo: 42,
      status: 'connected',
      error: 'boom',
      sendHistory: ['A', 'B'],
      search: before.search,
      filters: before.filters,
      keywords: before.keywords,
      autoReply: before.autoReply,
      alerts: before.alerts,
      capture: before.capture,
      plot: before.plot,
      centerView: 'split',
      followTail: false,
      onlyMatches: true,
      hexView: true,
      showDelta: true,
      showLineNo: false,
      showDir: false,
      recOn: false,
      rxBytes: 100,
      txBytes: 200,
      rxLines: 10,
      txLines: 20,
      jump: { no: 5, token: 12345 },
      kind: 'offline',
      config: before.config,
    }
    expect(s).toEqual(expected)
  })

  it('清屏行为与 FIELD_POLICY 逐字段一致：resetClear/resetReconnect 归零，其余保留', () => {
    const s = sentinelSession()
    const before = { ...s }
    clearSessionData(s)
    const fresh = createSession('fresh', makeConfig())
    for (const f of SESSION_FIELDS) {
      const policy = FIELD_POLICY[f]
      if (policy === 'resetClear' || policy === 'resetReconnect') {
        expect(s[f], String(f)).toEqual(fresh[f])
      } else {
        expect(s[f], String(f)).toEqual(before[f])
      }
    }
  })

  it('清屏是原地变更（返回 void），调用方持有的同一会话对象直接生效', () => {
    const s = sentinelSession()
    const same = s
    clearSessionData(s)
    expect(same).toBe(s)
    expect(same.lines).toEqual([])
    expect(same.bookmarks).toEqual([])
  })
})
