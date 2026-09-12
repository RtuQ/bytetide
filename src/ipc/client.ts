// 类型化 IPC 边界的传输端点：全仓库唯一 import @tauri-apps/api/core 与
// @tauri-apps/api/event 的文件（架构红线，scripts/check-architecture.mjs 强制）。
// 命令/事件语义见 ./commands.ts 与 ./events.ts；上层永远注入 IpcClient 接口，
// 不直接触碰 Tauri 的 Event 壳（{event,id,payload}）——适配层在这里解包 payload。
import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { emit as tauriEmit, listen as tauriListen } from '@tauri-apps/api/event'

/** 退订函数：同步调用即注销（listen 返回的 Promise 解析出的就是它本身） */
export type Unlisten = () => void

/**
 * 前端与 Tauri 后端之间唯一的传输抽象。
 * - invoke：命令调用（命令字符串只允许出现在 commands.ts）
 * - listen：事件订阅（事件字符串只允许出现在 events.ts），handler 直接收 payload
 * - emit：前端 → 后端的单发事件（App.vue 的 app-ready 防白屏握手）
 */
export interface IpcClient {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten>
  emit(event: string, payload?: unknown): Promise<void>
}

/** 默认 Tauri 2 实现；测试注入假 IpcClient，无需触碰此对象 */
export const tauriClient: IpcClient = {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    // 无参命令保持单参调用形态（不传显式 undefined），与迁移前 invoke(cmd) 逐字等价
    return args === undefined ? tauriInvoke<T>(command) : tauriInvoke<T>(command, args)
  },
  listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
    // unlisten Promise 在适配器内部消化：订阅者拿到的一律是同步退订函数
    return tauriListen<T>(event, (e) => handler(e.payload))
  },
  emit(event: string, payload?: unknown): Promise<void> {
    return tauriEmit(event, payload)
  },
}
