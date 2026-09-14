import { describe, expect, it } from 'vitest'
import type { IpcClient } from '../client'
import { createEmitter, createEventSubscriptions } from '../events'
import type { AiAnnotation, ErrorPayload, PlotConfig, PortInfo, StatusPayload } from '../types'

/** 录制型假 IpcClient：记录订阅的事件名与 handler，退订即停止投递 */
function makeFakeClient() {
  const emitted: { event: string; payload: unknown }[] = []
  const subs: {
    event: string
    handler: (payload: unknown) => void
    deliver: (payload: unknown) => void
    unlistened: boolean
  }[] = []
  const client: IpcClient = {
    async invoke<T>(_command: string, _args?: Record<string, unknown>) {
      return undefined as T
    },
    listen<T>(event: string, handler: (payload: T) => void) {
      const entry = {
        event,
        handler: handler as unknown as (payload: unknown) => void,
        deliver: (payload: unknown) => {
          if (!entry.unlistened) handler(payload as T)
        },
        unlistened: false,
      }
      subs.push(entry)
      return Promise.resolve(() => {
        entry.unlistened = true
      })
    },
    async emit(event: string, payload?: unknown) {
      emitted.push({ event, payload })
    },
  }
  return { client, subs, emitted }
}

/** 各命名订阅对应的后端事件名（gui_sink 转发的既有事件，不可改动） */
const EVENT_NAMES: [keyof ReturnType<typeof createEventSubscriptions>, string][] = [
  ['onSessionStatus', 'session-status'],
  ['onSessionError', 'session-error'],
  ['onPortChanged', 'port-changed'],
  ['onBridgePlotUpdated', 'bridge-plot-updated'],
  ['onBridgeAnnotationsUpdated', 'bridge-annotations-updated'],
  ['onAlertHit', 'alert-hit'],
  ['onCaptureActive', 'capture-active'],
  ['onCaptureSaved', 'capture-saved'],
  ['onReplayState', 'replay-state'],
]

describe('events 适配层：事件名与订阅', () => {
  it('每个命名订阅绑定正确的后端事件字符串', async () => {
    const { client, subs } = makeFakeClient()
    const events = createEventSubscriptions(client)
    for (const [name, expected] of EVENT_NAMES) {
      const subscribe = events[name] as unknown as (h: () => void) => Promise<() => void>
      await subscribe(() => {})
      expect(subs[subs.length - 1]!.event).toBe(expected)
    }
    expect(subs).toHaveLength(EVENT_NAMES.length)
  })

  it('payload 原样透传给订阅者（不包 Event 壳、不改引用）', async () => {
    const { client, subs } = makeFakeClient()
    const events = createEventSubscriptions(client)

    const status: StatusPayload = { sessionId: 's1', status: 'connected' }
    let gotStatus: StatusPayload | null = null
    await events.onSessionStatus((p) => {
      gotStatus = p
    })
    subs[0]!.deliver(status)
    expect(gotStatus).toBe(status)

    const err: ErrorPayload = { sessionId: 's1', error: 'boom' }
    let gotError: ErrorPayload | null = null
    await events.onSessionError((p) => {
      gotError = p
    })
    subs[1]!.deliver(err)
    expect(gotError).toBe(err)

    const ports: PortInfo[] = [{ name: 'COM3', portType: 'serial' }]
    let gotPorts: PortInfo[] | null = null
    await events.onPortChanged((p) => {
      gotPorts = p
    })
    subs[2]!.deliver(ports)
    expect(gotPorts).toBe(ports)

    const plot: { sessionId: string; config: PlotConfig } = { sessionId: 's1', config: { enabled: true } as PlotConfig }
    let gotPlot: { sessionId: string; config: PlotConfig } | null = null
    await events.onBridgePlotUpdated((p) => {
      gotPlot = p
    })
    subs[3]!.deliver(plot)
    expect(gotPlot).toBe(plot)

    const notes: { sessionId: string; annotations: AiAnnotation[] } = { sessionId: 's1', annotations: [] }
    let gotNotes: { sessionId: string; annotations: AiAnnotation[] } | null = null
    await events.onBridgeAnnotationsUpdated((p) => {
      gotNotes = p
    })
    subs[4]!.deliver(notes)
    expect(gotNotes).toBe(notes)

    const alertHit = { sessionId: 's1', hits: [{ ruleId: 'r', pattern: 'p', level: 'warn', no: 1, ts: 't', text: 'x', at: 2 }] }
    let gotHit: typeof alertHit | null = null
    await events.onAlertHit((p) => {
      gotHit = p
    })
    subs[5]!.deliver(alertHit)
    expect(gotHit).toBe(alertHit)

    let gotActive: { sessionId: string; rule: string } | null = null
    await events.onCaptureActive((p) => {
      gotActive = p
    })
    subs[6]!.deliver({ sessionId: 's1', rule: 'panic' })
    expect(gotActive).toEqual({ sessionId: 's1', rule: 'panic' })

    let gotSaved: { sessionId: string } | null = null
    await events.onCaptureSaved((p) => {
      gotSaved = p
    })
    subs[7]!.deliver({ sessionId: 's1' })
    expect(gotSaved).toEqual({ sessionId: 's1' })
  })

  it('退订函数是同步函数：调用后事件不再到达订阅者', async () => {
    const { client, subs } = makeFakeClient()
    const events = createEventSubscriptions(client)
    let count = 0
    const unlisten = await events.onCaptureSaved(() => {
      count += 1
    })
    subs[0]!.deliver({ sessionId: 's1' })
    expect(count).toBe(1)
    expect(typeof unlisten).toBe('function')
    unlisten()
    subs[0]!.deliver({ sessionId: 's1' })
    expect(count).toBe(1) // 退订后不再收到
  })

  it('emitAppReady 走 client.emit 且事件名为 app-ready', async () => {
    const { client, emitted } = makeFakeClient()
    await createEmitter(client).emitAppReady()
    expect(emitted).toEqual([{ event: 'app-ready', payload: undefined }])
  })
})
