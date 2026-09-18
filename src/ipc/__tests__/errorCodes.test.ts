// 错误码契约单测：CMD_ERROR_CODES 全集的形状约束（与 Rust 侧 cmd_err/err_msg 对齐）。
import { describe, expect, it } from 'vitest'

import { CMD_ERROR_CODES } from '../errorCodes'
import { zhCN } from '../../i18n/messages/zh-CN'

describe('CMD_ERROR_CODES', () => {
  it('含 generic 兜底码', () => {
    expect(CMD_ERROR_CODES).toContain('generic')
  })

  it('码全部为 snake_case 且非空', () => {
    for (const code of CMD_ERROR_CODES) {
      expect(code).toMatch(/^[a-z][a-z0-9_]*$/)
    }
  })

  it('无重复码', () => {
    expect(new Set(CMD_ERROR_CODES).size).toBe(CMD_ERROR_CODES.length)
  })
})

describe('errors 词典与错误码全集一一对应', () => {
  const dict = zhCN as Record<string, string>
  const entryKeys = Object.keys(dict).filter((k) => k.startsWith('errors.'))

  it('每个码都有 errors.<code> 词条且非空白', () => {
    for (const code of CMD_ERROR_CODES) {
      const v = dict[`errors.${code}`]
      expect(v, `缺词条 errors.${code}`).toBeDefined()
      expect(v!.trim()).not.toBe('')
    }
  })

  it('词典无游离键（errors.* 全部对应全集内的码）', () => {
    const valid = new Set(CMD_ERROR_CODES.map((c) => `errors.${c}`))
    expect(entryKeys.filter((k) => !valid.has(k))).toEqual([])
  })
})
