import { defineStore } from 'pinia'
import { commands } from '../ipc/commands'
import { normalizeIpcError } from '../ipc/errors'
import {
  DEFAULT_BRIDGE_CONFIG,
  DEFAULT_BRIDGE_RUNTIME,
  type BridgeConfig,
  type BridgeRuntime,
} from '../types'

/**
 * REST 分析桥配置 store。
 * 桥服务在 Tauri 后端按 enabled 自启/自停；前端只读写配置、复制 URL/令牌。
 * running 由后端 BridgeRuntime.state 派生（后端同步 bind——启动成败在命令返回前即确定，
 * 绑定失败经 runtime.state='error' + lastError 可见，不再仅 eprintln）。
 */
export const useBridgeStore = defineStore('bridge', {
  state: () => ({
    config: { ...DEFAULT_BRIDGE_CONFIG } as BridgeConfig,
    runtime: { ...DEFAULT_BRIDGE_RUNTIME } as BridgeRuntime,
    loaded: false,
    busy: false,
    /** 最近一次 apply 的错误（供面板提示） */
    lastError: '',
  }),
  getters: {
    // 后端真实运行态优先；load 前用 enabled&&token 旧派生兜底（浏览器冒烟无后端）
    running: (s): boolean =>
      s.loaded ? s.runtime.state === 'running' : s.config.enabled && s.config.token.length > 0,
    /** 完整调用地址（含 token 的 curl 一行仅用于展示，不回显 token） */
    baseUrl: (s): string => {
      const host = s.config.bind === '0.0.0.0' ? '127.0.0.1' : s.config.bind
      return `http://${host}:${s.config.port}`
    },
  },
  actions: {
    async load() {
      try {
        const view = await commands.getBridgeConfig()
        this.config = view.config
        this.runtime = view.runtime
      } catch (e) {
        // 拉不到后端（浏览器冒烟/后端异常）：回退默认配置，runtime 保持默认，错误入 lastError
        this.lastError = normalizeIpcError(e)
        this.config = { ...DEFAULT_BRIDGE_CONFIG }
      }
      this.loaded = true
    },
    /**
     * 应用补丁；后端持久化并按需重启（同步 bind，成败即刻可知）。
     * `confirmRemote`：远程绑定（非环回地址）的一次性显式确认，随补丁透传后端，
     * 后端未收到时对远程 bind 切换一律拒绝（fixed text: remote bind requires…）。
     */
    async update(patch: Partial<BridgeConfig> & { confirmRemote?: boolean }) {
      this.busy = true
      try {
        const view = await commands.setBridgeConfig(patch)
        this.config = view.config
        this.runtime = view.runtime
        this.lastError = ''
      } catch (e) {
        // 拒绝（替换绑失败/随机源失败等）：lastError 记录，runtime 置错误态便于面板展示
        //（保留原 bound——后端此时仍按旧配置监听，下次 load 与后端重新对齐）
        this.lastError = normalizeIpcError(e)
        this.runtime = { ...this.runtime, state: 'error', lastError: normalizeIpcError(e) }
      } finally {
        this.busy = false
      }
    },
    /** 重置令牌（后端无需重启，实时生效）。随机源失败 Err 且旧 token 不动。 */
    async regenToken() {
      this.busy = true
      try {
        const view = await commands.regenerateBridgeToken()
        this.config = view.config
        this.runtime = view.runtime
        this.lastError = ''
      } catch (e) {
        this.lastError = normalizeIpcError(e)
        this.runtime = { ...this.runtime, state: 'error', lastError: normalizeIpcError(e) }
      } finally {
        this.busy = false
      }
    },
    async copyUrl() {
      try {
        await navigator.clipboard.writeText(this.baseUrl)
      } catch {
        /* 剪贴板可能在非安全上下文失败，忽略 */
      }
    },
    async copyToken() {
      try {
        await navigator.clipboard.writeText(this.config.token)
      } catch {
        /* ignore */
      }
    },
  },
})
