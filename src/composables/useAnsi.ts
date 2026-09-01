/**
 * ANSI 转义序列解析：把含 SGR 颜色码的行文本切成带样式的游程，供 LogView 渲染。
 * 支持 0 重置 / 1 加粗 / 22 取消加粗 / 39·49 默认色 / 30-37·90-97 前景 /
 * 40-47·100-107 背景 / 38·48 的 5;N（256 色）与 2;R;G;B（真彩），`;` 与 `:` 分隔符均认；
 * 其余参数（斜体/下划线/闪烁等）与光标移动等非 SGR CSI 序列整体吞掉不显示。
 *
 * 基础 16 色输出 var(--ansi-*) token 引用（随深/浅主题自动切换）；
 * 256 色 cube/灰阶与真彩为设备下发的数据驱动颜色，用 rgb() 内联，不走设计 token。
 *
 * 限制：每行独立解析（嵌入式日志惯例每行自带 reset）；跨行颜色状态不维护。
 */

export interface AnsiStyle {
  fg?: string
  bg?: string
  bold?: boolean
}

export interface AnsiRun {
  text: string
  style: AnsiStyle | null
}

const ANSI_NAMES = ['black', 'red', 'green', 'yellow', 'blue', 'magenta', 'cyan', 'white'] as const

const BASE = ANSI_NAMES.map((c) => `var(--ansi-${c})`)
const BRIGHT = ANSI_NAMES.map((c) => `var(--ansi-bright-${c})`)

// CSI 序列：ESC [ 参数(数字/;/:) 终止字节(字母)
const CSI_RE = /\x1b\[([0-9;:]*)([A-Za-z])/g
// 游离 ESC（后面不是 [），以及行尾未闭合的 CSI 段（串口分包可能截断序列）
const STRAY_ESC_RE = /\x1b(?!\[)/g
const UNTERMINATED_CSI_RE = /\x1b\[[0-9;:]*$/

const clamp255 = (n: number) => Math.min(Math.max(n, 0), 255)

/** xterm 256 色号 → CSS 颜色：0-15 引用 token，16-231 6×6×6 cube，232-255 灰阶 */
function ansi256(n: number): string {
  if (!Number.isFinite(n)) return ''
  n = clamp255(Math.trunc(n))
  if (n < 16) return n < 8 ? BASE[n] : BRIGHT[n - 8]
  if (n < 232) {
    const i = n - 16
    const ch = (c: number) => (c === 0 ? 0 : c * 40 + 55)
    return `rgb(${ch(Math.floor(i / 36))},${ch(Math.floor((i % 36) / 6))},${ch(i % 6)})`
  }
  const g = 8 + (n - 232) * 10
  return `rgb(${g},${g},${g})`
}

function applySgr(paramStr: string, st: AnsiStyle): void {
  const reset = () => {
    st.fg = undefined
    st.bg = undefined
    st.bold = false
  }
  if (paramStr === '') {
    reset()
    return
  }
  const p = paramStr.split(/[;:]/)
  for (let i = 0; i < p.length; i++) {
    const v = p[i] === '' ? 0 : Number(p[i])
    if (!Number.isFinite(v)) continue
    if (v === 0) reset()
    else if (v === 1) st.bold = true
    else if (v === 22) st.bold = false
    else if (v === 39) st.fg = undefined
    else if (v === 49) st.bg = undefined
    else if (v >= 30 && v <= 37) st.fg = BASE[v - 30]
    else if (v >= 90 && v <= 97) st.fg = BRIGHT[v - 90]
    else if (v >= 40 && v <= 47) st.bg = BASE[v - 40]
    else if (v >= 100 && v <= 107) st.bg = BRIGHT[v - 100]
    else if (v === 38 || v === 48) {
      // 扩展色：38;5;N / 38;2;R;G;B（48 同构为背景）；子参数结构未知则放弃剩余参数
      const mode = p[i + 1]
      if (mode === '5') {
        const c = ansi256(Number(p[i + 2]))
        if (!c) break
        if (v === 38) st.fg = c
        else st.bg = c
        i += 2
      } else if (mode === '2') {
        const r = clamp255(Number(p[i + 2]))
        const g = clamp255(Number(p[i + 3]))
        const b = clamp255(Number(p[i + 4]))
        if (!Number.isFinite(r) || !Number.isFinite(g) || !Number.isFinite(b)) break
        const c = `rgb(${r},${g},${b})`
        if (v === 38) st.fg = c
        else st.bg = c
        i += 4
      } else break
    }
  }
}

function hasStyle(st: AnsiStyle): boolean {
  return st.fg !== undefined || st.bg !== undefined || st.bold === true
}

/** 行文本 → 样式游程；无 ESC 走快速路径。仅 SGR（终止字节 m）改变样式 */
export function parseAnsi(text: string): AnsiRun[] {
  if (text.indexOf('\x1b') === -1) return [{ text, style: null }]
  // 先吞掉游离 ESC 与行尾未闭合段，避免其参数数字被当作正文渲染
  const src = text.replace(UNTERMINATED_CSI_RE, '').replace(STRAY_ESC_RE, '')
  const runs: AnsiRun[] = []
  const style: AnsiStyle = {}
  // 快照稀疏化：只带已设置的属性（bold 关闭即省略，而非 false）
  const snapshot = (): AnsiStyle | null => {
    if (!hasStyle(style)) return null
    const s: AnsiStyle = {}
    if (style.fg !== undefined) s.fg = style.fg
    if (style.bg !== undefined) s.bg = style.bg
    if (style.bold === true) s.bold = true
    return s
  }
  let pos = 0
  const push = (end: number) => {
    if (end > pos) runs.push({ text: src.slice(pos, end), style: snapshot() })
  }
  CSI_RE.lastIndex = 0
  let m: RegExpExecArray | null
  while ((m = CSI_RE.exec(src)) !== null) {
    push(m.index)
    pos = m.index + m[0].length
    if (m[2] === 'm') applySgr(m[1], style)
  }
  push(src.length)
  return runs
}

/** 移除 ANSI 序列（估宽等需要显示长度的场合） */
export function stripAnsi(text: string): string {
  if (text.indexOf('\x1b') === -1) return text
  return text
    .replace(UNTERMINATED_CSI_RE, '')
    .replace(/\x1b\[([0-9;:]*)[A-Za-z]/g, '')
    .replace(STRAY_ESC_RE, '')
}
