import { ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { makeCodec } from '../persistence/schema'
import { loadValue, saveStored } from '../persistence/storage'
import { zhCN } from './messages/zh-CN'
import type { MessageKey, Messages } from './messages/zh-CN'
import { en } from './messages/en'

/**
 * 界面语言（自研轻量 i18n）：t() 查表 + {name} 插值，零运行时依赖，仿 useTheme
 * 模式（模块级单例 ref + 信封持久化）。
 * - t() 内读 locale.value → 模板/computed 调用点自动被依赖追踪，切语言即时全量
 *   刷新。**模块级映射表存文案值的写法已废止**：动态映射改存 MessageKey，使用点
 *   t(MAP[code]) 求值。
 * - 持久化键 `serialtool.locale`（v1 信封，AGENTS.md 键清单）；首启无值按系统
 *   语言（en* → en，其余中文）。
 * - `<html lang>` 首屏前的另一处默认同步点在 index.html 内联脚本（AGENTS.md
 *   「两处默认同步」纪律，改默认必须同步两处）。
 * - en 域键集由类型层强制与 zh 一致；运行时兜底回中文词条。
 */

export type Locale = 'zh-CN' | 'en'
export type { MessageKey, Messages }

const LOCALE_KEY = 'serialtool.locale'

const localeCodec = makeCodec<Locale>('locale', (raw) => {
  if (raw === 'zh-CN' || raw === 'en') return raw
  throw new Error(`invalid locale: ${String(raw)}`)
})

/** 系统语言判定（纯函数）：按偏好顺序取首个受支持语言（zh* → 中文，en* → 英文） */
export function detectSystemLocale(langs: readonly string[]): Locale {
  for (const lang of langs) {
    const s = lang.toLowerCase()
    if (s === 'zh' || s.startsWith('zh-')) return 'zh-CN'
    if (s === 'en' || s.startsWith('en-')) return 'en'
  }
  return 'zh-CN'
}

function systemLocales(): string[] {
  try {
    const nav = navigator as { languages?: readonly string[]; language?: string }
    if (Array.isArray(nav.languages) && nav.languages.length > 0) return [...nav.languages]
    if (nav.language) return [nav.language]
  } catch {
    /* 非 DOM 环境回落 */
  }
  return []
}

/** 当前语言（模块级单例 ref，跨组件共享） */
export const locale = ref<Locale>('zh-CN')

function dictOf(l: Locale): Messages {
  return l === 'en' ? en : zhCN
}

function interpolate(tpl: string, params: Record<string, string | number>): string {
  return tpl.replace(/\{(\w+)\}/g, (m, name) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : m,
  )
}

/** 翻译：查当前语言词条并做 {name} 插值 */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const tpl = dictOf(locale.value)[key] ?? zhCN[key]
  return params === undefined ? tpl : interpolate(tpl, params)
}

/** 动态键查表（errors.<code> 等运行期拼接键）：未命中回退 fallback（不插值） */
export function tDynamic(key: string, fallback: string, params?: Record<string, string | number>): string {
  const tpl = (dictOf(locale.value) as Record<string, string>)[key]
  if (tpl === undefined) return fallback
  return params === undefined ? tpl : interpolate(tpl, params)
}

function applyLocaleSideEffects(l: Locale) {
  try {
    document.documentElement.lang = l
  } catch {
    /* 非 DOM 环境 */
  }
  try {
    document.title = t('app.title')
  } catch {
    /* 非 DOM 环境 */
  }
  // 同步原生窗口标题（浏览器态/无权限时忽略，与 useTheme 同策略）
  try {
    getCurrentWindow().setTitle(t('app.title')).catch(() => {})
  } catch {
    /* ignore */
  }
}

/** 设置并持久化语言（副作用：<html lang>/文档与原生窗口标题即时更新） */
export function setLocale(l: Locale) {
  locale.value = l
  try {
    saveStored(LOCALE_KEY, localeCodec.schema, l)
  } catch {
    /* ignore */
  }
  applyLocaleSideEffects(l)
}

function init() {
  locale.value = loadValue(LOCALE_KEY, localeCodec, detectSystemLocale(systemLocales()))
  applyLocaleSideEffects(locale.value)
}
init() // 模块加载即同步初始化（loadValue 同步读，Vue 挂载前语言已定，无闪屏）

/** 组合式入口（与 useTheme 同风格：状态是模块级单例，此处仅薄封装） */
export function useLocale() {
  return { locale, setLocale, t }
}

/** 测试辅助：不传 l = 清掉持久化值后按系统语言重判；传 l = 直接钉住 */
export function _resetLocaleForTest(l?: Locale) {
  if (l === undefined) {
    try {
      localStorage.removeItem(LOCALE_KEY)
    } catch {
      /* ignore */
    }
    l = detectSystemLocale(systemLocales())
  }
  locale.value = l
  applyLocaleSideEffects(l)
}
