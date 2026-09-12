// 事件适配层：前端唯一的事件字符串登记处。事件名逐一对应
// src-tauri/src/gui_sink.rs 转发的既有事件（改事件名必须同步 useTauriEvents，
// 现在是：改后端事件名必须同步这里）。handler 直接收 payload，
// 不接触 Tauri 的 Event 壳；退订函数为同步调用。
import type { IpcClient, Unlisten } from './client'
import { tauriClient } from './client'
import type { AiAnnotation, ErrorPayload, PlotConfig, PortInfo, StatusPayload } from './types'

/** alert-hit 载荷：后端读线程评估命中，稀疏事件（行号是 ring no，回查 UI 行号用） */
export interface AlertHitPayload {
  sessionId: string
  hits: {
    ruleId: string
    pattern: string
    level: string
    no: number
    ts: string
    text: string
    at: number
  }[]
}

/** bridge-plot-updated 载荷：REST 桥写回绘图文法（POST /plot-config） */
export interface BridgePlotUpdatedPayload {
  sessionId: string
  config: PlotConfig
}

/** bridge-annotations-updated 载荷：AI 批注写入/删除的整包同步 */
export interface BridgeAnnotationsUpdatedPayload {
  sessionId: string
  annotations: AiAnnotation[]
}

/** capture-active 载荷：现场捕获 armed 置位（rule=命中的触发规则） */
export interface CaptureActivePayload {
  sessionId: string
  rule: string
}

/** capture-saved 载荷：现场捕获档案落成 */
export interface CaptureSavedPayload {
  sessionId: string
}

/** 按业务域命名的事件订阅；工厂形式便于测试注入假 IpcClient */
export function createEventSubscriptions(client: IpcClient) {
  return {
    onSessionStatus(handler: (payload: StatusPayload) => void): Promise<Unlisten> {
      return client.listen<StatusPayload>('session-status', handler)
    },
    onSessionError(handler: (payload: ErrorPayload) => void): Promise<Unlisten> {
      return client.listen<ErrorPayload>('session-error', handler)
    },
    onPortChanged(handler: (payload: PortInfo[]) => void): Promise<Unlisten> {
      return client.listen<PortInfo[]>('port-changed', handler)
    },
    onBridgePlotUpdated(handler: (payload: BridgePlotUpdatedPayload) => void): Promise<Unlisten> {
      return client.listen<BridgePlotUpdatedPayload>('bridge-plot-updated', handler)
    },
    onBridgeAnnotationsUpdated(
      handler: (payload: BridgeAnnotationsUpdatedPayload) => void,
    ): Promise<Unlisten> {
      return client.listen<BridgeAnnotationsUpdatedPayload>('bridge-annotations-updated', handler)
    },
    onAlertHit(handler: (payload: AlertHitPayload) => void): Promise<Unlisten> {
      return client.listen<AlertHitPayload>('alert-hit', handler)
    },
    onCaptureActive(handler: (payload: CaptureActivePayload) => void): Promise<Unlisten> {
      return client.listen<CaptureActivePayload>('capture-active', handler)
    },
    onCaptureSaved(handler: (payload: CaptureSavedPayload) => void): Promise<Unlisten> {
      return client.listen<CaptureSavedPayload>('capture-saved', handler)
    },
  }
}

export type EventSubscriptions = ReturnType<typeof createEventSubscriptions>

/** 应用默认订阅对象（绑定 Tauri 实现），逐个具名导出 */
const tauriEvents: EventSubscriptions = createEventSubscriptions(tauriClient)
export const onSessionStatus = tauriEvents.onSessionStatus
export const onSessionError = tauriEvents.onSessionError
export const onPortChanged = tauriEvents.onPortChanged
export const onBridgePlotUpdated = tauriEvents.onBridgePlotUpdated
export const onBridgeAnnotationsUpdated = tauriEvents.onBridgeAnnotationsUpdated
export const onAlertHit = tauriEvents.onAlertHit
export const onCaptureActive = tauriEvents.onCaptureActive
export const onCaptureSaved = tauriEvents.onCaptureSaved

/** 前端 → 后端单发事件；工厂形式便于测试注入 */
export function createEmitter(client: IpcClient) {
  return {
    /** 首帧就绪：通知 Rust 显示窗口（防白屏），失败静默 */
    emitAppReady(): Promise<void> {
      return client.emit('app-ready')
    },
  }
}

export const emitAppReady = createEmitter(tauriClient).emitAppReady
