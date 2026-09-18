// 后端错误码全集（Rust 侧 cmd_err/format!("{}|{}") 生成的 code 契约）。
// Rust 与此清单必须一一对应；generic 是前端对无前缀旧格式的兜底码。
// 解析逻辑见 ./errors.ts（parseCmdError/errorMessage），展示词条 errors.<code>。

export const CMD_ERROR_CODES = [
  // 兜底：无合法 code|detail 前缀的旧格式 / 异常对象
  'generic',

  // ---- core 事件面（EventSink::error → toast/横幅：serial/runtime.rs）----
  'open_port_failed', // 打开串口失败          detail: {端口名}: {io 文本}
  'open_link_failed', // 建立网络链路失败      detail: {tcp-client|tcp-server|udp host:port}: {io 文本}
  'port_disconnected', // 串口连接已断开（EOF）
  'net_disconnected', // 网络连接已断开（EOF）
  'port_write_failed', // 写入串口失败
  'net_write_failed', // 网络写入失败
  'read_failed', // 读取错误              detail: {io 文本}
  'log_path_unwritable', // 日志路径无效/不可写   detail: {路径}: {io 文本}
  'log_rotate_failed', // 另起新日志失败        detail: {路径}: {io 文本}
  'midnight_rotate_failed', // 午夜另起新日志失败    detail: {路径}: {io 文本}
  'set_signal_failed', // 设置信号线失败        detail: {io 文本}
  'capture_create_failed', // 现场档案创建失败      detail: {路径}: {io 文本}

  // ---- core 回放（replay/runner.rs + replay/model.rs 经 manager 包装）----
  'replay_read_failed', // 回放读取失败          detail: {io 文本}
  'replay_seek_failed', // 回放定位失败          detail: {io 文本}
  'replay_config_invalid', // 回放配置非法          detail: speed must be …（core 校验英文串）

  // ---- core 会话查询面（serial/manager/mod.rs，anyhow → 命令 Err）----
  'session_not_found', // 会话不存在
  'not_replay_session', // 非回放会话
  'replay_channel_unavailable', // 回放控制通道不可用
  'replay_channel_closed', // 回放控制通道已关闭
  'offline_no_send', // 离线会话不可发送
  'replay_no_send', // 回放会话不支持发送
  'send_channel_closed', // 发送通道已关闭
  'offline_no_signal', // 离线会话不可控制信号线
  'replay_no_signal', // 回放会话不支持信号线
  'signal_channel_closed', // 通道已关闭（信号线）
  'clear_channel_closed', // 通道已关闭（清屏）
  'channel_closed', // 通道已关闭（会话未连接；录制/分段）
  'offline_no_recording', // 离线会话不落盘
  'replay_no_recording', // 回放会话不支持落盘
  'recording_disabled', // 该会话未启用日志落盘
  'thread_spawn_failed', // 读线程启动失败        detail: {os 文本}
  'replay_thread_spawn_failed', // 回放线程启动失败      detail: {os 文本}

  // ---- src-tauri 命令层（commands/*.rs 自产）----
  'unknown_signal', // 未知信号线            detail: {pin 原值}
  'path_outside_captures', // 路径不在现场档案目录内
  'replay_session_not_found', // 会话不存在或非回放会话（回放状态/控制面）
  'replay_seek_invalid', // 回放 seek 行号非法    detail: {原值} | missing value
  'replay_speed_invalid', // 回放速度非法          detail: {原值} | missing value
  'replay_loop_invalid', // 回放循环开关非法      detail: {原值} | missing value
  'unknown_replay_action', // 未知回放控制          detail: {action 原值}
  'offline_no_scenario', // 离线会话不支持场景
  'replay_no_send_step', // 回放会话不支持发送步骤
  'scenario_invalid', // 场景校验失败          detail: {core 稳定英文码串 invalid_version 等}
  'scenario_already_running', // 同一会话同时只允许运行一个场景 detail: {sessionId}
  'scenario_thread_spawn_failed', // 场景运行线程启动失败  detail: {os 文本}
  'scenario_run_not_found', // 场景运行不存在        detail: {runId}
  'unknown_report_format', // 未知报告格式          detail: {format 原值}
  'report_not_ready', // 场景尚未完成，报告未生成 detail: {runId}
] as const

export type CmdErrorCode = (typeof CMD_ERROR_CODES)[number]
