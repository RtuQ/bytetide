import { describe, it, expect } from 'vitest'
import {
  isNewer,
  isCheckDue,
  isDismissed,
  normalizeTag,
  parseRelease,
} from '../useUpdateChecker'

const HOUR = 3600_000

describe('normalizeTag', () => {
  it('去掉小写/大写 v 前缀', () => {
    expect(normalizeTag('v0.2.0')).toBe('0.2.0')
    expect(normalizeTag('V1.2.3')).toBe('1.2.3')
  })
  it('无前缀原样返回', () => {
    expect(normalizeTag('0.2.0')).toBe('0.2.0')
  })
})

describe('isNewer', () => {
  it('补丁/次版本号更新 → true', () => {
    expect(isNewer('0.1.0', '0.1.1')).toBe(true)
    expect(isNewer('0.1.0', '0.2.0')).toBe(true)
    expect(isNewer('0.1.0', '1.0.0')).toBe(true)
  })
  it('相同版本 → false', () => {
    expect(isNewer('0.1.0', '0.1.0')).toBe(false)
  })
  it('远端更旧 → false', () => {
    expect(isNewer('0.2.0', '0.1.9')).toBe(false)
  })
  it('逐段数值比较而非字典序', () => {
    expect(isNewer('0.9.0', '0.10.0')).toBe(true)
    expect(isNewer('0.10.0', '0.9.0')).toBe(false)
  })
  it('长度不齐按 0 补齐', () => {
    expect(isNewer('0.1', '0.1.0')).toBe(false)
    expect(isNewer('0.1.0', '0.1')).toBe(false)
    expect(isNewer('0.1.0', '0.2')).toBe(true)
  })
  it('latest 带 v 前缀也能比较', () => {
    expect(isNewer('0.1.0', 'v0.1.1')).toBe(true)
  })
  it('同版本的预发布视为不更新', () => {
    expect(isNewer('0.2.0', '0.2.0-beta')).toBe(false)
  })
  it('跨过预发布号的正式版仍能比较', () => {
    expect(isNewer('0.1.0', '0.2.0-beta')).toBe(true)
  })
})

describe('isCheckDue', () => {
  const INTERVAL = 24 * HOUR
  it('从未检查过（null）→ true', () => {
    expect(isCheckDue(1000, null, INTERVAL)).toBe(true)
  })
  it('间隔内 → false', () => {
    expect(isCheckDue(HOUR, 0, INTERVAL)).toBe(false)
    expect(isCheckDue(INTERVAL - 1, 0, INTERVAL)).toBe(false)
  })
  it('达到间隔（边界相等即到期）→ true', () => {
    expect(isCheckDue(INTERVAL, 0, INTERVAL)).toBe(true)
    expect(isCheckDue(INTERVAL + 1, 0, INTERVAL)).toBe(true)
  })
})

describe('isDismissed', () => {
  it('未忽略过（null）→ false', () => {
    expect(isDismissed('v0.2.0', null)).toBe(false)
  })
  it('忽略的正是该 tag → true', () => {
    expect(isDismissed('v0.2.0', 'v0.2.0')).toBe(true)
  })
  it('忽略的是其他版本 → false', () => {
    expect(isDismissed('v0.3.0', 'v0.2.0')).toBe(false)
  })
})

describe('parseRelease', () => {
  it('提取标准 releases/latest 响应字段', () => {
    const info = parseRelease({
      tag_name: 'v0.2.0',
      html_url: 'https://github.com/o/r/releases/tag/v0.2.0',
      body: '修复若干问题',
      published_at: '2026-08-30T00:00:00Z',
    })
    expect(info).toEqual({
      tagName: 'v0.2.0',
      version: '0.2.0',
      notes: '修复若干问题',
      url: 'https://github.com/o/r/releases/tag/v0.2.0',
    })
  })
  it('body 为 null（GitHub API 无说明时）→ notes 为空串', () => {
    const info = parseRelease({ tag_name: 'v0.1.0', html_url: 'https://x', body: null })
    expect(info?.notes).toBe('')
  })
  it('缺 tag_name / html_url → null', () => {
    expect(parseRelease({ html_url: 'https://x' })).toBeNull()
    expect(parseRelease({ tag_name: 'v0.1.0' })).toBeNull()
  })
  it('非对象输入 → null', () => {
    expect(parseRelease(null)).toBeNull()
    expect(parseRelease('oops')).toBeNull()
  })
})
