// 命令适配层：前端唯一的命令字符串登记处。命令名与参数键逐一对应
// src-tauri/src/commands/{sessions,bridge,files}.rs 的 #[tauri::command]
// 函数签名（Tauri 2 自动把 snake_case 参数映射为 camelCase 键）。
// 行为一律透传：不重试、不改错、不做默认值——保持与直连 invoke 等价。
import type { IpcClient } from './client'
import { tauriClient } from './client'
import type {
  AiAnnotation,
  BridgeAlert,
  BridgeBookmark,
  BridgeConfigPatch,
  BridgeView,
  CaptureMeta,
  LiveRulesPayload,
  LogConfig,
  PerfDiagnostic,
  PlotConfig,
  PortConfig,
  PortInfo,
  PulledLine,
  RawLogLine,
  RingBounds,
  SendMode,
} from './types'

/** 按业务域命名的命令对象；工厂形式便于测试注入假 IpcClient */
export function createCommands(client: IpcClient) {
  return {
    // ---- 会话连接 / 数据通道（commands/sessions.rs）----
    listPorts(): Promise<PortInfo[]> {
      return client.invoke<PortInfo[]>('list_ports_cmd')
    },
    connect(config: PortConfig, logSettings: LogConfig): Promise<string> {
      return client.invoke<string>('connect_cmd', { config, logSettings })
    },
    disconnect(sessionId: string): Promise<void> {
      return client.invoke<void>('disconnect_cmd', { sessionId })
    },
    send(sessionId: string, mode: SendMode, text: string): Promise<void> {
      return client.invoke<void>('send_cmd', { sessionId, mode, text })
    },
    clearLog(sessionId: string): Promise<void> {
      return client.invoke<void>('clear_log_cmd', { sessionId })
    },
    setSignal(sessionId: string, pin: 'dtr' | 'rts', level: boolean): Promise<void> {
      return client.invoke<void>('set_signal_cmd', { sessionId, pin, level })
    },
    sessionLogPath(sessionId: string): Promise<string> {
      return client.invoke<string>('session_log_path_cmd', { sessionId })
    },
    rotateLog(sessionId: string): Promise<string> {
      return client.invoke<string>('rotate_log_cmd', { sessionId })
    },
    setRecording(sessionId: string, on: boolean): Promise<void> {
      return client.invoke<void>('set_recording_cmd', { sessionId, on })
    },
    /** 拉模型正向拉取（ring_lines_no_cmd）：no > sinceNo 的行，游标语义不重不漏 */
    ringLinesAfter(sessionId: string, sinceNo: number, max: number): Promise<PulledLine[]> {
      return client.invoke<PulledLine[]>('ring_lines_no_cmd', { sessionId, sinceNo, max })
    },
    /** 翻页补旧行（ring_lines_before_cmd）：no < beforeNo 的最新 max 行（升序） */
    ringLinesBefore(sessionId: string, beforeNo: number, max: number): Promise<PulledLine[]> {
      return client.invoke<PulledLine[]>('ring_lines_before_cmd', { sessionId, beforeNo, max })
    },
    ringBounds(sessionId: string): Promise<RingBounds> {
      return client.invoke<RingBounds>('ring_bounds_cmd', { sessionId })
    },
    createOfflineSession(config: PortConfig, path: string, lines: RawLogLine[]): Promise<string> {
      return client.invoke<string>('create_offline_session_cmd', { config, path, lines })
    },
    setLiveRules(sessionId: string, rules: LiveRulesPayload): Promise<void> {
      return client.invoke<void>('set_live_rules_cmd', {
        sessionId,
        autoReply: rules.autoReply,
        alerts: rules.alerts,
        capture: rules.capture,
      })
    },

    // ---- 文件 / 诊断（commands/files.rs）----
    readTextFile(path: string): Promise<string> {
      return client.invoke<string>('read_text_file_cmd', { path })
    },
    exportText(path: string, content: string): Promise<void> {
      return client.invoke<void>('export_text_cmd', { path, content })
    },
    listCaptures(): Promise<CaptureMeta[]> {
      return client.invoke<CaptureMeta[]>('list_captures_cmd')
    },
    deleteCapture(path: string): Promise<void> {
      return client.invoke<void>('delete_capture_cmd', { path })
    },
    capturesDir(): Promise<string> {
      return client.invoke<string>('captures_dir_cmd')
    },
    appendPerfDiagnostic(entry: PerfDiagnostic): Promise<void> {
      return client.invoke<void>('append_perf_diag_cmd', { ...entry })
    },

    // ---- REST 桥配置 / 镜像同步（commands/bridge.rs）----
    getBridgeConfig(): Promise<BridgeView> {
      return client.invoke<BridgeView>('bridge_get_config_cmd')
    },
    setBridgeConfig(patch: BridgeConfigPatch): Promise<BridgeView> {
      return client.invoke<BridgeView>('bridge_set_config_cmd', { patch })
    },
    regenerateBridgeToken(): Promise<BridgeView> {
      return client.invoke<BridgeView>('bridge_regen_token_cmd')
    },
    setPlotConfig(sessionId: string, config: PlotConfig): Promise<void> {
      return client.invoke<void>('set_plot_config_cmd', { sessionId, config })
    },
    syncBookmarks(sessionId: string, values: BridgeBookmark[]): Promise<void> {
      return client.invoke<void>('bridge_sync_bookmarks_cmd', { sessionId, bookmarks: values })
    },
    syncAlerts(sessionId: string, values: BridgeAlert[]): Promise<void> {
      return client.invoke<void>('bridge_sync_alerts_cmd', { sessionId, alerts: values })
    },
    syncAnnotations(sessionId: string, values: AiAnnotation[]): Promise<void> {
      return client.invoke<void>('bridge_sync_annotations_cmd', { sessionId, annotations: values })
    },
  }
}

export type Commands = ReturnType<typeof createCommands>

/** 应用默认命令对象（绑定 Tauri 实现） */
export const commands: Commands = createCommands(tauriClient)
