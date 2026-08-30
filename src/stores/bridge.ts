import { defineStore } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { DEFAULT_BRIDGE_CONFIG, type BridgeConfig } from '../types'

/**
 * REST 分析桥配置 store。
 * 桥服务在 Tauri 后端按 enabled 自启/自停；前端只读写配置、复制 URL/令牌。
 * running 由 config.enabled 派生（后端启用即监听，绑定失败仅 eprintln 不抛回前端）。
 */
export const useBridgeStore = defineStore('bridge', {
  state: () => ({
    config: { ...DEFAULT_BRIDGE_CONFIG } as BridgeConfig,
    loaded: false,
    busy: false,
    /** 最近一次 apply 的错误（供面板提示） */
    lastError: '',
  }),
  getters: {
    running: (s): boolean => s.config.enabled && s.config.token.length > 0,
    /** 完整调用地址（含 token 的 curl 一行仅用于展示，不回显 token） */
    baseUrl: (s): string => {
      const host = s.config.bind === '0.0.0.0' ? '127.0.0.1' : s.config.bind
      return `http://${host}:${s.config.port}`
    },
  },
  actions: {
    async load() {
      try {
        this.config = await invoke<BridgeConfig>('bridge_get_config_cmd')
      } catch (e) {
        this.lastError = String(e)
        this.config = { ...DEFAULT_BRIDGE_CONFIG }
      }
      this.loaded = true
    },
    /** 应用补丁；后端持久化并按需重启。 */
    async update(patch: Partial<BridgeConfig>) {
      this.busy = true
      try {
        this.config = await invoke<BridgeConfig>('bridge_set_config_cmd', { patch })
        this.lastError = ''
      } catch (e) {
        this.lastError = String(e)
      } finally {
        this.busy = false
      }
    },
    /** 重置令牌（后端无需重启，实时生效）。 */
    async regenToken() {
      this.busy = true
      try {
        this.config = await invoke<BridgeConfig>('bridge_regen_token_cmd')
        this.lastError = ''
      } catch (e) {
        this.lastError = String(e)
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
