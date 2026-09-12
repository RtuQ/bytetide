import { markRaw } from 'vue'
import { byteLength } from '../../utils/logLine'
import type { DecodedFrame } from '../../types/parser'
import type { LogLine, RawLogLine } from '../../types'
import type { Session } from './model'

/** 解码帧环形上限（plan-parser-v1：1000 条/会话 FIFO） */
export const MAX_DECODED = 1000

/** 超出视图缓冲上限从头部裁剪最旧行：累计 droppedLines（lifetime）与
 *  evictedPending（视口锚定待补偿，LogView 每渲染批次 takeEvicted 消费）。 */
function evictOverflow(s: Session, cap: number): void {
  if (s.lines.length <= cap) return
  const evicted = s.lines.length - cap
  s.lines = s.lines.slice(evicted)
  s.droppedLines += evicted
  s.evictedPending += evicted
}

/**
 * 事件流入表（原 appendLines 主体）：返回本次带行号的新行（供告警/回复等拿到 no）。
 * 补拉水位去重：事件队列晚到的行若已被自愈补拉插入（epoch <= 水位）直接丢弃，
 * 防同一行出现两次（否则缓冲加速膨胀、赤字自我强化）。
 * cap 由调用方显式传入（门面按 Math.max(1, logConfig.viewBufCap) 计算）。
 * 行对象 markRaw：日志行创建后不可变，跳过 Vue 深层 Proxy 包装——长跑时多条
 * 全量扫描（高亮/统计/过滤）以原生对象速度进行。
 */
export function appendLinesInto(s: Session, raw: RawLogLine[], cap: number): LogLine[] {
  if (raw.length === 0) return []
  const arr = s.pulledThrough > 0 ? raw.filter((r) => r.epochMillis > s.pulledThrough) : raw
  if (arr.length === 0) return []
  // 用 concat 产生新数组引用，保证虚拟滚动器感知变化
  const fresh: LogLine[] = arr.map((r) => markRaw({ no: ++s.lineCounter, ...r }))
  s.lines = s.lines.concat(fresh)
  evictOverflow(s, cap)
  return fresh
}

/**
 * 拉模型摄取（原 appendPulled 主体）：后端 ring 按 `no` 游标拉到的行一次性入表。
 * 游标（pullNo）是不重不漏的唯一真相——调用方（拉取循环）保证行序按 no 升序；
 * 防御性过滤 ringNo <= pullNo 的重复行，其余全部入表并推进游标。
 * ring 缺口检测在去重过滤之前：首行 no 越过 pullNo+1 说明中间有行在 ring 容量
 * 窗口内未来得及拉取就被覆盖（前端停顿过长时发生）。ring no 从 1 起单调递增、
 * pullNo 初值 0，正常连续拉取不会误报。
 * 返回本次插入的行（供 autoReply/alerts/tally 拿 no 与内容）。
 */
export function appendPulledInto(
  s: Session,
  lines: (RawLogLine & { ringNo: number })[],
  cap: number,
): LogLine[] {
  if (lines.length === 0) return []
  if (lines[0]!.ringNo > s.pullNo + 1) {
    s.ringDropped += lines[0]!.ringNo - (s.pullNo + 1)
  }
  const arr = lines.filter((l) => l.ringNo > s.pullNo)
  if (arr.length === 0) return []
  // 用 concat 产生新数组引用，保证虚拟滚动器感知变化
  // rn=后端 ring no（告警命中事件回查 UI 行号用；不入显示列）
  const fresh: LogLine[] = arr.map((r) =>
    markRaw({
      no: ++s.lineCounter,
      ts: r.ts,
      dir: r.dir,
      text: r.text,
      bytes: r.bytes,
      epochMillis: r.epochMillis,
      rn: r.ringNo,
    }),
  )
  s.lines = s.lines.concat(fresh)
  evictOverflow(s, cap)
  s.pullNo = arr[arr.length - 1]!.ringNo
  return fresh
}

