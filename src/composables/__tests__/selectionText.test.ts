// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest'
import {
  cmpBoundaryPoints,
  rangeTextClippedToElement,
  selectionTextWithin,
} from '../selectionText'

// 模拟 LogView 行结构：.log-row > col-no / col-ts / col-dir / col-tx(消息文本)
function makeRow(no: string, text: string) {
  const rowEl = document.createElement('div')
  rowEl.className = 'log-row'
  const colNo = document.createElement('span')
  colNo.className = 'col-no'
  colNo.textContent = no
  const colTs = document.createElement('span')
  colTs.className = 'col-ts'
  colTs.textContent = '12:00:00.000'
  const colDir = document.createElement('span')
  colDir.className = 'col-dir'
  colDir.textContent = 'RX'
  const colTx = document.createElement('span')
  colTx.className = 'col-tx'
  const textNode = document.createTextNode(text)
  colTx.appendChild(textNode)
  rowEl.append(colNo, colTs, colDir, colTx)
  return { rowEl, colNo, colTx, textNode }
}

let rows: ReturnType<typeof makeRow>[]
let root: HTMLElement

beforeEach(() => {
  document.body.innerHTML = ''
  rows = [
    makeRow('1', 'alpha beta'),
    makeRow('2', ''),
    makeRow('3', 'gamma delta'),
    makeRow('4', 'omega'),
  ]
  root = document.createElement('div')
  for (const r of rows) root.appendChild(r.rowEl)
  document.body.appendChild(root)
})

function select(n1: Node, o1: number, n2: Node, o2: number): Selection {
  const sel = window.getSelection()!
  const range = document.createRange()
  range.setStart(n1, o1)
  range.setEnd(n2, o2)
  sel.removeAllRanges()
  sel.addRange(range)
  return sel
}

describe('cmpBoundaryPoints', () => {
  it('同节点按 offset 比大小', () => {
    const t = rows[0]!.textNode
    expect(cmpBoundaryPoints({ node: t, offset: 3 }, { node: t, offset: 5 })).toBe(-1)
    expect(cmpBoundaryPoints({ node: t, offset: 5 }, { node: t, offset: 5 })).toBe(0)
    expect(cmpBoundaryPoints({ node: t, offset: 9 }, { node: t, offset: 5 })).toBe(1)
  })

  it('跨节点按文档序', () => {
    expect(
      cmpBoundaryPoints(
        { node: rows[0]!.textNode, offset: 0 },
        { node: rows[1]!.colTx, offset: 0 },
      ),
    ).toBe(-1)
    expect(
      cmpBoundaryPoints(
        { node: rows[2]!.colTx, offset: 0 },
        { node: rows[0]!.textNode, offset: 0 },
      ),
    ).toBe(1)
  })
})

describe('rangeTextClippedToElement', () => {
  it('选区在元素内部取所选片段', () => {
    const r = document.createRange()
    r.setStart(rows[0]!.textNode, 6)
    r.setEnd(rows[0]!.textNode, 10)
    expect(rangeTextClippedToElement(r, rows[0]!.colTx)).toBe('beta')
  })

  it('选区完整覆盖元素取整段文本', () => {
    const r = document.createRange()
    r.setStart(rows[2]!.colTx, 0)
    r.setEnd(rows[2]!.colTx, 1)
    expect(rangeTextClippedToElement(r, rows[2]!.colTx)).toBe('gamma delta')
  })

  it('无交集返回 null（元素在选区前/后两种方向）', () => {
    const r = document.createRange()
    r.setStart(rows[0]!.textNode, 6)
    r.setEnd(rows[0]!.textNode, 10)
    expect(rangeTextClippedToElement(r, rows[3]!.colTx)).toBeNull() // 在选区后
    const r2 = document.createRange()
    r2.setStart(rows[3]!.textNode, 0)
    r2.setEnd(rows[3]!.textNode, 3)
    expect(rangeTextClippedToElement(r2, rows[0]!.colTx)).toBeNull() // 在选区前
  })
})

describe('selectionTextWithin', () => {
  it('单行划选中段：只复制所选片段', () => {
    const sel = select(rows[0]!.textNode, 6, rows[0]!.textNode, 10)
    expect(selectionTextWithin(sel, root)).toBe('beta')
  })

  it('从行号列拖进消息列：行号/时间戳/方向不参与复制', () => {
    const sel = select(rows[0]!.colNo.firstChild!, 0, rows[0]!.textNode, 5)
    expect(selectionTextWithin(sel, root)).toBe('alpha')
  })

  it('跨多行：边界行取片段、中间行取整段，以 \\n 拼接', () => {
    const sel = select(rows[0]!.textNode, 6, rows[2]!.textNode, 5)
    expect(selectionTextWithin(sel, root)).toBe('beta\n\ngamma')
  })

  it('跨多行到末行：整行选中的行取完整消息', () => {
    const sel = select(rows[0]!.textNode, 6, rows[3]!.textNode, 3)
    expect(selectionTextWithin(sel, root)).toBe('beta\n\ngamma delta\nome')
  })

  it('选区端点贴着某行边缘：不产生多余的空行', () => {
    // 终点恰在第 2 行消息列起点（offset 0），第 2 行是空行——不应出现前导 \n
    const sel = select(rows[0]!.textNode, 6, rows[1]!.colTx, 0)
    expect(selectionTextWithin(sel, root)).toBe('beta')
  })

  it('选区不在日志行内：null', () => {
    const far = document.createElement('div')
    const t = document.createTextNode('outside')
    far.appendChild(t)
    document.body.appendChild(far)
    const sel = select(t, 0, t, 7)
    expect(selectionTextWithin(sel, root)).toBeNull()
  })

  it('折叠选区 / 空选区 / root 为空：null', () => {
    expect(selectionTextWithin(select(rows[0]!.textNode, 3, rows[0]!.textNode, 3), root)).toBeNull()
    expect(selectionTextWithin(null, root)).toBeNull()
    const sel = select(rows[0]!.textNode, 6, rows[0]!.textNode, 10)
    expect(selectionTextWithin(sel, null)).toBeNull()
  })
})
