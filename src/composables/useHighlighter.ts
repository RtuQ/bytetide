import { computed, ref, watch, type InjectionKey } from 'vue'
import { useThrottleFn } from '@vueuse/core'
import type { Keyword, LogLine, SearchState } from '../types'
import { SEARCH_COLOR } from '../types'

export interface ColorSegment {
  text: string
  color: string | null
}

export interface MatchStats {
  total: number
  matchLines: number[]
  kwCounts: Record<string, number>
}

export interface KeywordMatcher {
  id: string
  matcher: RegExp | null
  color: string
}

export function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** 由单个匹配配置构造 RegExp；空 pattern 或非法正则返回 null */
export function buildMatcher(s: {
  pattern: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
}): RegExp | null {
  if (!s.pattern) return null
  let src: string
  if (s.useRegex) {
    src = s.pattern
  } else {
    src = escapeRegex(s.pattern)
    if (s.wholeWord) src = `\\b${src}\\b`
  }
  const flags = s.caseSensitive ? 'g' : 'gi'
  try {
    return new RegExp(src, flags)
  } catch {
    return null
  }
}

export function buildKeywordMatchers(keywords: Keyword[]): KeywordMatcher[] {
  return keywords.map((k) => ({ id: k.id, matcher: buildMatcher(k), color: k.color }))
}

/** 布尔测试用匹配器：非全局 flag，便于 re.test() 重复调用且无 lastIndex 副作用 */
export function buildTestMatcher(s: {
  pattern: string
  useRegex: boolean
  caseSensitive: boolean
  wholeWord: boolean
}): RegExp | null {
  if (!s.pattern) return null
  let src: string
  if (s.useRegex) {
    src = s.pattern
  } else {
    src = escapeRegex(s.pattern)
    if (s.wholeWord) src = `\\b${src}\\b`
  }
  const flags = s.caseSensitive ? '' : 'i'
  try {
    return new RegExp(src, flags)
  } catch {
    return null
  }
}

/**
 * 多色高亮分段：关键词按列表顺序优先（先到先得），搜索色其次。
 * 与已着色区间重叠的后续区间整体跳过，保证每段只一种颜色。
 */
export function segmentsMulti(
  text: string,
  searchMatcher: RegExp | null,
  keywordMatchers: KeywordMatcher[],
): ColorSegment[] {
  if (!text) return [{ text, color: null }]
  const intervals: { s: number; e: number; color: string }[] = []
  const tryAdd = (s: number, e: number, color: string) => {
    if (e <= s) return
    for (const iv of intervals) {
      if (s < iv.e && iv.s < e) return // 重叠则跳过
    }
    intervals.push({ s, e, color })
  }

  for (const km of keywordMatchers) {
    if (!km.matcher) continue
    km.matcher.lastIndex = 0
    let m: RegExpExecArray | null
    while ((m = km.matcher.exec(text)) !== null) {
      if (m[0].length) tryAdd(m.index, m.index + m[0].length, km.color)
      else km.matcher.lastIndex++
    }
  }
  if (searchMatcher) {
    searchMatcher.lastIndex = 0
    let m: RegExpExecArray | null
    while ((m = searchMatcher.exec(text)) !== null) {
      if (m[0].length) tryAdd(m.index, m.index + m[0].length, SEARCH_COLOR)
      else searchMatcher.lastIndex++
    }
  }

  if (!intervals.length) return [{ text, color: null }]
  intervals.sort((a, b) => a.s - b.s)
  const out: ColorSegment[] = []
  let pos = 0
  for (const iv of intervals) {
    if (iv.s > pos) out.push({ text: text.slice(pos, iv.s), color: null })
    out.push({ text: text.slice(iv.s, iv.e), color: iv.color })
    pos = iv.e
  }
  if (pos < text.length) out.push({ text: text.slice(pos), color: null })
  return out
}

/** 行内高亮样式：背景半透明 + 前景全色 + 加粗 */
export function hlStyle(color: string) {
  return { background: color + '40', color, fontWeight: 700 }
}

/** 增量扫描：对 lines[from..] 段执行匹配并累加进 acc（原地修改）。
 *  computeStats 全量 = scanInto(from=0)；增量路径传上次游标，只扫新增行。 */
