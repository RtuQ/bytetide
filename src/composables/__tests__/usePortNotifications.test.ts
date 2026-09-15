import { describe, expect, it, beforeEach } from 'vitest'
import { consumePortDiff, describePort, diffPorts, _resetPortWatchForTest } from '../usePortNotifications'
import type { PortInfo } from '../../types'

function port(name: string, extra: Partial<PortInfo> = {}): PortInfo {
  return { name, portType: 'serial', ...extra }
}

describe('diffPorts', () => {
  it('新插入的端口进 arrived', () => {
    const d = diffPorts([port('COM3')], [port('COM3'), port('COM7')])
    expect(d.arrived.map((p) => p.name)).toEqual(['COM7'])
    expect(d.removed).toEqual([])
  })

  it('被拔出的端口进 removed', () => {
    const d = diffPorts([port('COM3'), port('COM7')], [port('COM3')])
    expect(d.arrived).toEqual([])
    expect(d.removed.map((p) => p.name)).toEqual(['COM7'])
  })

  it('同时插拔双向返回', () => {
    const d = diffPorts([port('COM3'), port('COM7')], [port('COM7'), port('COM11')])
    expect(d.arrived.map((p) => p.name)).toEqual(['COM11'])
    expect(d.removed.map((p) => p.name)).toEqual(['COM3'])
  })

  it('同名端口元数据变化不算插拔；顺序无关', () => {
    const d = diffPorts([port('COM3', { product: 'A' })], [port('COM3', { product: 'B' })])
    expect(d.arrived).toEqual([])
    expect(d.removed).toEqual([])

    const reordered = diffPorts([port('COM3'), port('COM7')], [port('COM7'), port('COM3')])
    expect(reordered.arrived).toEqual([])
    expect(reordered.removed).toEqual([])
  })

  it('空列表到空列表为空 diff', () => {
    expect(diffPorts([], [])).toEqual({ arrived: [], removed: [] })
  })
})

describe('describePort', () => {
  it('产品名优先，厂商拼接其后', () => {
    expect(describePort(port('COM7', { product: 'CP2102N', vendor: 'Silicon Labs' }))).toBe('CP2102N · Silicon Labs')
    expect(describePort(port('COM7', { vendor: 'FTDI' }))).toBe('FTDI')
  })

  it('无产品/厂商时回退序列号，再回退传输类型', () => {
    expect(describePort(port('COM7', { serial: 'FF95' }))).toBe('SN FF95')
    expect(describePort({ name: 'COM7', portType: 'usb' })).toBe('usb')
  })

  it('空白字符串视同缺失', () => {
    expect(describePort(port('COM7', { product: '  ', vendor: '', serial: '  ' }))).toBe('serial')
  })
})

describe('consumePortDiff（启动基线）', () => {
  beforeEach(() => _resetPortWatchForTest())

  it('首帧只建基线返回 null，之后返回与上次的 diff', () => {
    expect(consumePortDiff([port('COM3')])).toBeNull()
    const d = consumePortDiff([port('COM3'), port('COM7')])
    expect(d!.arrived.map((p) => p.name)).toEqual(['COM7'])
    expect(d!.removed).toEqual([])
  })

  it('重置后重新建基线', () => {
    consumePortDiff([port('COM3')])
    _resetPortWatchForTest()
    expect(consumePortDiff([port('COM9')])).toBeNull()
  })
})
