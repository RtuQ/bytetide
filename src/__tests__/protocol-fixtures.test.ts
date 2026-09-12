import { describe, expect, it } from 'vitest'
import matcherFixture from '../../testdata/protocol/matcher-v1.json'
import { lineBytes } from '../parser/lineBytes'
import type { Dir } from '../types'

/**
 * 桥交换匹配器黄金样本契约（testdata/protocol/matcher-v1.json）。
 * Rust 侧 compile_exchange_match / parse_hex_strict / parse_mask_strict /
 * find_exchange_response（src-tauri/src/bridge/routes/exchange.rs）与这些向量同源：
 * 错误码是 Stage 1 冻结的跨语言契约（400 ApiError {error:{code,message}}），
 * 本测试冻结 TS 消费侧认得的向量集合与解码结果——两侧任一漂移即红。
 */

interface MatcherFixture {
  version: number
  validMatchers: {
    cases: {
      name: string
      match: Record<string, string> | null
      compiled: { dir: string; reSource: string | null; hexBytes: number[]; mask: (number | null)[] }
    }[]
  }
  invalidMatchers: {
    cases: { name: string; match: Record<string, string>; code: string }[]
    stableCodes: string[]
  }
  hexStrict: { cases: { input: string; ok: boolean; bytes?: number[]; code?: string }[] }
  maskStrict: { cases: { input: string; ok: boolean; mask?: (number | null)[]; code?: string }[] }
  findCases: {
    cases: {
      name: string
      match: Record<string, string>
      lines: { no: number; dir: string; text: string; bytes: number[] | null; epochMillis: number }[]
      expectedNo: number | null
    }[]
  }
}

const fx = matcherFixture as unknown as MatcherFixture

const ERROR = 'invalid input'

// ---- 与 Rust parse_hex_strict 同语义的最小 TS 镜像（仅测试内使用） ----
function parseHexStrict(s: string): { ok: true; bytes: number[] } | { ok: false; code: string } {
  const cleaned = s.replace(/\s/g, '')
  if (cleaned === '') return { ok: false, code: ERROR }
  if (!/^[0-9a-fA-F]*$/.test(cleaned)) return { ok: false, code: ERROR }
  if (cleaned.length % 2 !== 0) return { ok: false, code: ERROR }
  const bytes: number[] = []
  for (let i = 0; i < cleaned.length; i += 2) bytes.push(parseInt(cleaned.slice(i, i + 2), 16))
  return { ok: true, bytes }
}

// ---- 与 Rust parse_mask_strict 同语义的最小 TS 镜像（仅测试内使用） ----
function parseMaskStrict(s: string): { ok: true; mask: (number | null)[] } | { ok: false; code: string } {
  const cleaned = s.replace(/\s/g, '')
  if (cleaned === '' || cleaned.length % 2 !== 0) return { ok: false, code: ERROR }
  const mask: (number | null)[] = []
  for (let i = 0; i < cleaned.length; i += 2) {
    const pair = cleaned.slice(i, i + 2)
    if (pair === '??') mask.push(null)
    else if (/^[0-9a-fA-F]{2}$/.test(pair)) mask.push(parseInt(pair, 16))
    else return { ok: false, code: ERROR }
  }
  return { ok: true, mask }
}

// ---- 与 Rust find_exchange_response 同语义的最小 TS 镜像（仅测试内使用） ----
function bytesFind(hay: Uint8Array, needle: number[]): boolean {
  outer: for (let i = 0; i + needle.length <= hay.length; i++) {
    for (let k = 0; k < needle.length; k++) if (hay[i + k] !== needle[k]) continue outer
    return true
  }
  return false
}
function maskFind(hay: Uint8Array, mask: (number | null)[]): boolean {
  outer: for (let i = 0; i + mask.length <= hay.length; i++) {
    for (let k = 0; k < mask.length; k++) {
      const m = mask[k]
      if (m !== null && hay[i + k] !== m) continue outer
    }
    return true
  }
  return false
}

describe('matcher-v1 契约：版本与稳定错误码集合', () => {
  it('fixture 版本为 1', () => {
    expect(fx.version).toBe(1)
  })
  it('stableCodes 是 Stage 1 冻结的五元集，且所有 invalid 用例的 code 都在集合内', () => {
    expect(fx.invalidMatchers.stableCodes).toEqual([
      'invalid_regex',
      'invalid_hex',
      'invalid_mask',
      'invalid_direction',
      'conflicting_matchers',
    ])
    for (const c of fx.invalidMatchers.cases) {
      expect(fx.invalidMatchers.stableCodes, c.name).toContain(c.code)
    }
  })
})