export function scanInto(
  lines: LogLine[],
  from: number,
  sm: RegExp | null,
  kms: KeywordMatcher[],
  acc: MatchStats,
): void {
  for (let i = from; i < lines.length; i++) {
    const ln = lines[i]!
    if (sm) {
      sm.lastIndex = 0
      let count = 0
      let m: RegExpExecArray | null
      while ((m = sm.exec(ln.text)) !== null) {
        count++
        if (m[0].length === 0) sm.lastIndex++
      }
      if (count > 0) {
        acc.total += count
        acc.matchLines.push(ln.no)
      }
    }
    for (const km of kms) {
      if (!km.matcher) continue
      km.matcher.lastIndex = 0
      let m: RegExpExecArray | null
      while ((m = km.matcher.exec(ln.text)) !== null) {
        acc.kwCounts[km.id] = (acc.kwCounts[km.id] ?? 0) + 1
        if (m[0].length === 0) km.matcher.lastIndex++
      }
    }
  }
}

/** 统计：搜索命中总数 + 命中行号 + 每关键词命中次数（单次扫描 lines） */
export function computeStats(
  lines: LogLine[],
  searchMatcher: RegExp | null,
  keywordMatchers: KeywordMatcher[],
): MatchStats {
  const acc: MatchStats = { total: 0, matchLines: [], kwCounts: {} }
  for (const km of keywordMatchers) acc.kwCounts[km.id] = 0
  scanInto(lines, 0, searchMatcher, keywordMatchers, acc)
  return acc
}

/**
 * 高亮 + 统计 composable（在 App 顶层创建并 provide，供 LogView/MatchStats/KeywordPanel 共享，
 * 避免重复扫描全量行）。
 * - 仅对可见行做高亮（模板里调 segmentsFor）
 * - 全量统计走节流（300ms）后台扫描
 */
export function useHighlighter(
  getSearch: () => SearchState,
  getKeywords: () => Keyword[],
  getLines: () => LogLine[],
  getVersion: () => number,
) {
  const searchMatcher = computed(() => buildMatcher(getSearch()))
  const keywordMatchers = computed(() => buildKeywordMatchers(getKeywords()))
  const stats = ref<MatchStats>({ total: 0, matchLines: [], kwCounts: {} })
  // 增量游标：上次已扫到的行数组下标 + 参与过统计的匹配器身份。
  // 匹配器/关键词变化或缓冲截断（长度小于游标）时全量重建；否则只扫新增行--
  // 搜索激活时长跑大缓冲不再每 300ms 全量重扫（吞吐赤字根因之一）。
  let cursor = 0
  let lastSm: RegExp | null = null
  let lastKms: KeywordMatcher[] | null = null
  const acc: MatchStats = { total: 0, matchLines: [], kwCounts: {} }

  const recompute = useThrottleFn(() => {
    const sm = searchMatcher.value
    const kms = keywordMatchers.value
    const lines = getLines()
    // 空载早退：无搜索词且无关键词时跳过扫描（长跑监控的常态路径）
    if (!sm && kms.length === 0) {
      cursor = 0
      acc.total = 0
      acc.matchLines.length = 0
      acc.kwCounts = {}
      stats.value = { total: 0, matchLines: [], kwCounts: {} }
      lastSm = sm
      lastKms = kms
      return
    }
    if (sm !== lastSm || kms !== lastKms || lines.length < cursor) {
      cursor = 0
      acc.total = 0
      acc.matchLines.length = 0
      acc.kwCounts = {}
      for (const km of kms) acc.kwCounts[km.id] = 0
    }
    if (cursor < lines.length) {
      scanInto(lines, cursor, sm, kms, acc)
      cursor = lines.length
    }
    lastSm = sm
    lastKms = kms
    stats.value = { total: acc.total, matchLines: acc.matchLines, kwCounts: { ...acc.kwCounts } }
  }, 300)
  watch([searchMatcher, keywordMatchers, getVersion], () => recompute(), { immediate: true })
  const segmentsFor = (text: string) =>
    segmentsMulti(text, searchMatcher.value, keywordMatchers.value)
  return { searchMatcher, keywordMatchers, stats, segmentsFor }
}

export type Highlighter = ReturnType<typeof useHighlighter>
export const HIGHLIGHTER_KEY: InjectionKey<Highlighter> = Symbol('highlighter')
