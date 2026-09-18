import type { logic as logicZh } from '../zh-CN/logic'

/** English · logic 域（逻辑层字符串）。键集必须与 zh-CN 逐键对应（vue-tsc 强制）。 */
export const logic: Record<keyof typeof logicZh, string> = {
  // ---- useToast: connection error title/action pairs (dispatched by code) ----
  'logic.toast.errBusyTitle': 'Port may be in use',
  'logic.toast.errBusyAction': 'Close other serial tools and retry',
  'logic.toast.errNotFoundTitle': 'Port is no longer available',
  'logic.toast.errNotFoundAction': 'Refresh the port list and select again',
  'logic.toast.errTimeoutTitle': 'Connection not established',
  'logic.toast.errTimeoutAction': 'Check the device, cable, or network address and retry',
  'logic.toast.errConnTitle': 'Connection failed',
  'logic.toast.errConnAction': 'Check the settings and retry',

  // ---- useTauriEvents: connect / disconnect / hot-plug toasts ----
  'logic.toast.connected': 'Connected',
  'logic.toast.disconnected': 'Disconnected',
  'logic.toast.portArrived': 'Serial port plugged in · {name}',
  'logic.toast.portRemoved': 'Serial port unplugged · {name}',

  // ---- useTauriEvents: alert level labels + empty-line snippet fallback ----
  'logic.alert.levelInfo': 'Info',
  'logic.alert.levelWarn': 'Warning',
  'logic.alert.levelErr': 'Error',
  'logic.alert.emptyLine': '(empty line)',

  // ---- useTauriEvents: capture archive saved ----
  'logic.capture.saved': 'Capture archive saved',

  // ---- stores/session/compat: log open failure + session name fallbacks ----
  'logic.log.openFailed': 'Could not open log file',
  'logic.log.noLogFile': 'No log file has been created for this session yet',
  'logic.session.offlineName': 'Offline log',
  'logic.session.replayName': 'Replay log',

  // ---- stores/automation: copy suffix + save validation ----
  'logic.scen.copySuffix': '{name} copy',
  'logic.scen.emptyName': 'Name cannot be empty',
  'logic.scen.validationFailed': 'Scenario validation failed',

  // ---- stores/session/presets: default preset name ----
  'logic.preset.defaultName': '{category} preset',

  // ---- useUpdateChecker: update check errors ----
  'logic.update.noReleases': 'No releases have been published yet',
  'logic.update.apiStatus': 'GitHub API returned {status}',
  'logic.update.badResponse': 'Unexpected update response format',

  // ---- usePlotParser: plot config validation ----
  'logic.plot.needHeadOrTail': 'Frame head or tail is required',
  'logic.plot.invalidLayout': 'Invalid channel/byte configuration',

  // ---- useParserEngine: framing summary labels + no script ----
  'logic.parser.lenFixed': 'Fixed {n}B',
  'logic.parser.lenField': 'Len {fmt}@{at}+{add}',
  'logic.parser.lenUntil': 'Delimiter {hex}',
  'logic.parser.lenLine': 'One frame per line',
  'logic.parser.srcBinary': 'Binary',
  'logic.parser.sync': 'Sync {hex}',
  'logic.parser.crc': 'CRC {algo}',
  'logic.parser.noScript': 'No script loaded',

  // ---- parser/fields: type name and field position annotations ----
  'logic.parser.typeUnknown': 'Unknown',
  'logic.parser.outOfRange': 'out of range@{at}',
}
