import { describe, it, expect } from 'vitest'
import { parseAnsi, stripAnsi } from '../useAnsi'

describe('parseAnsi 基础', () => {
  it('无 ESC 快速路径：单段无样式', () => {
    const runs = parseAnsi('plain log line')
    expect(runs).toEqual([{ text: 'plain log line', style: null }])
  })

  it('空文本返回单段', () => {
    expect(parseAnsi('')).toEqual([{ text: '', style: null }])
  })

  it('SGR 32 前景绿持续到行尾', () => {
    const runs = parseAnsi('\x1b[32m[12:00:01.000 I/main: firmware v2.1.0 started]')
    expect(runs).toHaveLength(1)
    expect(runs[0]!.style).toEqual({ fg: 'var(--ansi-green)' })
  })

  it('仿真实日志格式：信息绿、重置后错误红（每行独立解析）', () => {
    const info = parseAnsi('\x1b[32m[10:20:30.100 I/storage: card mounted at /data]')
    const error = parseAnsi('\x1b[0m\x1b[31m[10:20:30.101 E/storage: read failed at sector 42]')
    expect(info[0]!.style!.fg).toBe('var(--ansi-green)')
    expect(error[0]!.style!.fg).toBe('var(--ansi-red)')
  })

  it('0m 重置后再次 32m，三段样式各自独立', () => {
    const runs = parseAnsi('\x1b[32mAAA\x1b[0mBBB\x1b[32mCCC')
    expect(runs.map((r) => r.text)).toEqual(['AAA', 'BBB', 'CCC'])
    expect(runs.map((r) => r.style?.fg ?? null)).toEqual([
      'var(--ansi-green)',
      null,
      'var(--ansi-green)',
    ])
  })

  it('行内状态延续：31 后 32 各管一段', () => {
    const runs = parseAnsi('\x1b[31mR\x1b[32mG')
    expect(runs.map((r) => r.text)).toEqual(['R', 'G'])
    expect(runs[0]!.style!.fg).toBe('var(--ansi-red)')
    expect(runs[1]!.style!.fg).toBe('var(--ansi-green)')
  })
})

describe('parseAnsi SGR 参数', () => {
  it('组合参数 1;32 → 加粗+绿', () => {
    const runs = parseAnsi('\x1b[1;32mgo')
    expect(runs[0]!.style).toEqual({ fg: 'var(--ansi-green)', bold: true })
  })

  it('22 取消加粗但保留前景色', () => {
    const runs = parseAnsi('\x1b[1;31mx\x1b[22my')
    expect(runs[0]!.style!.bold).toBe(true)
    expect(runs[1]!.style!.fg).toBe('var(--ansi-red)')
    expect(runs[1]!.style!.bold).toBeUndefined()
  })

  it('39/49 恢复默认前景/背景', () => {
    const runs = parseAnsi('\x1b[31;41mx\x1b[39;49my')
    expect(runs[0]!.style!.fg).toBe('var(--ansi-red)')
    expect(runs[0]!.style!.bg).toBe('var(--ansi-red)')
    expect(runs[1]!.style).toBeNull()
  })

  it('\\x1b[m 等价重置', () => {
    const runs = parseAnsi('\x1b[31mx\x1b[my')
    expect(runs[1]!.style).toBeNull()
  })

  it('90-97 亮色前景 / 100-107 亮色背景', () => {
    const runs = parseAnsi('\x1b[92ma\x1b[101mb')
    expect(runs[0]!.style!.fg).toBe('var(--ansi-bright-green)')
    expect(runs[1]!.style!.bg).toBe('var(--ansi-bright-red)')
  })

  it('斜体/下划线等不支持参数被忽略，不影响样式', () => {
    const runs = parseAnsi('\x1b[3;4;32mgo')
    expect(runs[0]!.style).toEqual({ fg: 'var(--ansi-green)' })
  })
})

describe('parseAnsi 扩展色', () => {
  it('256 色 196 → 纯红 rgb', () => {
    expect(parseAnsi('\x1b[38;5;196mx')[0]!.style!.fg).toBe('rgb(255,0,0)')
  })

  it('256 色边界：16 → 黑，231 → 白，232/255 灰阶', () => {
    expect(parseAnsi('\x1b[38;5;16mx')[0]!.style!.fg).toBe('rgb(0,0,0)')
    expect(parseAnsi('\x1b[38;5;231mx')[0]!.style!.fg).toBe('rgb(255,255,255)')
    expect(parseAnsi('\x1b[38;5;232mx')[0]!.style!.fg).toBe('rgb(8,8,8)')
    expect(parseAnsi('\x1b[38;5;255mx')[0]!.style!.fg).toBe('rgb(238,238,238)')
  })

  it('256 色 0-15 引用主题 token', () => {
    expect(parseAnsi('\x1b[38;5;9mx')[0]!.style!.fg).toBe('var(--ansi-bright-red)')
  })

  it('48;5;N 设为背景色', () => {
    expect(parseAnsi('\x1b[48;5;21mx')[0]!.style!.bg).toBe('rgb(0,0,255)')
  })

  it('真彩 38;2;R;G;B 透传', () => {
    expect(parseAnsi('\x1b[38;2;10;20;30mx')[0]!.style!.fg).toBe('rgb(10,20,30)')
  })

  it('真彩 48;2;… 设为背景', () => {
    expect(parseAnsi('\x1b[48;2;1;2;3mx')[0]!.style!.bg).toBe('rgb(1,2,3)')
  })

  it('冒号分隔的 38:5:N 等价分号', () => {
    expect(parseAnsi('\x1b[38:5:196mx')[0]!.style!.fg).toBe('rgb(255,0,0)')
  })

  it('扩展色子参数缺失时放弃剩余参数，不误吞正文', () => {
    const runs = parseAnsi('\x1b[38;5mok')
    expect(runs[0]!.text).toBe('ok')
  })
})

describe('parseAnsi 非 SGR 与畸形序列', () => {
  it('光标移动/清屏等 CSI 整体吞掉', () => {
    const runs = parseAnsi('\x1b[2J\x1b[1;1Hhello')
    expect(runs).toEqual([{ text: 'hello', style: null }])
  })

  it('带参数的非 SGR 序列不改变样式', () => {
    const runs = parseAnsi('\x1b[31m\x1b[10;20fa\x1b[0mb')
    expect(runs.map((r) => r.text)).toEqual(['a', 'b'])
    expect(runs[0]!.style!.fg).toBe('var(--ansi-red)')
    expect(runs[1]!.style).toBeNull()
  })

  it('游离 ESC 被吞', () => {
    expect(parseAnsi('a\x1bb')).toEqual([{ text: 'ab', style: null }])
  })

  it('行尾未闭合 CSI 段被吞', () => {
    expect(parseAnsi('abc\x1b[32')).toEqual([{ text: 'abc', style: null }])
  })
})

describe('stripAnsi', () => {
  it('移除颜色序列保留正文', () => {
    expect(stripAnsi('\x1b[32mhello\x1b[0m world')).toBe('hello world')
  })

  it('无 ESC 原样返回', () => {
    expect(stripAnsi('plain')).toBe('plain')
  })

  it('移除非 SGR CSI 与游离 ESC', () => {
    expect(stripAnsi('a\x1b[2Jb\x1bc')).toBe('abc')
  })

  it('移除行尾未闭合段', () => {
    expect(stripAnsi('abc\x1b[32')).toBe('abc')
  })
})
