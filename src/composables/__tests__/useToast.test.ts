import { describe, expect, it, vi, afterEach } from 'vitest'
import {
  _resetToasts,
  connectionErrorHint,
  dismissByTag,
  pauseToast,
  resumeToast,
  toast,
  toasts,
} from '../useToast'

describe('useToast', () => {
  it('adds a toast and can dismiss it', () => {
    _resetToasts()
    const id = toast('连接成功', 'success', 0)
    expect(toasts.value).toHaveLength(1)
    expect(toasts.value[0]).toMatchObject({ id, message: '连接成功', kind: 'success' })

    _resetToasts()
    expect(toasts.value).toHaveLength(0)
  })

  it('caps to 5 toasts, dropping oldest', () => {
    _resetToasts()
    const ids: number[] = []
    for (let i = 0; i < 7; i++) ids.push(toast(`m${i}`, 'info', 0))
    expect(toasts.value).toHaveLength(5)
    // 最旧的 m0/m1 被挤掉
    expect(toasts.value[0]!.id).toBe(ids[2])
    expect(toasts.value.map((t) => t.message)).toEqual(['m2', 'm3', 'm4', 'm5', 'm6'])
  })

  it('throttles identical message within dedup window', () => {
    _resetToasts()
    const first = toast('same', 'info', 0)
    const second = toast('same', 'info', 0)
    expect(second).toBe(-1)
    expect(toasts.value).toHaveLength(1)
    expect(toasts.value[0]!.id).toBe(first)
    // 不同文案不受节流影响
    expect(toast('other', 'info', 0)).toBeGreaterThan(0)
    expect(toasts.value).toHaveLength(2)
  })

  it('duration 记入 item；tag 可选', () => {
    _resetToasts()
    toast('带标记', 'warning', 6000, 'COM3', 'disconnect')
    expect(toasts.value[0]).toMatchObject({ duration: 6000, tag: 'disconnect' })
    toast('无时长', 'info', 0)
    expect(toasts.value[1]).toMatchObject({ duration: 0, tag: undefined })
  })

  it('dismissByTag 只收同标记的通知', () => {
    _resetToasts()
    toast('断开A', 'warning', 0, undefined, 'disconnect')
    toast('断开B', 'warning', 0, undefined, 'disconnect')
    toast('别的', 'success', 0)
    dismissByTag('disconnect')
    expect(toasts.value.map((t) => t.message)).toEqual(['别的'])
  })

  it('pause/resume 推迟自动关闭（悬停暂停倒计时）', () => {
    vi.useFakeTimers()
    _resetToasts()
    const id = toast('计时', 'info', 1000)
    pauseToast(id)
    vi.advanceTimersByTime(5000)
    expect(toasts.value).toHaveLength(1)
    resumeToast(id)
    vi.advanceTimersByTime(900)
    expect(toasts.value).toHaveLength(1)
    vi.advanceTimersByTime(200)
    expect(toasts.value).toHaveLength(0)
  })

  it('duration=0 不挂计时器（不自动关闭）', () => {
    vi.useFakeTimers()
    _resetToasts()
    toast('常驻', 'info', 0)
    vi.advanceTimersByTime(60000)
    expect(toasts.value).toHaveLength(1)
  })

  afterEach(() => {
    vi.useRealTimers()
    _resetToasts()
  })
})

describe('connectionErrorHint', () => {
  it('正常断连返回 null（由 disconnected 状态提示负责）', () => {
    expect(connectionErrorHint('串口连接已断开')).toBeNull()
    expect(connectionErrorHint('device disconnected')).toBeNull()
  })

  it('占用类（含中文拒绝访问）→ 端口可能已被占用', () => {
    expect(connectionErrorHint('端口被其他程序占用')!.title).toBe('端口可能已被占用')
    expect(connectionErrorHint('拒绝访问')!.title).toBe('端口可能已被占用')
    expect(connectionErrorHint('Access is denied')!.title).toBe('端口可能已被占用')
  })

  it('超时/未知分类', () => {
    expect(connectionErrorHint('连接超时')!.action).toBe('检查设备、电缆或网络地址后重试')
    expect(connectionErrorHint('某种未知错误')).toEqual({
      title: '连接失败',
      action: '检查参数后重试',
    })
  })
})
