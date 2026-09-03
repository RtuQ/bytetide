import { describe, it, expect } from 'vitest'
import { alignCompareLines, diffSpans, scopeCompareLines } from '../useCompare'
import type { LogLine } from '../../types'

function mkLine(no: number, epoch: number, dir: 'rx' | 'tx' = 'rx', text = `l${no}`): LogLine {
  return { no, ts: `t${no}`, dir, text, bytes: null, epochMillis: epoch }
}

describe('alignCompareLines 时间近邻配对', () => {
  it('容差内配对并给出绝对差值', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 1010)]
    const out = alignCompareLines(a, b, 50)
    expect(out).toEqual([{ a: a[0], b: b[0], delta: 10 }])
  })

  it('B 侧过旧行在配对前淘汰为孤立行', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 900), mkLine(2, 990)]
    const out = alignCompareLines(a, b, 50)
    expect(out).toEqual([
      { a: null, b: b[0], delta: null },
      { a: a[0], b: b[1], delta: 10 },
    ])
  })

  it('窗口内无候选时 A 行落单，B 侧行不消耗', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 1100)]
    const out = alignCompareLines(a, b, 50)
    expect(out).toEqual([{ a: a[0], b: null, delta: null }])
  })

  it('容差窗口内挑选时间最近的 B，被跳过的 B 转为孤立行（配对行在前，沿用原顺序）', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 980), mkLine(2, 995), mkLine(3, 1005)]
    const out = alignCompareLines(a, b, 50)
    expect(out).toEqual([
      { a: a[0], b: b[1], delta: 5 },
      { a: null, b: b[0], delta: null },
    ])
  })

  it('容差 0 只配对完全同刻的行', () => {
    const a = [mkLine(1, 1000), mkLine(2, 2000)]
    const b = [mkLine(1, 1000), mkLine(2, 1500), mkLine(3, 2000)]
    const out = alignCompareLines(a, b, 0)
    expect(out).toEqual([
      { a: a[0], b: b[0], delta: 0 },
      { a: null, b: b[1], delta: null },
      { a: a[1], b: b[2], delta: 0 },
    ])
  })

  it('B 耗尽后余下的 A 行全部落单', () => {
    const a = [mkLine(1, 1000), mkLine(2, 1100)]
    const b = [mkLine(1, 1050)]
    const out = alignCompareLines(a, b, 50)
    expect(out).toEqual([
      { a: a[0], b: b[0], delta: 50 },
      { a: a[1], b: null, delta: null },
    ])
  })

  it('A 侧为空时无输出', () => {
    expect(alignCompareLines([], [mkLine(1, 1)], 50)).toEqual([])
  })
})

describe('diffSpans 差异游程', () => {
  it('公共前后缀修剪，中间差异段高亮', () => {
    expect(diffSpans('abcdefgh', 'abbefgh')).toEqual([
      { t: 'ab', hl: false },
      { t: 'cd', hl: true },
      { t: 'efgh', hl: false },
    ])
  })

  it('同长单字符差异只高亮差异字符', () => {
    expect(diffSpans('cat', 'hat')).toEqual([
      { t: 'c', hl: true },
      { t: 'at', hl: false },
    ])
  })

  it('文本相同返回单段无高亮', () => {
    expect(diffSpans('abc', 'abc')).toEqual([{ t: 'abc', hl: false }])
  })

  it('对侧为空或缺失时整段高亮', () => {
    expect(diffSpans('abc', '')).toEqual([{ t: 'abc', hl: true }])
    expect(diffSpans('abc', undefined)).toEqual([{ t: 'abc', hl: true }])
  })

  it('双侧皆空返回空串单段', () => {
    expect(diffSpans('', '')).toEqual([{ t: '', hl: false }])
  })
})

describe('scopeCompareLines 方向范围过滤', () => {
  const lines = [mkLine(1, 1, 'rx'), mkLine(2, 2, 'tx'), mkLine(3, 3, 'rx')]
  it('rx 范围只保留接收行', () => {
    expect(scopeCompareLines(lines, 'rx')).toEqual([lines[0], lines[2]])
  })
  it('all 范围保留全部行', () => {
    expect(scopeCompareLines(lines, 'all')).toEqual(lines)
  })
})
