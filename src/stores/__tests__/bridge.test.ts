import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useBridgeStore } from '../bridge'
import { DEFAULT_BRIDGE_CONFIG, type BridgeView } from '../../types'

// invoke 全文件打桩：bridge store 的 load/update/regenToken 在无 Tauri 后端的测试环境可走通
const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

function mkView(overrides: {
  config?: Partial<BridgeView['config']>
  runtime?: Partial<BridgeView['runtime']>
} = {}): BridgeView {
  return {
    config: { ...DEFAULT_BRIDGE_CONFIG, enabled: true, token: 'ab'.repeat(32), ...overrides.config },
    runtime: { state: 'disabled', bound: null, lastError: null, ...overrides.runtime },
  }
}

describe('bridge store 摄取 BridgeView（config+runtime 两段）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    invokeMock.mockReset()
    invokeMock.mockImplementation(async () => null as unknown)
  })

  it('load 成功同时存 config 与 runtime', async () => {
    invokeMock.mockResolvedValueOnce(
      mkView({ runtime: { state: 'running', bound: '127.0.0.1:8765', lastError: null } }),
    )
    const store = useBridgeStore()
    await store.load()
    expect(invokeMock).toHaveBeenCalledWith('bridge_get_config_cmd')
    expect(store.config.enabled).toBe(true)
    expect(store.runtime.state).toBe('running')
    expect(store.runtime.bound).toBe('127.0.0.1:8765')
    expect(store.loaded).toBe(true)
    expect(store.running).toBe(true)
  })

  it('update 成功存回 BridgeView 两段并清 lastError', async () => {
    const store = useBridgeStore()
    invokeMock.mockResolvedValueOnce(mkView())
    await store.load()
    invokeMock.mockResolvedValueOnce(
      mkView({ config: { port: 9999 }, runtime: { state: 'running', bound: '127.0.0.1:9999' } }),
    )
    await store.update({ port: 9999 })
    expect(store.config.port).toBe(9999)
    expect(store.runtime.state).toBe('running')
    expect(store.runtime.bound).toBe('127.0.0.1:9999')
    expect(store.lastError).toBe('')
  })

  it('update 拒绝时 lastError 记录且 runtime 呈错误态（保留原 bound）', async () => {
    const store = useBridgeStore()
    invokeMock.mockResolvedValueOnce(
      mkView({ runtime: { state: 'running', bound: '127.0.0.1:8765', lastError: null } }),
    )
    await store.load()
    invokeMock.mockRejectedValueOnce('bind 127.0.0.1:80 failed: Address already in use')
    await store.update({ port: 80 })
    expect(store.lastError).toContain('Address already in use')
    expect(store.runtime.state).toBe('error')
    expect(store.runtime.lastError).toContain('Address already in use')
    // 保留后端最近一次返回的绑定地址（后端此刻仍按旧配置监听）
    expect(store.runtime.bound).toBe('127.0.0.1:8765')
    expect(store.running).toBe(false)
  })

  it('regenToken 成功只换 token、runtime 原样', async () => {
    const store = useBridgeStore()
    invokeMock.mockResolvedValueOnce(
      mkView({ runtime: { state: 'running', bound: '127.0.0.1:8765' } }),
    )
    await store.load()
    invokeMock.mockResolvedValueOnce(
      mkView({ config: { token: 'cd'.repeat(32) }, runtime: { state: 'running', bound: '127.0.0.1:8765' } }),
    )
    await store.regenToken()
    expect(store.config.token).toBe('cd'.repeat(32))
    expect(store.runtime.state).toBe('running')
    expect(store.lastError).toBe('')
  })

  it('regenToken 拒绝时 lastError 记录且旧 token 不动', async () => {
    const store = useBridgeStore()
    invokeMock.mockResolvedValueOnce(mkView({ config: { token: 'ab'.repeat(32) } }))
    await store.load()
    invokeMock.mockRejectedValueOnce('secure randomness unavailable: injected')
    await store.regenToken()
    expect(store.lastError).toContain('randomness')
    expect(store.config.token).toBe('ab'.repeat(32))
  })

  it('远程绑定确认随补丁一次性透传（confirmRemote → confirm_remote）', async () => {
    const store = useBridgeStore()
    invokeMock.mockResolvedValueOnce(mkView())
    await store.load()
    invokeMock.mockResolvedValueOnce(
      mkView({ config: { bind: '0.0.0.0' }, runtime: { state: 'running', bound: '0.0.0.0:8765' } }),
    )
    await store.update({ bind: '0.0.0.0', confirmRemote: true })
    expect(invokeMock).toHaveBeenLastCalledWith('bridge_set_config_cmd', {
      patch: { bind: '0.0.0.0', confirmRemote: true },
    })
    expect(store.config.bind).toBe('0.0.0.0')
  })
})

describe('running getter 四态派生', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    invokeMock.mockReset()
    invokeMock.mockImplementation(async () => null as unknown)
  })

  it('load 前：enabled&&token 旧派生兜底', () => {
    const store = useBridgeStore()
    expect(store.running).toBe(false) // 默认 config.enabled=false
    store.config.enabled = true
    store.config.token = 'ab'.repeat(32)
    expect(store.running).toBe(true) // 兜底派生仍可用
  })

  it('load 后：由 runtime.state 派生，disabled/starting/error 均 false', async () => {
    const store = useBridgeStore()
    for (const state of ['disabled', 'starting', 'error'] as const) {
      invokeMock.mockResolvedValueOnce(mkView({ runtime: { state } }))
      await store.load()
      expect(store.runtime.state).toBe(state)
      expect(store.running).toBe(false)
    }
    invokeMock.mockResolvedValueOnce(mkView({ runtime: { state: 'running', bound: '127.0.0.1:8765' } }))
    await store.load()
    expect(store.running).toBe(true)
  })

  it('load 失败（浏览器冒烟）：回退默认配置、runtime 保持默认、错误入 lastError', async () => {
    invokeMock.mockRejectedValueOnce('invoke unavailable')
    const store = useBridgeStore()
    await store.load()
    expect(store.loaded).toBe(true)
    expect(store.config).toEqual({ ...DEFAULT_BRIDGE_CONFIG })
    expect(store.runtime.state).toBe('disabled')
    expect(store.lastError).toContain('invoke unavailable')
    expect(store.running).toBe(false)
  })
})
