import type { LogLine } from '../types'

/** 双会话对比的行方向范围：rx=仅接收行；all=收发行全部 */
export type CompareDirScope = 'rx' | 'all'

/** 行级 diff 判定：equal=锚点（文本一致且 Δt≤容差）；changed=缝隙内时间近邻
 *  配对（内容有差异）；insert-a/insert-b=落单行（对侧无对应行） */
export type CompareOp = 'equal' | 'changed' | 'insert-a' | 'insert-b'

/** 对齐配对结果：a/b 任一侧可为 null（该侧为孤立行） */
export interface ComparePair {
  a: LogLine | null
  b: LogLine | null
  /** null 表示该侧为孤立行（未配对） */
  delta: number | null
  op: CompareOp
  /** 预计算的本侧差异游程（数据层算好，渲染零逻辑）：equal=整行不高亮；
   *  changed=多段字符级 diff；insert=整行高亮 */
  spansA: CompareDiffSpan[]
  spansB: CompareDiffSpan[]
}

/** 差异渲染游程：hl=true 为与对侧文本的差异段 */
export interface CompareDiffSpan {
  t: string
  hl: boolean
}

/** 锚点候选对上限（Hunt–Szymanski 事件数）：重复文本×宽时间窗的病态日志
 *  超限即回退贪心对齐，防 computed 卡死 */
const ANCHOR_CAND_CAP = 200_000
/** 行内字符 diff 的 DP 封顶（单侧长度）：超过退回公共前后缀修剪三段游程 */
const DIFF_SEG_MAX = 512
/** 参与对齐的单侧行数上限：取尾部（最新）参与 diff，防大缓冲全量重算 */
export const DIFF_INPUT_CAP = 5000

/** 锚点文本归一化：只修剪行尾 \r（协议行常见的 CRLF 差异），时间戳/行号
 *  本就是独立字段不掺入 */
function normText(t: string): string {
  return t.endsWith('\r') ? t.slice(0, -1) : t
}

function eqSpan(t: string): CompareDiffSpan {
  return { t, hl: false }
}

function mkPair(
  a: LogLine | null,
  b: LogLine | null,
  op: CompareOp,
  delta: number | null,
): ComparePair {
  if (op === 'equal') {
    const t = eqSpan(a?.text ?? b?.text ?? '')
    return { a, b, delta, op, spansA: [t], spansB: [{ ...t }] }
  }
  if (op === 'changed') {
    const { spansS, spansO } = diffSpansCore(a?.text ?? '', b?.text ?? '')
    return { a, b, delta, op, spansA: spansS, spansB: spansO }
  }
  // insert-a / insert-b：落单行整段高亮（与旧版孤立行渲染一致）
  return op === 'insert-a'
    ? { a, b: null, delta: null, op, spansA: [{ t: a?.text ?? '', hl: true }], spansB: [] }
    : { a: null, b, delta: null, op, spansA: [], spansB: [{ t: b?.text ?? '', hl: true }] }
}

/**
 * 锚点匹配（Hunt–Szymanski）：候选 = 归一化文本相等且 |Δt|≤tol 的 (i,j) 对，
 * 在候选集上求 j 严格递增（且 i 天然严格递增）的最长匹配子序列——全局最优、
 * 保序，解决贪心「时间错开内容相同」的误配/漏配与重复行交叉匹配。
 * 返回 [i,j] 升序对；候选超限返回 null（调用方回退贪心）。
 */
function anchorPairs(
  aArr: LogLine[],
  bArr: LogLine[],
  tol: number,
  offset: number,
): [number, number][] | null {
  if (aArr.length === 0 || bArr.length === 0) return []
  // B 侧按归一化文本建索引
  const bIndex = new Map<string, number[]>()
  for (let j = 0; j < bArr.length; j++) {
    const key = normText(bArr[j]!.text)
    const list = bIndex.get(key)
    if (list) list.push(j)
    else bIndex.set(key, [j])
  }
  let total = 0
  const cand: number[][] = new Array(aArr.length)
  for (let i = 0; i < aArr.length; i++) {
    const at = aArr[i]!.epochMillis
    const list = bIndex.get(normText(aArr[i]!.text))
    const js: number[] = []
    if (list) {
      for (const j of list) {
        if (Math.abs(bArr[j]!.epochMillis + offset - at) <= tol) js.push(j)
      }
      // 时间近邻优先：patience 放置时同 i 先试更近的 j
      // 同 i 按 j 降序参与 patience 放置：严格递增 j 的链无法取同一 i 的两个
      // 候选（同 i 内 j 递减），保证每行至多配一个锚点
      js.sort((x, y) => y - x)
    }
    total += js.length
    if (total > ANCHOR_CAND_CAP) return null
    cand[i] = js
  }
  // patience LIS（j 严格递增）+ 父指针回溯
  const tails: number[] = [] // tails[p] = 候选事件编号（该长度 LIS 的最小 j 尾）
  const tailJ: number[] = []
  const parent: number[] = []
  const evI: number[] = []
  const evJ: number[] = []
  for (let i = 0; i < aArr.length; i++) {
    for (const j of cand[i]!) {
      let lo = 0
      let hi = tailJ.length
      while (lo < hi) {
        const mid = (lo + hi) >> 1
        if (tailJ[mid]! >= j) hi = mid
        else lo = mid + 1
      }
      const ev = evI.length
      if (lo === tailJ.length) {
        tailJ.push(j)
        tails.push(ev)
      } else {
        tailJ[lo] = j
        tails[lo] = ev
      }
      parent.push(lo > 0 ? tails[lo - 1]! : -1)
      evI.push(i)
      evJ.push(j)
    }
  }
  // 回溯最长匹配链（父指针；每事件放置时父=放置前前一位的尾事件）
  const pairs: [number, number][] = []
  let cur = tails.length ? tails[tails.length - 1]! : -1
  while (cur >= 0) {
    pairs.push([evI[cur]!, evJ[cur]!])
    cur = parent[cur]!
  }
  pairs.reverse()
  return pairs
}

