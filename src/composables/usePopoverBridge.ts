export const POPOVER_EVENT = 'bytetide:popover'

export type PopoverName = 'new-connection' | 'settings' | 'open-log'

export function requestPopover(name: PopoverName, target?: EventTarget) {
  const eventTarget = target ?? (typeof window !== 'undefined' ? window : undefined)
  eventTarget?.dispatchEvent(new CustomEvent<PopoverName>(POPOVER_EVENT, { detail: name }))
}
