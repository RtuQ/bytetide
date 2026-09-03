import type { LogLine } from '../types'

/** 双会话对比的行方向范围：rx=仅接收行；all=收发行全部 */
export type CompareDirScope = 'rx' | 'all'

/** 时间对齐配对结果：a/b 任一侧可为 null（该侧为孤立行） */
export interface ComparePair {
  a: LogLine | null
  b: LogLine | null
  /** null 表示该侧为孤立行（未配对） */
  delta: number | null
}

/** 差异渲染游程：hl=true 为与对侧文本的差异段 */
export interface CompareDiffSpan {
  t: string
  hl: boolean
}

/** 时间近邻配对（贪心双指针，各自保持原顺序；b 过旧且无法再匹配时淘汰为孤立行）。
 *  与 ComparePanel 的原实现语义一致：以 A 为时间轴基准，尾部长于最后一条 A 的
 *  B 行暂不输出（待 A 侧出现新行后再参与配对）。 */
export function alignCompareLines(
  aArr: LogLine[],
  bArr: LogLine[],
  tol: number,
): ComparePair[] {
  const out: ComparePair[] = []
  let j = 0
  for (const a of aArr) {
    while (j < bArr.length && bArr[j]!.epochMillis < a.epochMillis - tol) {
      out.push({ a: null, b: bArr[j]!, delta: null })
      j += 1
    }
    if (j < bArr.length && Math.abs(bArr[j]!.epochMillis - a.epochMillis) <= tol) {
      let best = j
      // 在容差窗口内挑时间最近的一个 b（仅向前看有限个候选）
      let limit = Math.min(bArr.length, j + 8)
      for (let k = j; k < limit; k++) {
        if (
          Math.abs(bArr[k]!.epochMillis - a.epochMillis) <
          Math.abs(bArr[best]!.epochMillis - a.epochMillis)
        ) {
          best = k
        }
      }
      const bb = bArr[best]!
      out.push({ a, b: bb, delta: Math.abs(bb.epochMillis - a.epochMillis) })
      // 吞掉被跳过的 b 作为孤立行
      for (let k = j; k < best; k++) out.push({ a: null, b: bArr[k]!, delta: null })
      j = best + 1
    } else {
      out.push({ a, b: null, delta: null })
    }
  }
  return out
}

/** 与对侧文本的公共前后缀修剪，返回三段游程（前缀 / 差异段 hl=true / 后缀） */
export function diffSpans(
  text: string | undefined,
  other: string | undefined,
): CompareDiffSpan[] {
  const s = text ?? ''
  const o = other ?? ''
  let p = 0
  const minLen = Math.min(s.length, o.length)
  while (p < minLen && s[p] === o[p]) p += 1
  let suf = 0
  while (suf < minLen - p && s[s.length - 1 - suf] === o[o.length - 1 - suf]) suf += 1
  const midEnd = s.length - suf
  const out: CompareDiffSpan[] = []
  if (p > 0) out.push({ t: s.slice(0, p), hl: false })
  if (midEnd > p) out.push({ t: s.slice(p, midEnd), hl: true })
  if (suf > 0) out.push({ t: s.slice(midEnd), hl: false })
  return out.length ? out : [{ t: '', hl: false }]
}

/** 按方向范围过滤参与对比的行 */
export function scopeCompareLines(
  lines: LogLine[],
  scope: CompareDirScope,
): LogLine[] {
  return lines.filter((l) => (scope === 'rx' ? l.dir === 'rx' : true))
}