/** 缝隙填充：相邻锚点间的 a-run/b-run 互为差异，时间近邻贪心配成 changed，
 *  落单为 insert-a/insert-b（含首锚点前与末锚点后的两端）。 */
function fillGap(
  aRun: LogLine[],
  bRun: LogLine[],
  offset: number,
  out: ComparePair[],
): void {
  let j = 0
  for (const a of aRun) {
    const at = a.epochMillis
    // b 指针前进条件：当前 b 更早且下一个更近（两侧都按时间升序）
    while (
      j < bRun.length &&
      bRun[j]!.epochMillis + offset < at &&
      j + 1 < bRun.length &&
      Math.abs(bRun[j + 1]!.epochMillis + offset - at) <=
        Math.abs(bRun[j]!.epochMillis + offset - at)
    ) {
      out.push(mkPair(null, bRun[j]!, 'insert-b', null))
      j += 1
    }
    if (j < bRun.length) {
      out.push(mkPair(a, bRun[j]!, 'changed', Math.abs(bRun[j]!.epochMillis + offset - at)))
      j += 1
    } else {
      out.push(mkPair(a, null, 'insert-a', null))
    }
  }
  for (; j < bRun.length; j++) out.push(mkPair(null, bRun[j]!, 'insert-b', null))
}

/** 贪心回退路径（锚点候选超限时）：以 A 为时间轴基准的双指针近邻配对。
 *  与旧版语义一致，额外补上 B 尾部遗留行输出与 op/spans 字段。 */
function greedyAlign(
  aArr: LogLine[],
  bArr: LogLine[],
  tol: number,
  offset: number,
): ComparePair[] {
  const out: ComparePair[] = []
  let j = 0
  for (const a of aArr) {
    while (j < bArr.length && bArr[j]!.epochMillis + offset < a.epochMillis - tol) {
      out.push(mkPair(null, bArr[j]!, 'insert-b', null))
      j += 1
    }
    if (j < bArr.length && Math.abs(bArr[j]!.epochMillis + offset - a.epochMillis) <= tol) {
      let best = j
      let limit = Math.min(bArr.length, j + 8) // 容差窗内向前看 ≤8 个候选挑最近
      for (let k = j; k < limit; k++) {
        if (
          Math.abs(bArr[k]!.epochMillis + offset - a.epochMillis) <
          Math.abs(bArr[best]!.epochMillis + offset - a.epochMillis)
        ) {
          best = k
        }
      }
      const bb = bArr[best]!
      out.push(mkPair(a, bb, 'changed', Math.abs(bb.epochMillis + offset - a.epochMillis)))
      for (let k = j; k < best; k++) out.push(mkPair(null, bArr[k]!, 'insert-b', null))
      j = best + 1
    } else {
      out.push(mkPair(a, null, 'insert-a', null))
    }
  }
  for (; j < bArr.length; j++) out.push(mkPair(null, bArr[j]!, 'insert-b', null))
  return out
}

/**
 * 双会话序列对齐：先锚点（Hunt–Szymanski 全局最优，文本相等 + 时间窗），
 * 再缝隙填充（时间近邻配成 changed，落单 insert-*）。offset 为 B 侧时钟
 * 偏移毫秒（live↔offline 对比时人工对表），bTime = epochMillis + offset。
 */
export function alignCompareLines(
  aArr: LogLine[],
  bArr: LogLine[],
  tol: number,
  offset = 0,
): ComparePair[] {
  const anchors = anchorPairs(aArr, bArr, tol, offset)
  if (anchors === null) return greedyAlign(aArr, bArr, tol, offset)
  const out: ComparePair[] = []
  let ai = 0
  let bj = 0
  for (const [i, jj] of anchors) {
    fillGap(aArr.slice(ai, i), bArr.slice(bj, jj), offset, out)
    out.push(
      mkPair(aArr[i]!, bArr[jj]!, 'equal', Math.abs(bArr[jj]!.epochMillis + offset - aArr[i]!.epochMillis)),
    )
    ai = i + 1
    bj = jj + 1
  }
  fillGap(aArr.slice(ai), bArr.slice(bj), offset, out)
  return out
}