/**
 * 翻页补旧行（方案 B，原 prependBackfill 主体）：视图缓冲裁掉的行若仍在后端
 * ring 窗口内，上滑时回补到头部。沿用被裁前的原行号（no = headNo-k+i 连续延伸
 * ——no 连续性是 SearchPanel O(1) 映射与书签/跳转的前提），不推进
 * lineCounter/pullNo，不动 droppedLines/ringDropped/evictedPending。仅 live
 * 会话且视图头属当前 ring 纪元（head.no > reconnectNo）时可补；返回本次回补的行。
 */
export function prependBackfillInto(
  s: Session,
  lines: (RawLogLine & { ringNo: number })[],
): LogLine[] {
  if (s.kind !== 'live' || lines.length === 0) return []
  const head = s.lines[0]
  if (!head || head.no <= s.reconnectNo) return []
  // 防御性去重：调用方以 head.rn 为 beforeNo 拉取，正常不会带回 ≥ 它的行
  const headRn = head.rn ?? Number.MAX_SAFE_INTEGER
  const arr = lines.filter((l) => l.ringNo < headRn)
  if (arr.length === 0) return []
  const k = arr.length
  const base = head.no - k
  const fresh: LogLine[] = arr.map((r, i) =>
    markRaw({
      no: base + i,
      ts: r.ts,
      dir: r.dir,
      text: r.text,
      bytes: r.bytes,
      epochMillis: r.epochMillis,
      rn: r.ringNo,
    }),
  )
  // concat 产生新数组引用，保证虚拟滚动器感知变化；暂不回裁容量——
  // 尾部是 live 边缘不可裁（pullNo 已越过），下一批 append 会按 cap 从头部重新收敛
  s.lines = fresh.concat(s.lines)
  s.backfillTotal += k
  s.backfillPending = s.backfillPending.concat(fresh)
  return fresh
}

/** 视口锚定补偿：返回该会话累计的被裁行数并清零（未知会话由门面挡）。
 *  LogView 跟随 watcher 每个渲染批次消费一次，取走即清零防同 tick 多批次漏计。 */
export function takeEvictedFrom(s: Session): number {
  const n = s.evictedPending
  s.evictedPending = 0
  return n
}

/** 视口锚定补偿（头部插入方向）：返回本批回补的行并清空暂存。
 *  LogView 由此统计通过过滤链的渲染行数，做 scrollTop 等量下移补偿。 */
export function takeBackfilledFrom(s: Session): LogLine[] {
  if (s.backfillPending.length === 0) return []
  const out = s.backfillPending
  s.backfillPending = []
  return out
}

/**
 * 累计 RX/TX 字节与行数（lifetime，随缓冲裁剪不回退）；与入表分离，不改其行为。
 * 优先原始字节（后端仅在该行含非法 UTF-8 时附带 bytes）；无 bytes 才按文本
 * UTF-8 编码——二进制行若把 lossy 文本（U+FFFD）再编码必算错。
 */
export function tallyBytesInto(s: Session, raw: RawLogLine[]): void {
  if (raw.length === 0) return
  for (const r of raw) {
    const bytes = byteLength(r)
    if (r.dir === 'rx') {
      s.rxBytes = (s.rxBytes ?? 0) + bytes
      s.rxLines = (s.rxLines ?? 0) + 1
    } else {
      s.txBytes = (s.txBytes ?? 0) + bytes
      s.txLines = (s.txLines ?? 0) + 1
    }
  }
}

/** 解析引擎落表：解码帧追加（元素 markRaw + FIFO；replace=true 用于回溯整表替换）。
 *  200ms 节流批量由调用方（useParserEngine）负责，这里只管入表。 */
export function applyDecodedInto(s: Session, frames: DecodedFrame[], replace = false): void {
  const fresh = frames.map((f) => markRaw({ ...f }))
  s.decoded = replace ? fresh : s.decoded.concat(fresh)
  if (s.decoded.length > MAX_DECODED) {
    s.decoded = s.decoded.slice(s.decoded.length - MAX_DECODED)
  }
}

/** 清空解码帧（卸载/停用回溯前重置） */
export function resetDecodedOf(s: Session): void {
  s.decoded = []
}
