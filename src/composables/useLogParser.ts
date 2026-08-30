import type { Dir, ParsedLog, RawLogLine } from '../types'

/** 前端环形缓冲上限（与 store 一致） */
const MAX_LINES = 50000

/**
 * 把行时间戳串解析为毫秒数（用于 Δ 时间）。
 * 默认 ts 格式为 %h:%m:%s.%t 即 HH:MM:SS.mmm；解析失败返回 -1。
 * 同一文件内均为同日，故毫秒数之差即为真实 Δ。
 */
const TS_RE = /^(\d{1,2}):(\d{2}):(\d{2})(?:\.(\d{1,3}))?$/
export function parseTsToMs(ts: string): number {
  const m = TS_RE.exec(ts.trim())
  if (!m) return -1
  const h = Number(m[1])
  const mi = Number(m[2])
  const s = Number(m[3])
  const ms = m[4] ? Number(m[4].padEnd(3, '0')) : 0
  if (h > 23 || mi > 59 || s > 59) return -1
  return h * 3600000 + mi * 60000 + s * 1000 + ms
}

/**
 * 解析自动保存的会话日志（TSV：ts \t RX/TX \t text）。
 * - dir：RX/TX -> rx/tx，非法视为 rx
 * - bytes：恒 null（自动日志有损，不含原始字节；二进制源不可恢复）
 * - epochMillis：由 ts 解析为毫秒数；解析失败退化为行序号（保持单调）
 * - 跳过字段不足的非法行，计入 errors
 * - 截断到 MAX_LINES（保留最后）
 */
export function parseLogFile(content: string): ParsedLog {
  const lines: RawLogLine[] = []
  let errors = 0
  let seq = 0
  const rawLines = content.split(/\r?\n/)
  for (const line of rawLines) {
    if (line === '') continue
    // text 内可能含 tab，故仅按前两个字 tab 切分
    const first = line.indexOf('\t')
    if (first < 0) {
      errors += 1
      continue
    }
    const second = line.indexOf('\t', first + 1)
    let ts: string
    let dirStr: string
    let text: string
    if (second < 0) {
      ts = line.slice(0, first)
      dirStr = line.slice(first + 1)
      text = ''
    } else {
      ts = line.slice(0, first)
      dirStr = line.slice(first + 1, second)
      text = line.slice(second + 1)
    }
    const dir: Dir = dirStr.trim().toUpperCase() === 'TX' ? 'tx' : 'rx'
    const ms = parseTsToMs(ts)
    const epochMillis = ms >= 0 ? ms : seq
    lines.push({ ts, dir, text, bytes: null, epochMillis })
    seq += 1
  }
  const total = lines.length
  const capped = total > MAX_LINES ? lines.slice(total - MAX_LINES) : lines
  return { lines: capped, total, errors }
}
