import { reactive } from 'vue'
import { makeCodec } from '../persistence/schema'
import { loadValue, saveStored } from '../persistence/storage'

/**
 * 通知开关（通知重设计 v2）：单个总开关，统一门控连接状态与串口插拔提示。
 * localStorage 键 `serialtool.notifications`（v1 信封），模块级单例——
 * SettingsPopover 的开关与 useTauriEvents 的弹出门控共享同一份状态。
 * 纯校验在 codec.parse，副作用统一走 src/persistence。
 */

const KEY = 'serialtool.notifications'

const codec = makeCodec<{ enabled: boolean }>('notifications', (raw) => {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid notifications')
  const v = raw as { enabled?: unknown }
  // 缺失回落默认开（增量字段向后兼容）
  return { enabled: v.enabled === undefined ? true : v.enabled === true }
})

const prefs = reactive({ enabled: true })
let loaded = false
function ensureLoaded() {
  if (loaded) return
  loaded = true
  Object.assign(prefs, loadValue(KEY, codec, { enabled: true }))
}

export function useNotificationPrefs() {
  ensureLoaded()
  return {
    /** 响应式偏好（门控与开关 UI 共享） */
    prefs,
    setEnabled(value: boolean) {
      if (prefs.enabled === value) return
      prefs.enabled = value
      saveStored(KEY, codec.schema, { ...prefs })
    },
  }
}

/** 测试辅助：重置模块级单例 */
export function _resetNotificationPrefsForTest() {
  loaded = false
  prefs.enabled = true
}
