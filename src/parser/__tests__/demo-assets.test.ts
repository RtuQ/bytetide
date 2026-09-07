/**
 * demo 资产守卫：sample-data/parser-power.js 始终可导入，且帧布局与
 * scripts/demo-feed.mjs（数据源）、数据绘图（累加和校验）三方口径一致。
 * 截图资产被改坏时在此报警，而不是在用户导入/绘图时静默失败。
 */
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { validateDecl } from '../engine'
import { createFramerState, framerFeed } from '../framer'
import { decodeDeclarative } from '../fields'
import { computeChecksum } from '../../composables/usePlotParser'
// @ts-ignore — 演示资产是纯 JS ESM，无类型声明
import demoScript from '../../../sample-data/parser-power.js'

const SRC = readFileSync(new URL('../../../sample-data/parser-power.js', import.meta.url), 'utf8')

/** 导入期校验产物（framing 已规范化为 NormalizedFraming） */
function validated() {
  const r = validateDecl(demoScript, SRC)
  if (!r.ok) throw new Error(`demo 解析脚本 schema 校验失败：${r.error}`)
  return r.script
}

/** 与 scripts/demo-feed.mjs powerFrame() 完全同构的帧构造 */
function buildFrame(temp: number, volt: number, curr: number): Uint8Array {
  const buf = new Uint8Array(9)
  const view = new DataView(buf.buffer)
  buf[0] = 0xaa
  buf[1] = 0x55
  view.setInt16(2, Math.round(temp * 10)) // 大端
  view.setUint16(4, Math.round(volt * 100))
  view.setUint16(6, Math.round(curr * 1000))
  let sum = 0
  for (let i = 2; i < 8; i++) sum = (sum + buf[i]) & 0xff
  buf[8] = sum
  return buf
}

describe('demo 解析脚本资产（sample-data/parser-power.js）', () => {
  it('通过导入期 schema 校验（validateDecl）', () => {
    const script = validated()
    expect(script.framing.length).toEqual({ kind: 'fixed', value: 9 })
    expect(demoScript.meta.name).toBe('电源监控')
  })

  it('帧与 feed 布局一致：sync+定长切帧，连续两帧不粘连', () => {
    const script = validated()
    const state = createFramerState()
    const glued = new Uint8Array(18)
    glued.set(buildFrame(26.4, 3.7, 0.45), 0)
    glued.set(buildFrame(30.1, 3.65, 0.82), 9)
    const { frames } = framerFeed(state, script.framing, glued)
    expect(frames.length).toBe(2)
    for (const f of frames) {
      expect(f.bytes.length).toBe(9)
      expect(f.bytes[0]).toBe(0xaa)
      expect(f.crcOk).toBeNull() // 脚本无 crc 声明
    }
  })

  it('声明式解码：温度/电压/电流 三字段值与文本模板', () => {
    const script = validated()
    const { text, fields } = decodeDeclarative(script, buildFrame(26.4, 3.7, 0.45))
    expect(fields.map((f) => f.value)).toEqual(['26.4', '3.7', '0.45'])
    expect(fields.map((f) => f.unit)).toEqual(['℃', 'V', 'A'])
    expect(text).toBe('温度 26.4℃，电压 3.7V，电流 0.45A')
  })

  it('校验字节与绘图 computeChecksum（数据段累加和）同口径', () => {
    const frame = buildFrame(26.4, 3.7, 0.45)
    const expectSum = computeChecksum(frame.subarray(2, 8), 'sum')
    expect(frame[8]).toBe(expectSum)
  })
})
