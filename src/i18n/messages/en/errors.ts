import type { errors as errorsZh } from '../zh-CN/errors'

/**
 * English · errors 域（后端错误码文案，code 全集见 src/ipc/errorCodes.ts）。
 * detail（OS/IO text, port names）is shown verbatim in parentheses, never translated.
 */
export const errors: Record<keyof typeof errorsZh, string> = {
  // Fallback: legacy format without a valid code|detail prefix / thrown objects
  'errors.generic': 'Operation failed',

  // ---- core event surface (EventSink::error → toast/banner) ----
  'errors.open_port_failed': 'Failed to open serial port',
  'errors.open_link_failed': 'Failed to establish network link',
  'errors.port_disconnected': 'Serial connection lost',
  'errors.net_disconnected': 'Network connection lost',
  'errors.port_write_failed': 'Failed to write to serial port',
  'errors.net_write_failed': 'Failed to write to the network',
  'errors.read_failed': 'Failed to read data',
  'errors.log_path_unwritable': 'Log path is invalid or not writable',
  'errors.log_rotate_failed': 'Failed to start a new log segment',
  'errors.midnight_rotate_failed': 'Failed to start the midnight log segment',
  'errors.set_signal_failed': 'Failed to set the signal line',
  'errors.capture_create_failed': 'Failed to create the capture archive',

  // ---- core replay ----
  'errors.replay_read_failed': 'Failed to read the replay source',
  'errors.replay_seek_failed': 'Failed to seek in the replay source',
  'errors.replay_config_invalid': 'Invalid replay configuration',

  // ---- core session query surface ----
  'errors.session_not_found': 'Session not found',
  'errors.not_replay_session': 'This is not a replay session',
  'errors.replay_channel_unavailable': 'Replay control channel is unavailable',
  'errors.replay_channel_closed': 'Replay control channel is closed',
  'errors.offline_no_send': 'Cannot send in an offline session',
  'errors.replay_no_send': 'Sending is not supported in replay sessions',
  'errors.send_channel_closed': 'Send channel is closed',
  'errors.offline_no_signal': 'Signal lines are not available in offline sessions',
  'errors.replay_no_signal': 'Signal lines are not supported in replay sessions',
  'errors.signal_channel_closed': 'Signal line channel is closed',
  'errors.clear_channel_closed': 'Clear channel is closed',
  'errors.channel_closed': 'Channel is closed',
  'errors.offline_no_recording': 'Offline sessions do not write log files',
  'errors.replay_no_recording': 'Log writing is not supported in replay sessions',
  'errors.recording_disabled': 'Log recording is disabled for this session',
  'errors.thread_spawn_failed': 'Failed to start the read thread',
  'errors.replay_thread_spawn_failed': 'Failed to start the replay thread',

  // ---- src-tauri command layer ----
  'errors.unknown_signal': 'Unknown signal line',
  'errors.path_outside_captures': 'Path is outside the captures directory',
  'errors.replay_session_not_found': 'Session not found or not a replay session',
  'errors.replay_seek_invalid': 'Invalid replay seek position',
  'errors.replay_speed_invalid': 'Invalid replay speed',
  'errors.replay_loop_invalid': 'Invalid replay loop setting',
  'errors.unknown_replay_action': 'Unknown replay control action',
  'errors.offline_no_scenario': 'Scenarios are not supported in offline sessions',
  'errors.replay_no_send_step': 'Send steps are not supported in replay sessions',
  'errors.scenario_invalid': 'Scenario validation failed',
  'errors.scenario_already_running': 'Only one scenario can run at a time per session',
  'errors.scenario_thread_spawn_failed': 'Failed to start the scenario run thread',
  'errors.scenario_run_not_found': 'Scenario run not found',
  'errors.unknown_report_format': 'Unknown report format',
  'errors.report_not_ready': 'The scenario has not finished yet; no report is available',
}
