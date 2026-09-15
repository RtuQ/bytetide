/**
 * 选区感知的右键复制（LogView 行菜单「复制文本」用）。
 *
 * 菜单过去固定复制右键所在整行，无视用户划选——划选一行的一部分复制出
 * 整行、划选多行只复制右键指向的那一行。这里把 DOM 选区按行裁剪到消息列
 * （.log-row .col-tx），跨行按渲染顺序以 \n 拼接，做到「所见即所复制」：
 * 边界行取划到的片段、中间行取整段消息文本，行号/时间戳/方向列不参与。
 *
 * 调用方必须在打开菜单的瞬间把结果快照为字符串——点击菜单项时浏览器
 * 已经清掉 DOM 选区，事后读不回来。
 */

export interface BoundaryPoint {
  node: Node
  offset: number
}

/** child 在 parent 直接子节点中的下标（调用方保证是直接子节点） */
function childIndex(parent: Node, child: Node): number {
  let i = 0
  for (let c = parent.firstChild; c; c = c.nextSibling) {
    if (c === child) return i
    i++
  }
  return -1
}

/** node 在 ancestor 内部时，其子树挂在 ancestor 下的那个直接子节点 */
function topChildUnder(ancestor: Node, node: Node): Node {
  let n = node
  while (n.parentNode !== ancestor) n = n.parentNode as Node
  return n
}

/**
 * 比较两个边界点的文档序先后：a 在 b 前 → -1，相等 → 0，a 在 b 后 → 1。
 * 同树节点用 compareDocumentPosition；祖先-后代对（offset 语义失效）按
 * DOM 规范用「包含方 point 落在哪 个直接子节点边界」细化。
 */
export function cmpBoundaryPoints(a: BoundaryPoint, b: BoundaryPoint): number {
  if (a.node === b.node) return a.offset === b.offset ? 0 : a.offset < b.offset ? -1 : 1
  const pos = a.node.compareDocumentPosition(b.node)
  // 浏览器语义：CONTAINS(8) ⟹ b 是 a 的祖先（伴随 PRECEDING）；
  // CONTAINED_BY(16) ⟹ b 是 a 的后代（伴随 FOLLOWING）。方向位对祖先-后代对
  // 不可信，包含关系必须先按标志分派，再用子节点边界细化 offset 比较。
  if (pos & Node.DOCUMENT_POSITION_CONTAINS) {
    // b 是 a 的祖先：(b,offset) 落在 a 所在子节点边界的哪侧决定先后
    const idx = childIndex(b.node, topChildUnder(b.node, a.node))
    return b.offset <= idx ? 1 : -1
  }
  if (pos & Node.DOCUMENT_POSITION_CONTAINED_BY) {
    // b 是 a 的后代：(a,offset) 落在 b 所在子节点边界的哪侧决定先后
    const idx = childIndex(a.node, topChildUnder(a.node, b.node))
    return a.offset <= idx ? -1 : 1
  }
  if (pos & Node.DOCUMENT_POSITION_FOLLOWING) return -1
  if (pos & Node.DOCUMENT_POSITION_PRECEDING) return 1
  return 0 // 断开树等病态情形按相等处理
}

/**
 * 选区 range 与元素内容交集的文本；无交集返回 null。
 * 端点比较用含边界语义：选区端点恰好贴在元素边缘时得到空串（上层裁掉
 * 首尾空片段），而不是误吞/误丢该行。
 */
export function rangeTextClippedToElement(range: Range, el: Element): string | null {
  const elStart: BoundaryPoint = { node: el, offset: 0 }
  const elEnd: BoundaryPoint = { node: el, offset: el.childNodes.length }
  const selStart: BoundaryPoint = { node: range.startContainer, offset: range.startOffset }
  const selEnd: BoundaryPoint = { node: range.endContainer, offset: range.endOffset }
  if (cmpBoundaryPoints(selEnd, elStart) < 0) return null // 元素整体在选区后
  if (cmpBoundaryPoints(elEnd, selStart) < 0) return null // 元素整体在选区前
  const start = cmpBoundaryPoints(elStart, selStart) < 0 ? selStart : elStart
  const end = cmpBoundaryPoints(selEnd, elEnd) < 0 ? selEnd : elEnd
  const r = document.createRange()
  r.setStart(start.node, start.offset)
  r.setEnd(end.node, end.offset)
  return r.toString()
}

/**
 * 读取选区文本，限定 root 内日志行的消息列（.log-row .col-tx）。
 * 无选区/折叠选区/选区不落在任何日志行 → null（调用方回落整行复制）。
 * 首尾裁掉空片段：选区端点贴着某行边缘时不产生多余的空行，行中的空行保留。
 */
export function selectionTextWithin(sel: Selection | null, root: Element | null): string | null {
  if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !root) return null
  const range = sel.getRangeAt(0)
  const parts: string[] = []
  for (const row of root.querySelectorAll('.log-row')) {
    const tx = row.querySelector('.col-tx')
    if (!tx) continue
    const t = rangeTextClippedToElement(range, tx)
    if (t !== null) parts.push(t)
  }
  while (parts.length > 0 && parts[0] === '') parts.shift()
  while (parts.length > 0 && parts[parts.length - 1] === '') parts.pop()
  return parts.length > 0 ? parts.join('\n') : null
}
