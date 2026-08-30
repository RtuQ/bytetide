import { ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'

export type ThemeMode = 'dark' | 'light'

const THEME_KEY = 'serialtool.theme'

function loadTheme(): ThemeMode {
  try {
    const v = localStorage.getItem(THEME_KEY)
    if (v === 'light' || v === 'dark') return v
  } catch {
    /* ignore */
  }
  return 'light' // 默认浅色（白）；仅当用户显式存过 dark 才用深色
}

/** 当前主题（模块级单例 ref，跨组件共享） */
export const theme = ref<ThemeMode>(loadTheme())

/** 应用主题到 <html data-theme>、持久化、并同步原生窗口标题栏 */
export function applyTheme(mode: ThemeMode = theme.value) {
  theme.value = mode
  try {
    document.documentElement.dataset.theme = mode
  } catch {
    /* ignore */
  }
  try {
    localStorage.setItem(THEME_KEY, mode)
  } catch {
    /* ignore */
  }
  // 同步原生窗口标题栏主题（浏览器态/无权限时忽略）
  try {
    getCurrentWindow().setTheme(mode).catch(() => {})
  } catch {
    /* ignore */
  }
}

export function toggleTheme() {
  applyTheme(theme.value === 'dark' ? 'light' : 'dark')
}

/** 组合式入口：在 App 根 setup 调用一次，确保主题已应用 */
export function useTheme() {
  applyTheme()
  return { theme, toggleTheme, applyTheme }
}
