import { describe, expect, it } from 'vitest'
import { createCommands } from '../commands'
import type { IpcClient } from '../client'
import type { BridgeView, CaptureMeta, PulledLine, RingBounds } from '../types'

/**
 * 录制型假 IpcClient：记录每次 invoke 的 (command, args) 与 listen/emit，
 * 按 FIFO 回放预设响应——断言命令字符串、Rust camelCase 参数键、DTO 透传。
 */
function makeFakeClient(replies: unknown[] = []) {
  const calls: { command: string; args: Record<string, unknown> }[] = []
  const listened: { event: string; handler: (payload: unknown) => void; unlistened: boolean }[] = []
  const emitted: { event: string; payload: unknown }[] = []
  let replyIndex = 0
  const client: IpcClient = {
    async invoke<T>(command: string, args?: Record<string, unknown>) {
      calls.push({ command, args: args ?? {} })
      return replies[replyIndex++] as T
    },
    listen<T>(event: string, handler: (payload: T) => void) {
      const entry = {
        event,
        handler: handler as unknown as (payload: unknown) => void,
        unlistened: false,
      }
      listened.push(entry)
      return Promise.resolve(() => {
        entry.unlistened = true
      })
    },
    async emit(event: string, payload?: unknown) {
      emitted.push({ event, payload })
    },
  }
  return { client, calls, listened, emitted }
}

function lastCall(calls: ReturnType<typeof makeFakeClient>['calls']) {
  const call = calls[calls.length - 1]
  expect(call).toBeDefined()
  return call!
}

function pulledLine(no: number): PulledLine {
  return { no, ts: '12:00:00.000', dir: 'rx', text: `line-${no}`, bytes: null, epochMillis: no }
}

