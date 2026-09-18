/**
 * 中文词典 · errors 域：后端错误码文案（`errors.<code>`，code 全集见
 * src/ipc/errorCodes.ts，由 Rust 错误码机制生成）。展示统一走
 * src/ipc/errors.ts 的 errorMessage（词条 + 技术细节括注）。
 * detail（OS/IO 文本、端口名等）不翻译，括注展示。
 */
export const errors = {
  // 兜底：无合法 code|detail 前缀的旧格式 / 异常对象
  'errors.generic': '操作失败',

  // ---- core 事件面（EventSink::error → toast/横幅）----
  'errors.open_port_failed': '打开串口失败',
  'errors.open_link_failed': '建立网络链路失败',
  'errors.port_disconnected': '串口连接已断开',
  'errors.net_disconnected': '网络连接已断开',
  'errors.port_write_failed': '写入串口失败',
  'errors.net_write_failed': '网络写入失败',
  'errors.read_failed': '读取数据失败',
  'errors.log_path_unwritable': '日志路径无效或不可写',
  'errors.log_rotate_failed': '另起新日志失败',
  'errors.midnight_rotate_failed': '午夜另起新日志失败',
  'errors.set_signal_failed': '设置信号线失败',
  'errors.capture_create_failed': '现场档案创建失败',

  // ---- core 回放 ----
  'errors.replay_read_failed': '回放读取失败',
  'errors.replay_seek_failed': '回放定位失败',
  'errors.replay_config_invalid': '回放配置非法',

  // ---- core 会话查询面 ----
  'errors.session_not_found': '会话不存在',
  'errors.not_replay_session': '该会话不是回放会话',
  'errors.replay_channel_unavailable': '回放控制通道不可用',
  'errors.replay_channel_closed': '回放控制通道已关闭',
  'errors.offline_no_send': '离线会话不可发送',
  'errors.replay_no_send': '回放会话不支持发送',
  'errors.send_channel_closed': '发送通道已关闭',
  'errors.offline_no_signal': '离线会话不可控制信号线',
  'errors.replay_no_signal': '回放会话不支持信号线',
  'errors.signal_channel_closed': '信号线通道已关闭',
  'errors.clear_channel_closed': '清屏通道已关闭',
  'errors.channel_closed': '通道已关闭',
  'errors.offline_no_recording': '离线会话不写日志文件',
  'errors.replay_no_recording': '回放会话不支持落盘',
  'errors.recording_disabled': '该会话未启用日志落盘',
  'errors.thread_spawn_failed': '读线程启动失败',
  'errors.replay_thread_spawn_failed': '回放线程启动失败',

  // ---- src-tauri 命令层 ----
  'errors.unknown_signal': '未知信号线',
  'errors.path_outside_captures': '路径不在现场档案目录内',
  'errors.replay_session_not_found': '会话不存在或不是回放会话',
  'errors.replay_seek_invalid': '回放定位行号非法',
  'errors.replay_speed_invalid': '回放速度非法',
  'errors.replay_loop_invalid': '回放循环开关非法',
  'errors.unknown_replay_action': '未知回放控制命令',
  'errors.offline_no_scenario': '离线会话不支持场景',
  'errors.replay_no_send_step': '回放会话不支持发送步骤',
  'errors.scenario_invalid': '场景校验失败',
  'errors.scenario_already_running': '同一会话同时只能运行一个场景',
  'errors.scenario_thread_spawn_failed': '场景运行线程启动失败',
  'errors.scenario_run_not_found': '场景运行不存在',
  'errors.unknown_report_format': '未知报告格式',
  'errors.report_not_ready': '场景尚未完成，报告未生成',
} as const
