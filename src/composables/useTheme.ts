import { ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { makeCodec } from '../persistence/schema'
import { loadValue, saveStored } from '../persistence/storage'

export type ThemeMode = 'dark' | 'light'

const THEME_KEY = 'serialtool.theme'

/** 信封 codec（schema 'theme'）：旧裸字符串 'dark'/'light'（含非 JSON 原文）由
 *  loadStored 迁移，首次成功读取即回写 v1 信封。index.html 内联脚本同步认两种
 *  形状（默认值初始化的两处之一，改默认必须同步两处——AGENTS.md）。 */
const themeCodec = makeCodec<ThemeMode>('theme', (raw) => {
  if (raw === 'light' || raw === 'dark') return raw
  throw new Error(`invalid theme: ${String(raw)}`)
})

function loadTheme(): ThemeMode {
  return loadValue(THEME_KEY, themeCodec, 'light') // 默认浅色（白）；仅当用户显式存过 dark 才用深色
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
    saveStored(THEME_KEY, themeCodec.schema, mode)
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