/**
 * 与对侧文本的差异游程（多段）：公共前后缀内的中段走 LCS DP 字符级 diff，
 * 输出交错的高低亮游程；任一侧中段超 DIFF_SEG_MAX 时退回三段游程（原实现）。
 */
export function diffSpans(
  text: string | undefined,
  other: string | undefined,
): CompareDiffSpan[] {
  return diffSpansCore(text ?? '', other ?? '').spansS
}

/**
 * 双侧一次算齐（mkPair 用）：spansS 为第一参侧、spansO 为第二参侧的游程。
 */
function diffSpansCore(
  s: string,
  o: string,
): { spansS: CompareDiffSpan[]; spansO: CompareDiffSpan[] } {
  if (s === o) {
    const t = s ? [eqSpan(s)] : [{ t: '', hl: false }]
    return { spansS: [{ ...t[0]! }], spansO: [{ ...t[0]! }] }
  }
  let p = 0
  const minLen = Math.min(s.length, o.length)
  while (p < minLen && s[p] === o[p]) p += 1
  let suf = 0
  while (suf < minLen - p && s[s.length - 1 - suf] === o[o.length - 1 - suf]) suf += 1
  const midS = s.slice(p, s.length - suf)
  const midO = o.slice(p, o.length - suf)
  const preS = p > 0 ? [eqSpan(s.slice(0, p))] : []
  const preO = p > 0 ? [eqSpan(o.slice(0, p))] : []
  const sufS = suf > 0 ? [eqSpan(s.slice(s.length - suf))] : []
  const sufO = suf > 0 ? [eqSpan(o.slice(o.length - suf))] : []
  if (midS.length > 0 && midO.length > 0 && Math.max(midS.length, midO.length) <= DIFF_SEG_MAX) {
    const [runsS, runsO] = lcsDiffRuns(midS, midO)
    return {
      spansS: [...preS, ...runsS.map((r) => ({ t: r.t, hl: !r.eq })), ...sufS],
      spansO: [...preO, ...runsO.map((r) => ({ t: r.t, hl: !r.eq })), ...sufO],
    }
  }
  // 三段游程回退（超长中段 / 单侧中段为空——后者本就是最优三段）
  const midEnd = s.length - suf
  const side = (mid: string, pre: CompareDiffSpan[], sufSpans: CompareDiffSpan[]) => {
    const out = [...pre]
    if (mid) out.push({ t: mid, hl: true })
    out.push(...sufSpans)
    return out.length ? out : [{ t: '', hl: false }]
  }
  return {
    spansS: side(s.slice(p, midEnd), preS, sufS),
    spansO: side(o.slice(p, o.length - suf), preO, sufO),
  }
}

interface DiffRun {
  t: string
  eq: boolean
}

/** LCS DP：返回 (A 侧游程, B 侧游程)。等值游程 eq=true 两侧文本相同。 */
function lcsDiffRuns(a: string, b: string): [DiffRun[], DiffRun[]] {
  const m = a.length
  const n = b.length
  // f[i][j] = a[i:] 与 b[j:] 的 LCS 长度（滚动不便于前向回溯，直接全表）
  const f = new Int32Array((m + 1) * (n + 1))
  const at = (i: number, j: number) => i * (n + 1) + j
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      f[at(i, j)] =
        a[i] === b[j]
          ? f[at(i + 1, j + 1)]! + 1
          : Math.max(f[at(i + 1, j)]!, f[at(i, j + 1)]!)
    }
  }
  const runsA: DiffRun[] = []
  const runsB: DiffRun[] = []
  let i = 0
  let j = 0
  const push = (runs: DiffRun[], ch: string, eq: boolean) => {
    const last = runs[runs.length - 1]
    if (last && last.eq === eq) last.t += ch
    else runs.push({ t: ch, eq })
  }
  while (i < m && j < n) {
    if (a[i] === b[j]) {
      push(runsA, a[i]!, true)
      push(runsB, b[j]!, true)
      i += 1
      j += 1
    } else if (f[at(i + 1, j)]! >= f[at(i, j + 1)]!) {
      push(runsA, a[i]!, false)
      i += 1
    } else {
      push(runsB, b[j]!, false)
      j += 1
    }
  }
  while (i < m) {
    push(runsA, a[i]!, false)
    i += 1
  }
  while (j < n) {
    push(runsB, b[j]!, false)
    j += 1
  }
  return [runsA, runsB]
}

/** 按方向范围过滤参与对比的行 */
export function scopeCompareLines(
  lines: LogLine[],
  scope: CompareDirScope,
): LogLine[] {
  return lines.filter((l) => (scope === 'rx' ? l.dir === 'rx' : true))
}
