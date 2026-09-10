import { describe, expect, it } from 'vitest'
import { POPOVER_EVENT, requestPopover } from '../usePopoverBridge'

describe('usePopoverBridge', () => {
  it('broadcasts which popover should be opened', () => {
    const received: string[] = []
    const target = new EventTarget()
    const onEvent = (event: Event) => {
      received.push((event as CustomEvent<string>).detail)
    }
    target.addEventListener(POPOVER_EVENT, onEvent)

    requestPopover('new-connection', target)

    target.removeEventListener(POPOVER_EVENT, onEvent)
    expect(received).toEqual(['new-connection'])
  })
})
