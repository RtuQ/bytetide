import { describe, it, expect } from 'vitest'
import { createSession } from '../model'
import {
  getSession,
  getActive,
  listSessions,
  registerSession,
  removeSession,
  replaceSession,
  recordStatus,
  recordError,
  flushPendingTo,
  dropPending,
  type RegistryState,
} from '../registry'
import type { PortConfig } from '../../../types'

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

function makeRegistry(): RegistryState {
  return { sessions: {}, order: [], activeId: null }
}

describe('registry 会话注册表（唯一真相）', () => {
  it('registerSession：入表 + 追加 tab 序 + 置为活动', () => {
    const st = makeRegistry()
    const a = createSession('s1', makeConfig())
    const b = createSession('s2', makeConfig())
    registerSession(st, a)
    registerSession(st, b)
    expect(Object.keys(st.sessions)).toEqual(['s1', 's2'])
    expect(st.order).toEqual(['s1', 's2'])
    expect(st.activeId).toBe('s2')
  })

  it('getSession / getActive / listSessions：listSessions 按 order 序输出', () => {
    const st = makeRegistry()
    registerSession(st, createSession('s1', makeConfig()))
    registerSession(st, createSession('s2', makeConfig()))
    expect(getSession(st, 's1')?.id).toBe('s1')
    expect(getSession(st, 'nope')).toBeUndefined()
    expect(getActive(st)?.id).toBe('s2')
    expect(listSessions(st).map((s) => s.id)).toEqual(['s1', 's2'])
  })

  it('空表：getActive 返回 null、listSessions 返回空', () => {
    const st = makeRegistry()
    expect(getActive(st)).toBeNull()
    expect(listSessions(st)).toEqual([])
  })

  it('removeSession：摘除会话与 tab 序；移除活动 tab 回退到最后一个', () => {
    const st = makeRegistry()
    registerSession(st, createSession('s1', makeConfig()))
    registerSession(st, createSession('s2', makeConfig()))
    registerSession(st, createSession('s3', makeConfig()))
    removeSession(st, 's3')
    expect(st.sessions['s3']).toBeUndefined()
    expect(st.order).toEqual(['s1', 's2'])
    expect(st.activeId).toBe('s2')
  })

  it('removeSession：移除非活动 tab 不改变 activeId', () => {
    const st = makeRegistry()
    registerSession(st, createSession('s1', makeConfig()))
    registerSession(st, createSession('s2', makeConfig()))
    removeSession(st, 's1')
    expect(st.order).toEqual(['s2'])
    expect(st.activeId).toBe('s2')
  })

  it('removeSession：清空后 activeId 归 null', () => {
    const st = makeRegistry()
    registerSession(st, createSession('s1', makeConfig()))
    removeSession(st, 's1')
    expect(st.order).toEqual([])
    expect(st.activeId).toBeNull()
  })

  it('replaceSession（重连换账）：旧 id 出表新 id 入表，tab 序原位替换、activeId 跟随', () => {
    const st = makeRegistry()
    registerSession(st, createSession('s1', makeConfig()))
    registerSession(st, createSession('s2', makeConfig()))
    registerSession(st, createSession('s3', makeConfig()))
    const carried = createSession('s2-new', makeConfig())
    replaceSession(st, 's2', carried)
    expect(st.sessions['s2']).toBeUndefined()
    expect(st.sessions['s2-new']).toBe(carried)
    expect(st.order).toEqual(['s1', 's2-new', 's3'])
    // 活动的是 s3，不受影响
    expect(st.activeId).toBe('s3')
    // 换掉活动 tab 时 activeId 跟随新 id
    const carried1 = createSession('s1-new', makeConfig())
    replaceSession(st, 's1', carried1)
    expect(st.order).toEqual(['s1-new', 's2-new', 's3'])
    expect(st.activeId).toBe('s3')
    removeSession(st, 's3')
    removeSession(st, 's2-new')
    expect(st.activeId).toBe('s1-new')
  })
})

describe('registry 建账竞态缓冲（落账域）', () => {
  it('recordStatus/recordError：未落账先暂存、落账后 flushPendingTo 回放（先状态后错误）', () => {
    const st = makeRegistry()
    recordStatus(st, 'race', 'connected')
    recordError(st, 'race', '')
    expect(st.sessions['race']).toBeUndefined()
    registerSession(st, createSession('race', makeConfig()))
    flushPendingTo(st, 'race')
    expect(st.sessions['race']!.status).toBe('connected')
    expect(st.sessions['race']!.error).toBe('')
  })

  it('recordError 非空错误连带置 error 态；非法状态值不写入', () => {
    const st = makeRegistry()
    registerSession(st, createSession('rc-1', makeConfig()))
    recordError(st, 'rc-1', '打开失败')
    expect(st.sessions['rc-1']!.error).toBe('打开失败')
    expect(st.sessions['rc-1']!.status).toBe('error')
    recordStatus(st, 'rc-1', 'bogus-status' as string)
    expect(st.sessions['rc-1']!.status).toBe('error')
    recordStatus(st, 'rc-1', 'connected')
    expect(st.sessions['rc-1']!.status).toBe('connected')
  })

  it('dropPending 丢弃未落账积压（重连换账后旧 id 事件无意义）', () => {
    const st = makeRegistry()
    recordStatus(st, 'old', 'connected')
    recordError(st, 'old', 'x')
    dropPending('old')
    registerSession(st, createSession('old', makeConfig()))
    flushPendingTo(st, 'old')
    expect(st.sessions['old']!.status).toBe('connecting')
    expect(st.sessions['old']!.error).toBe('')
  })
})