describe('commands 适配层：命令字符串与 camelCase 参数键', () => {
  it('listPorts：无参调用 list_ports_cmd，DTO 原样透传', async () => {
    const ports = [{ name: 'COM3', portType: 'serial' }]
    const { client, calls } = makeFakeClient([ports])
    const out = await createCommands(client).listPorts()
    expect(lastCall(calls)).toEqual({ command: 'list_ports_cmd', args: {} })
    expect(out).toEqual(ports)
  })

  it('connect：config + logSettings（Rust log_settings 的 camelCase）', async () => {
    const { client, calls } = makeFakeClient(['s1'])
    const config = { name: 'x', baudRate: 115200, dataBits: 8, parity: 'none', stopBits: '1', flowControl: 'none' }
    const logSettings = { timestamps: true, viewBufCap: 200000 }
    const id = await createCommands(client).connect(config, logSettings as never)
    expect(lastCall(calls)).toEqual({ command: 'connect_cmd', args: { config, logSettings } })
    expect(id).toBe('s1')
  })

  it('disconnect / clearLog：仅 sessionId', async () => {
    const { client, calls } = makeFakeClient([null, null])
    const c = createCommands(client)
    await c.disconnect('s1')
    expect(lastCall(calls)).toEqual({ command: 'disconnect_cmd', args: { sessionId: 's1' } })
    await c.clearLog('s2')
    expect(lastCall(calls)).toEqual({ command: 'clear_log_cmd', args: { sessionId: 's2' } })
  })

  it('send：sessionId + mode + text（mode 原样透传 ascii/hex）', async () => {
    const { client, calls } = makeFakeClient([null, null])
    const c = createCommands(client)
    await c.send('s1', 'ascii', 'ping')
    expect(lastCall(calls)).toEqual({ command: 'send_cmd', args: { sessionId: 's1', mode: 'ascii', text: 'ping' } })
    await c.send('s1', 'hex', 'AA 55')
    expect(lastCall(calls)).toEqual({ command: 'send_cmd', args: { sessionId: 's1', mode: 'hex', text: 'AA 55' } })
  })

  it('setSignal：sessionId + pin + level', async () => {
    const { client, calls } = makeFakeClient([null, null])
    const c = createCommands(client)
    await c.setSignal('s1', 'dtr', true)
    expect(lastCall(calls)).toEqual({ command: 'set_signal_cmd', args: { sessionId: 's1', pin: 'dtr', level: true } })
    await c.setSignal('s1', 'rts', false)
    expect(lastCall(calls)).toEqual({ command: 'set_signal_cmd', args: { sessionId: 's1', pin: 'rts', level: false } })
  })

  it('sessionLogPath / rotateLog / setRecording：录制控制三命令', async () => {
    const { client, calls } = makeFakeClient(['/logs/a.log', '/logs/a-2.log', null])
    const c = createCommands(client)
    expect(await c.sessionLogPath('s1')).toBe('/logs/a.log')
    expect(lastCall(calls)).toEqual({ command: 'session_log_path_cmd', args: { sessionId: 's1' } })
    expect(await c.rotateLog('s1')).toBe('/logs/a-2.log')
    expect(lastCall(calls)).toEqual({ command: 'rotate_log_cmd', args: { sessionId: 's1' } })
    await c.setRecording('s1', false)
    expect(lastCall(calls)).toEqual({ command: 'set_recording_cmd', args: { sessionId: 's1', on: false } })
  })

  it('ringLinesAfter：映射 ring_lines_no_cmd（sessionId/sinceNo/max），PulledLine[] 透传', async () => {
    const lines = [pulledLine(1), pulledLine(2)]
    const { client, calls } = makeFakeClient([lines])
    const out = await createCommands(client).ringLinesAfter('s1', 7, 5000)
    expect(lastCall(calls)).toEqual({ command: 'ring_lines_no_cmd', args: { sessionId: 's1', sinceNo: 7, max: 5000 } })
    expect(out).toEqual(lines)
    expect(out[0]).toEqual({ no: 1, ts: '12:00:00.000', dir: 'rx', text: 'line-1', bytes: null, epochMillis: 1 })
  })

  it('ringLinesBefore：映射 ring_lines_before_cmd（sessionId/beforeNo/max）', async () => {
    const { client, calls } = makeFakeClient([[pulledLine(1)]])
    await createCommands(client).ringLinesBefore('s1', 100, 2000)
    expect(lastCall(calls)).toEqual({ command: 'ring_lines_before_cmd', args: { sessionId: 's1', beforeNo: 100, max: 2000 } })
  })

  it('ringBounds：RingBounds DTO 透传（firstNo/lastNo/size/ringCap）', async () => {
    const bounds: RingBounds = { firstNo: 10, lastNo: 99, size: 90, ringCap: 100000 }
    const { client, calls } = makeFakeClient([bounds])
    const out = await createCommands(client).ringBounds('s1')
    expect(lastCall(calls)).toEqual({ command: 'ring_bounds_cmd', args: { sessionId: 's1' } })
    expect(out).toEqual(bounds)
  })

  it('readTextFile / createOfflineSession：离线载入两步', async () => {
    const { client, calls } = makeFakeClient(['TSV 内容', 'o1'])
    const c = createCommands(client)
    const config = { name: 'demo', baudRate: 0, dataBits: 8, parity: 'none', stopBits: '1', flowControl: 'none' }
    const lines = [{ ts: 't', dir: 'rx', text: 'x', epochMillis: 1 }]
    expect(await c.readTextFile('/tmp/a.log')).toBe('TSV 内容')
    expect(lastCall(calls)).toEqual({ command: 'read_text_file_cmd', args: { path: '/tmp/a.log' } })
    expect(await c.createOfflineSession(config as never, '/tmp/a.log', lines as never)).toBe('o1')
    expect(lastCall(calls)).toEqual({ command: 'create_offline_session_cmd', args: { config, path: '/tmp/a.log', lines } })
  })

  it('listCaptures / deleteCapture / capturesDir：现场档案三命令', async () => {
    const metas: CaptureMeta[] = [{ fileName: 'x.log', path: '/c/x.log', size: 3, modifiedMs: 5 }]
    const { client, calls } = makeFakeClient([metas, null, '/c'])
    const c = createCommands(client)
    expect(await c.listCaptures()).toEqual(metas)
    expect(lastCall(calls)).toEqual({ command: 'list_captures_cmd', args: {} })
    await c.deleteCapture('/c/x.log')
    expect(lastCall(calls)).toEqual({ command: 'delete_capture_cmd', args: { path: '/c/x.log' } })
    expect(await c.capturesDir()).toBe('/c')
    expect(lastCall(calls)).toEqual({ command: 'captures_dir_cmd', args: {} })
  })

  it('setLiveRules：LiveRulesPayload 拆为 autoReply/alerts/capture 三个顶层参数', async () => {
    const { client, calls } = makeFakeClient([null])
    const rules = {
      autoReply: { enabled: true, rules: [] },
      alerts: { enabled: false, rules: [] },
      capture: { enabled: false, onDisconnect: true, preMs: 1, postMs: 2, rules: [] },
    }
    await createCommands(client).setLiveRules('s1', rules as never)
    expect(lastCall(calls)).toEqual({
      command: 'set_live_rules_cmd',
      args: { sessionId: 's1', autoReply: rules.autoReply, alerts: rules.alerts, capture: rules.capture },
    })
  })

  it('桥配置三命令：get 无参 / set 传 patch / regen 无参，BridgeView 透传', async () => {
    const view: BridgeView = {
      config: { enabled: true, bind: '127.0.0.1', port: 8765, token: 't', allowSend: false },
      runtime: { state: 'running', bound: '127.0.0.1:8765', lastError: null },
    }
    const { client, calls } = makeFakeClient([view, view, view])
    const c = createCommands(client)
    expect(await c.getBridgeConfig()).toEqual(view)
    expect(lastCall(calls)).toEqual({ command: 'bridge_get_config_cmd', args: {} })
    const patch = { enabled: true, confirmRemote: true }
    expect(await c.setBridgeConfig(patch)).toEqual(view)
    expect(lastCall(calls)).toEqual({ command: 'bridge_set_config_cmd', args: { patch } })
    expect(await c.regenerateBridgeToken()).toEqual(view)
    expect(lastCall(calls)).toEqual({ command: 'bridge_regen_token_cmd', args: {} })
  })

  it('setPlotConfig：sessionId + config', async () => {
    const { client, calls } = makeFakeClient([null])
    const config = { enabled: true, source: 'binary' } as never
    await createCommands(client).setPlotConfig('s1', config)
    expect(lastCall(calls)).toEqual({ command: 'set_plot_config_cmd', args: { sessionId: 's1', config } })
  })

  it('镜像同步三命令：键名 bookmarks/alerts/annotations', async () => {
    const { client, calls } = makeFakeClient([null, null, null])
    const c = createCommands(client)
    await c.syncBookmarks('s1', [{ no: 3, ts: 't', text: 'x' }])
    expect(lastCall(calls)).toEqual({ command: 'bridge_sync_bookmarks_cmd', args: { sessionId: 's1', bookmarks: [{ no: 3, ts: 't', text: 'x' }] } })
    await c.syncAlerts('s1', [{ id: 'a', ruleId: 'r', pattern: 'p', level: 'warn', no: 1, ts: 't', text: 'x', at: 2 }])
    expect(lastCall(calls)).toEqual({
      command: 'bridge_sync_alerts_cmd',
      args: { sessionId: 's1', alerts: [{ id: 'a', ruleId: 'r', pattern: 'p', level: 'warn', no: 1, ts: 't', text: 'x', at: 2 }] },
    })
    await c.syncAnnotations('s1', [{ id: 'n', no: 1, ts: 't', text: 'x', note: 'n', at: 2 }])
    expect(lastCall(calls)).toEqual({
      command: 'bridge_sync_annotations_cmd',
      args: { sessionId: 's1', annotations: [{ id: 'n', no: 1, ts: 't', text: 'x', note: 'n', at: 2 }] },
    })
  })

  it('exportText：path + content', async () => {
    const { client, calls } = makeFakeClient([null])
    await createCommands(client).exportText('/out/a.txt', '内容')
    expect(lastCall(calls)).toEqual({ command: 'export_text_cmd', args: { path: '/out/a.txt', content: '内容' } })
  })

  it('appendPerfDiagnostic：六个 camelCase 诊断参数', async () => {
    const { client, calls } = makeFakeClient([null])
    await createCommands(client).appendPerfDiagnostic({
      kind: 'lag',
      sessionId: 's1',
      lagMs: 2500,
      batchMs: 12,
      lines: 500,
      vis: 'visible',
    })
    expect(lastCall(calls)).toEqual({
      command: 'append_perf_diag_cmd',
      args: { kind: 'lag', sessionId: 's1', lagMs: 2500, batchMs: 12, lines: 500, vis: 'visible' },
    })
  })

  it('默认单例 commands 已绑定 Tauri 实现（方法为函数）', async () => {
    const { commands } = await import('../commands')
    expect(typeof commands.listPorts).toBe('function')
    expect(typeof commands.ringLinesAfter).toBe('function')
    expect(typeof commands.appendPerfDiagnostic).toBe('function')
  })
})
