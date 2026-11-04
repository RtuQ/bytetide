/** @vitest-environment jsdom */

import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import ConnectionConfigPopover from '../ConnectionConfigPopover.vue'
import type { PortConfig } from '../../types'

const config: PortConfig = {
  name: 'COM7', baudRate: 115200, dataBits: 8, parity: 'none',
  stopBits: '1', flowControl: 'none', transport: 'serial',
}

function rect(left: number, top: number, width: number, height: number): DOMRect {
  return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect
}
function viewport(width: number, height: number) {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: width })
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: height })
}
function panel() {
  return document.body.querySelector<HTMLFormElement>('.connection-config-pop')!
}

describe('ConnectionConfigPopover', () => {
  it('取消不会提交、端口字段只读，关闭后焦点回到触发器', async () => {
    const wrapper = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'old' } })
    const trigger = wrapper.get('button[aria-label="连接参数"]')
    expect(trigger.text()).toBe('连接参数')
    expect(trigger.attributes('title')).toBe('编辑串口参数并重连')
    await trigger.trigger('click')
    const port = document.body.querySelector<HTMLInputElement>('input[readonly]')!
    expect(port.value).toBe('COM7')
    expect(port.getAttribute('aria-readonly')).toBe('true')
    expect(document.activeElement).toBe(document.body.querySelector('input[type="number"]'))
    const cancel = [...document.body.querySelectorAll('button')].find((b) => b.textContent === '取消')!
    cancel.click()
    await wrapper.vm.$nextTick()
    expect(wrapper.emitted('apply')).toBeUndefined()
    expect(panel()).toBeNull()
    expect(document.activeElement).toBe(trigger.element)
    wrapper.unmount()
  })

  it('显示失败信息，并在会话 ID 替换成功后关闭', async () => {
    const wrapper = mount(ConnectionConfigPopover, {
      props: { config, connected: true, sessionId: 'old', error: '端口打开失败' },
    })
    await wrapper.find('button[aria-label="连接参数"]').trigger('click')
    expect(panel().textContent).toContain('端口打开失败')
    await wrapper.setProps({ sessionId: 'new' })
    expect(panel()).toBeNull()
    wrapper.unmount()
  })

  it('提交编辑参数并保留只读端口值', async () => {
    const wrapper = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: false, sessionId: 'old' } })
    await wrapper.find('button[aria-label="连接参数"]').trigger('click')
    const baud = document.body.querySelector<HTMLInputElement>('input[type="number"]')!
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set?.call(baud, '57600')
    baud.dispatchEvent(new Event('input', { bubbles: true }))
    await wrapper.vm.$nextTick()
    panel().dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
    await wrapper.vm.$nextTick()
    expect(wrapper.emitted('apply')?.[0]?.[0]).toMatchObject({ name: 'COM7', baudRate: 57600 })
    wrapper.unmount()
  })

  it('把传送面板钳制在窄小视口内，并限制高度供内容滚动', async () => {
    viewport(300, 220)
    const wrapper = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'old' } })
    const anchor = wrapper.get('.connection-config-anchor').element as HTMLElement
    vi.spyOn(anchor, 'getBoundingClientRect').mockReturnValue(rect(240, 175, 55, 30))
    await wrapper.get('button[aria-label="连接参数"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 0))
    const pop = panel()
    expect(pop.parentElement).toBe(document.body)
    vi.spyOn(pop, 'getBoundingClientRect').mockReturnValue(rect(0, 0, 276, 196))
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()
    const left = Number.parseFloat(pop.style.left)
    const top = Number.parseFloat(pop.style.top)
    expect(left).toBeGreaterThanOrEqual(12)
    expect(left + 276).toBeLessThanOrEqual(288)
    expect(top).toBeGreaterThanOrEqual(12)
    expect(top + 196).toBeLessThanOrEqual(208)
    expect(pop.style.maxHeight).toContain('100vh')
    expect(pop.style.overflowY).toBe('auto')
    wrapper.unmount()
  })

  it('resize 和祖先 scroll 后重新定位，面板内点击不关闭、外部点击关闭', async () => {
    viewport(800, 600)
    const wrapper = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'old' } })
    const anchor = wrapper.get('.connection-config-anchor').element as HTMLElement
    let anchorRect = rect(100, 100, 140, 32)
    vi.spyOn(anchor, 'getBoundingClientRect').mockImplementation(() => anchorRect)
    await wrapper.get('button[aria-label="连接参数"]').trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 0))
    const pop = panel()
    vi.spyOn(pop, 'getBoundingClientRect').mockReturnValue(rect(0, 0, 320, 350))
    await wrapper.vm.$nextTick()
    const firstLeft = pop.style.left
    anchorRect = rect(500, 180, 140, 32)
    window.dispatchEvent(new Event('resize'))
    await wrapper.vm.$nextTick()
    expect(pop.style.left).not.toBe(firstLeft)
    anchorRect = rect(430, 230, 140, 32)
    window.dispatchEvent(new Event('scroll'))
    await wrapper.vm.$nextTick()
    expect(pop.style.left).toBe('250px')

    pop.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    await wrapper.vm.$nextTick()
    expect(panel()).not.toBeNull()
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
    await wrapper.vm.$nextTick()
    expect(panel()).toBeNull()
    wrapper.unmount()
  })

  it('错误文案让面板增高后重新钳制位置，多个实例拥有独立 aria 目标', async () => {
    viewport(800, 600)
    const first = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'first' } })
    const second = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'second' } })
    const firstTrigger = first.get('button[aria-label="连接参数"]')
    const secondTrigger = second.get('button[aria-label="连接参数"]')
    expect(firstTrigger.attributes('aria-controls')).not.toBe(secondTrigger.attributes('aria-controls'))

    const anchor = first.get('.connection-config-anchor').element as HTMLElement
    vi.spyOn(anchor, 'getBoundingClientRect').mockReturnValue(rect(300, 150, 140, 32))
    await firstTrigger.trigger('click')
    await new Promise((resolve) => setTimeout(resolve, 0))
    const pop = document.getElementById(firstTrigger.attributes('aria-controls')!) as HTMLFormElement
    let height = 350
    vi.spyOn(pop, 'getBoundingClientRect').mockImplementation(() => rect(0, 0, 320, height))
    window.dispatchEvent(new Event('resize'))
    await first.vm.$nextTick()
    const before = pop.style.top
    height = 420
    await first.setProps({ error: '连接失败，请检查设备状态。' })
    expect(pop.style.top).not.toBe(before)
    expect(Number.parseFloat(pop.style.top)).toBeGreaterThanOrEqual(12)
    first.unmount()
    second.unmount()
  })

  it('Escape 关闭传送面板并恢复触发器焦点', async () => {
    const wrapper = mount(ConnectionConfigPopover, { attachTo: document.body, props: { config, connected: true, sessionId: 'old' } })
    const trigger = wrapper.get('button[aria-label="连接参数"]')
    await trigger.trigger('click')
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await wrapper.vm.$nextTick()
    expect(panel()).toBeNull()
    expect(document.activeElement).toBe(trigger.element)
    wrapper.unmount()
  })
})
