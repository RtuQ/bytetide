import { describe, expect, it, vi } from 'vitest'
import { POPOVER_EVENT, requestPopover } from '../usePopoverBridge'

describe('usePopoverBridge', () => {
  it('broadcasts which popover should be opened', () => {
    // Node <19 无 CustomEvent 全局（CI 环境）：用可携带 detail 的替身打桩，
    // 浏览器运行时不受影响（实现只在 window 侧使用）
    class StubCustomEvent<T> extends Event {
      detail: T
      constructor(type: string, params?: { detail?: T }) {
        super(type)
        this.detail = params?.detail as T
      }
    }
    vi.stubGlobal('CustomEvent', StubCustomEvent as unknown as typeof CustomEvent)
    try {
      const received: string[] = []
      const target = new EventTarget()
      const onEvent = (event: Event) => {
        received.push((event as CustomEvent<string>).detail)
      }
      target.addEventListener(POPOVER_EVENT, onEvent)

      requestPopover('new-connection', target)

      expect(received).toEqual(['new-connection'])
    } finally {
      vi.unstubAllGlobals()
    }
  })
})
