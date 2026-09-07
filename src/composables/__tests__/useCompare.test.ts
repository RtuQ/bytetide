import { describe, it, expect } from 'vitest'
import { alignCompareLines, diffSpans, scopeCompareLines, DIFF_INPUT_CAP } from '../useCompare'
import type { LogLine } from '../../types'

function mkLine(no: number, epoch: number, dir: 'rx' | 'tx' = 'rx', text = `l${no}`): LogLine {
  return { no, ts: `t${no}`, dir, text, bytes: null, epochMillis: epoch }
}

/** 紧凑断言形状：[a.no|null, b.no|null, op, delta] */
function shape(out: ReturnType<typeof alignCompareLines>) {
  return out.map((p) => [p.a?.no ?? null, p.b?.no ?? null, p.op, p.delta])
}

describe('alignCompareLines 序列对齐（锚点 + 缝隙填充）', () => {
  it('文本一致且 Δt≤容差 → equal 锚点，给绝对差值', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 1010)]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([[1, 1, 'equal', 10]])
    expect(out[0]!.spansA).toEqual([{ t: 'l1', hl: false }])
  })

  it('B 侧过旧行在缝隙填充中淘汰为 insert-b，近邻行配成 changed', () => {
    const a = [mkLine(1, 1000)]
    const b = [mkLine(1, 900), mkLine(2, 990)]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([
      [null, 1, 'insert-b', null],
      [1, 2, 'changed', 10],
    ])
  })

  it('窗口内无锚点时缝隙近邻配成 changed（Δ 超容差显示红由 UI 判定）', () => {
    const a = [mkLine(1, 1000, 'rx', 'v=1')]
    const b = [mkLine(1, 1100, 'rx', 'v=1')]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([[1, 1, 'changed', 100]])
  })

  it('时间近邻让位于文本一致：锚点优先配相同文本，时间更近的异文本成落单/changed', () => {
    const a = [mkLine(1, 1000, 'rx', 'l1')]
    const b = [mkLine(1, 980, 'rx', 'l1'), mkLine(2, 995, 'rx', 'l2'), mkLine(3, 1005, 'rx', 'l3')]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([
      [1, 1, 'equal', 20],
      [null, 2, 'insert-b', null],
      [null, 3, 'insert-b', null],
    ])
  })

  it('容差 0 只锚定完全同刻且同文本的行', () => {
    const a = [mkLine(1, 1000), mkLine(2, 2000)]
    const b = [mkLine(1, 1000), mkLine(2, 1500), mkLine(3, 2000)]
    const out = alignCompareLines(a, b, 0)
    expect(shape(out)).toEqual([
      [1, 1, 'equal', 0],
      [null, 2, 'insert-b', null],
      [2, 3, 'changed', 0],
    ])
  })

  it('B 耗尽后余下的 A 行全部 insert-a；锚点后无 B 行同理', () => {
    const a = [mkLine(1, 1000), mkLine(2, 1100)]
    const b = [mkLine(1, 1050)]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([
      [1, 1, 'equal', 50],
      [2, null, 'insert-a', null],
    ])
  })

  it('A 侧为空时 B 尾部也全部输出（修复旧版「B 尾部暂不输出」）', () => {
    expect(alignCompareLines([], [mkLine(1, 1)], 50).map((p) => p.op)).toEqual(['insert-b'])
  })

  it('重复文本行保序配对，不交叉重复消费', () => {
    const a = [mkLine(1, 1000, 'rx', 'ok'), mkLine(2, 2000, 'rx', 'ok')]
    const b = [mkLine(1, 1005, 'rx', 'ok'), mkLine(2, 2005, 'rx', 'ok')]
    const out = alignCompareLines(a, b, 50)
    expect(shape(out)).toEqual([
      [1, 1, 'equal', 5],
      [2, 2, 'equal', 5],
    ])
  })

  it('插入行夹在锚点之间：A 侧多出的行判 insert-a', () => {
    const a = [mkLine(1, 0, 'rx', 'p'), mkLine(2, 10, 'rx', 'Q'), mkLine(3, 20, 'rx', 'r')]
    const b = [mkLine(1, 0, 'rx', 'p'), mkLine(2, 20, 'rx', 'r')]
    const out = alignCompareLines(a, b, 100)
    expect(shape(out)).toEqual([
      [1, 1, 'equal', 0],
      [2, null, 'insert-a', null],
      [3, 2, 'equal', 0],
    ])
  })

  it('offset 偏移应用于 B 侧：live↔offline 时钟对表后可锚定', () => {
    const a = [mkLine(1, 1_700_000_000_000, 'rx', 'l1')]
    const b = [mkLine(1, 5000, 'rx', 'l1')]
    // 无偏移：量级悬殊不可锚定
    expect(alignCompareLines(a, b, 50)[0]!.op).toBe('changed')
    // 偏移对表后锚定
    const out = alignCompareLines(a, b, 50, 1_700_000_000_000 - 5000)
    expect(shape(out)).toEqual([[1, 1, 'equal', 0]])
  })

  it('文本不同但时间近邻的缝隙行：配成 changed 且行内 diff 高亮差异段', () => {
    const a = [mkLine(1, 1000, 'rx', 'v=1')]
    const b = [mkLine(1, 1005, 'rx', 'v=2')]
    const out = alignCompareLines(a, b, 50)
    expect(out[0]!.op).toBe('changed')
    expect(out[0]!.delta).toBe(5)
    expect(out[0]!.spansA).toEqual([
      { t: 'v=', hl: false },
      { t: '1', hl: true },
    ])
    expect(out[0]!.spansB).toEqual([
      { t: 'v=', hl: false },
      { t: '2', hl: true },
    ])
  })

  it('insert 落单行整段高亮', () => {
    const out = alignCompareLines([mkLine(1, 1000, 'rx', 'solo')], [], 50)
    expect(out[0]!.spansA).toEqual([{ t: 'solo', hl: true }])
    expect(out[0]!.spansB).toEqual([])
  })

  it('DIFF_INPUT_CAP 导出（CompareView 尾部截断用）', () => {
    expect(DIFF_INPUT_CAP).toBe(5000)
  })
})

describe('diffSpans 差异游程（多段字符级）', () => {
  it('公共前后缀保留，中间多段交错高亮（LCS）', () => {
    expect(diffSpans('aXbXc', 'aYbYc')).toEqual([
      { t: 'a', hl: false },
      { t: 'X', hl: true },
      { t: 'b', hl: false },
      { t: 'X', hl: true },
      { t: 'c', hl: false },
    ])
  })

  it('单段差异（对侧中段为空时退回三段游程）', () => {
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

  it('超长中段（>512）退回三段游程不做 DP', () => {
    const s = `q${'a'.repeat(600)}q`
    const o = `q${'b'.repeat(600)}q`
    expect(diffSpans(s, o)).toEqual([
      { t: 'q', hl: false },
      { t: 'a'.repeat(600), hl: true },
      { t: 'q', hl: false },
    ])
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