describe('matcher-v1 契约：非法向量按规则归类（regex 用 TS RegExp 判定，hex/mask 用镜像函数判定）', () => {
  it('invalid_regex 向量确实无法编译、合法 re 向量确实可编译', () => {
    for (const c of fx.invalidMatchers.cases) {
      if (c.code !== 'invalid_regex') continue
      expect(() => new RegExp(c.match.re!), c.name).toThrow()
    }
    for (const c of fx.validMatchers.cases) {
      const m = c.match
      if (!m || typeof m.re !== 'string') continue
      expect(() => new RegExp(m.re), c.name).not.toThrow()
    }
  })
  it('invalid_hex / invalid_mask 向量经同语义镜像函数判定为非法', () => {
    for (const c of fx.invalidMatchers.cases) {
      if (c.code === 'invalid_hex') expect(parseHexStrict(c.match.hex!), c.name).toMatchObject({ ok: false })
      if (c.code === 'invalid_mask') expect(parseMaskStrict(c.match.mask!), c.name).toMatchObject({ ok: false })
    }
  })
  it('冲突用例携带多于一个 matcher 字段', () => {
    for (const c of fx.invalidMatchers.cases) {
      if (c.code !== 'conflicting_matchers') continue
      const n = ['re', 'hex', 'mask'].filter((k) => typeof c.match[k] === 'string').length
      expect(n, c.name).toBeGreaterThan(1)
    }
  })
})

describe('matcher-v1 契约：hexStrict / maskStrict 函数级向量', () => {
  it('hex 向量解码一致', () => {
    for (const c of fx.hexStrict.cases) {
      const r = parseHexStrict(c.input)
      if (c.ok) {
        expect(r, c.input).toMatchObject({ ok: true, bytes: c.bytes })
      } else {
        expect(r.ok, c.input).toBe(false)
      }
    }
  })
  it('mask 向量解码一致（?? 通配 → null）', () => {
    for (const c of fx.maskStrict.cases) {
      const r = parseMaskStrict(c.input)
      if (c.ok) {
        expect(r, c.input).toMatchObject({ ok: true, mask: c.mask })
      } else {
        expect(r.ok, c.input).toBe(false)
      }
    }
  })
})

describe('matcher-v1 契约：validMatchers 编译产物', () => {
  it('dir 缺省回落 rx；空字段视为未携带；hex/mask 解码与 compiled 向量一致', () => {
    for (const c of fx.validMatchers.cases) {
      expect(c.compiled.dir === 'rx' || c.compiled.dir === 'tx', c.name).toBe(true)
      if (c.compiled.hexBytes.length > 0) {
        const src = (c.match as Record<string, string>).hex
        expect(parseHexStrict(src!), c.name).toMatchObject({ ok: true, bytes: c.compiled.hexBytes })
      }
      if (c.compiled.mask.length > 0) {
        const src = (c.match as Record<string, string>).mask
        expect(parseMaskStrict(src!), c.name).toMatchObject({ ok: true, mask: c.compiled.mask })
      }
    }
  })
})

describe('matcher-v1 契约：findCases 行匹配（bytes 优先于 lossy text）', () => {
  for (const c of fx.findCases.cases) {
    it(c.name, () => {
      const hex = typeof c.match.hex === 'string' ? parseHexStrict(c.match.hex) : null
      const mask = typeof c.match.mask === 'string' ? parseMaskStrict(c.match.mask) : null
      const re = typeof c.match.re === 'string' ? new RegExp(c.match.re) : null
      const dir = c.match.dir ?? 'rx'
      const hit = c.lines.find((l) => {
        if ((l.dir as Dir) !== dir) return false
        if (re && !re.test(l.text)) return false
        const b = lineBytes(
          { no: l.no, ts: '', dir: l.dir as Dir, text: l.text, bytes: l.bytes, epochMillis: l.epochMillis },
          'binary',
        )
        if (hex?.ok && !bytesFind(b, hex.bytes)) return false
        if (mask?.ok && !maskFind(b, mask.mask)) return false
        return true
      })
      expect(hit?.no ?? null).toBe(c.expectedNo)
    })
  }
})
