import { describe, it, expect } from 'vitest'
import { computeStats, scanInto, buildMatcher, buildKeywordMatchers } from '../useHighlighter'
import type { Keyword, LogLine } from '../../types'

function mkLine(no: number, text: string): LogLine {
  return { no, ts: '00:00:00.000', dir: 'rx', text, bytes: null, epochMillis: no * 10 }
}

const SEARCH = {
  pattern: 'err',
  useRegex: false,
  caseSensitive: false,
  wholeWord: false,
}

const KWS: Keyword[] = [
  { id: 'k1', pattern: 'warn', color: '#fbbf24', useRegex: false, caseSensitive: false, wholeWord: false },
]

function matchers() {
  return {
    sm: buildMatcher(SEARCH),
    kms: buildKeywordMatchers(KWS),
  }
}

describe('高亮统计增量等价', () => {
  it('分段累加与全量扫描结果一致', () => {
    const all = [
      mkLine(1, 'ok line'),
      mkLine(2, 'err one'),
      mkLine(3, 'warn err twice err'),
      mkLine(4, 'warn only'),
      mkLine(5, 'nothing'),
    ]
    const { sm, kms } = matchers()
    const full = computeStats(all, sm, kms)

    // 模拟真实增量用法：数组追加 + 游标前进（useHighlighter 的 cursor 模式）
    const acc = { total: 0, matchLines: [] as number[], kwCounts: { k1: 0 } }
    const arr: LogLine[] = []
    let cursor = 0
    for (const l of all) {
      arr.push(l)
      scanInto(arr, cursor, sm, kms, acc)
      cursor = arr.length
    }

    expect(acc.total).toBe(full.total)
    expect(acc.matchLines).toEqual(full.matchLines)
    expect(acc.kwCounts).toEqual(full.kwCounts)
    // 具体值抽查：err 出现 3 次、命中行 2/3；warn 出现 2 次
    expect(full.total).toBe(3)
    expect(full.matchLines).toEqual([2, 3])
    expect(full.kwCounts['k1']).toBe(2)
  })

  it('空段扫描不改变累计值', () => {
    const lines = [mkLine(1, 'err')]
    const { sm, kms } = matchers()
    const acc = { total: 0, matchLines: [] as number[], kwCounts: { k1: 0 } }
    scanInto(lines, 1, sm, kms, acc) // from == length：无操作
    expect(acc.total).toBe(0)
    expect(acc.matchLines).toEqual([])
  })
})
