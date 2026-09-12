import { readFileSync } from 'node:fs'
import { describe, it, expect } from 'vitest'
import { parseTsToMs, parseLogFile } from '../useLogParser'
import type { RawLogLine } from '../../types'

/**
 * TSV 会话录制/现场捕获黄金样本（testdata/protocol/tsv-v1.log，Rust 离线读取
 * 同源消费）：覆盖 # 注释头、CRLF/LF（仓库 eol=lf 规范化，CRLF 由消费方运行时
 * 全文转换验证）、坏行、非法 dir 归一、tab 含于 text、ts 失败 epoch 回退、
 * 二进制 lossy U+FFFD。
 */
const TSV_FIXTURE = readFileSync(
  new URL('../../../testdata/protocol/tsv-v1.log', import.meta.url),
  'utf8',
)

describe('parseTsToMs', () => {
  it('parses HH:MM:SS.mmm', () => {
    expect(parseTsToMs('14:30:25.123')).toBe(52_225_123)
  })
  it('parses without milliseconds (-> .000)', () => {
    expect(parseTsToMs('14:30:25')).toBe(52_225_000)
  })
  it('pads fractional .5 to 500ms', () => {
    // 1*3600000 + 2*60000 + 3*1000 + 500（分秒须两位，与真实日志格式一致）
    expect(parseTsToMs('01:02:03.5')).toBe(3_723_500)
  })
  it('rejects out-of-range hour', () => {
    expect(parseTsToMs('25:00:00.000')).toBe(-1)
  })
  it('rejects out-of-range minute', () => {
    expect(parseTsToMs('14:60:00.000')).toBe(-1)
  })
  it('rejects garbage / empty', () => {
    expect(parseTsToMs('abc')).toBe(-1)
    expect(parseTsToMs('')).toBe(-1)
  })
})

describe('parseLogFile', () => {
  it('splits only on first two tabs (text may contain tabs)', () => {
    const r = parseLogFile('14:30:25.123\tRX\thello\tworld')
    expect(r.total).toBe(1)
    expect(r.errors).toBe(0)
    expect(r.lines[0]).toEqual({
      ts: '14:30:25.123',
      dir: 'rx',
      text: 'hello\tworld',
      bytes: null,
      epochMillis: 52_225_123,
    } satisfies RawLogLine)
  })

  it('normalizes dir (TX->tx, anything else->rx) and keeps bytes null', () => {
    const content = ['00:00:00.000\ttx\tA', '00:00:00.000\tXYZ\tB', '00:00:00.000\t TX \tC'].join('\n')
    const r = parseLogFile(content)
    expect(r.lines.map((l) => l.dir)).toEqual(['tx', 'rx', 'tx'])
    expect(r.lines.every((l) => l.bytes === null)).toBe(true)
  })

  it('falls back epochMillis to seq when ts unparseable', () => {
    // line0: bad ts -> epoch = seq 0；line1: real ts .005 -> epoch = 5（非 seq 1）
    const r = parseLogFile('badts\tRX\tfoo\n00:00:00.005\tRX\tbar')
    expect(r.lines[0].ts).toBe('badts')
    expect(r.lines[0].epochMillis).toBe(0) // seq 回退
    expect(r.lines[1].epochMillis).toBe(5) // 真实 ms
  })

  it('skips empty lines and counts no-tab lines as errors', () => {
    const r = parseLogFile('00:00:00.000\tRX\ta\n\nnotabline\n00:00:00.000\tRX\tb')
    expect(r.total).toBe(2) // 空行 + 无 tab 行不计入
    expect(r.errors).toBe(1) // 仅 notabline
  })

  it('skips # comment lines (capture file headers)', () => {
    const content = [
      '# bytetide-capture v1 trigger=keyword rule=OVERTEMP at_ms=1757424631123 at=2026-09-09T21:30:31.123+08:00',
      '# 注意：更早的行已超出 ring 窗口，部分前置现场缺失',
      '00:00:00.000\tRX\treal line',
    ].join('\n')
    const r = parseLogFile(content)
    expect(r.total).toBe(1)
    expect(r.errors).toBe(0)
    expect(r.lines[0]!.text).toBe('real line')
  })

  it('caps to 50000 keeping the last', () => {
    const one = '00:00:00.000\tRX\tx'
    const r = parseLogFile(Array(50_001).fill(one).join('\n'))
    expect(r.total).toBe(50_001)
    expect(r.lines.length).toBe(50_000)
  })

  it('golden fixture tsv-v1.log：注释头/坏行/dir 归一/tab text/epoch 回退/lossy U+FFFD', () => {
    const r = parseLogFile(TSV_FIXTURE)
    expect(r.total).toBe(9)
    expect(r.errors).toBe(1) // 仅无 tab 行
    const [hello, csq, lower, padded, tabbed, garbageTs, lossy, crlf, lf] = r.lines
    expect(hello).toEqual({
      ts: '00:00:01.000',
      dir: 'rx',
      text: 'hello',
      bytes: null,
      epochMillis: 1000,
    } satisfies RawLogLine)
    expect(csq!.dir).toBe('tx')
    expect(lower!.dir).toBe('rx') // 小写 rx 保持
    expect(padded!.dir).toBe('tx') // ' TX ' 归一 tx
    expect(tabbed!.text).toBe('text may contain\ta tab here') // 仅按前两个 tab 切
    expect(garbageTs!.ts).toBe('garbage-ts')
    expect(garbageTs!.epochMillis).toBe(5) // ts 解析失败 → 行序号回退
    expect(lossy!.text).toBe('binary lossy: \uFFFD\uFFFD OK\uFFFD') // 二进制 lossy 占位
    expect(lossy!.bytes).toBeNull() // 有损 TSV 不含原始字节
    expect(crlf!.epochMillis).toBe(8000)
    expect(lf!.epochMillis).toBe(8250)
  })

  it('golden fixture 全文 CRLF 变体与 LF 结果一致（.gitattributes 规范化 LF 后仍覆盖 CRLF 语义）', () => {
    const asCrlf = TSV_FIXTURE.replace(/\n/g, '\r\n')
    const crlf = parseLogFile(asCrlf)
    const lf = parseLogFile(TSV_FIXTURE)
    expect(crlf).toEqual(lf)
    // ts 字段无残留 \r（若按裸 \n 切分，CRLF 行的 ts 会带 \r 且解析失败）
    expect(crlf.lines.every((l) => !l.ts.endsWith('\r'))).toBe(true)
    expect(crlf.lines[7]!.epochMillis).toBe(8000)
  })
})
