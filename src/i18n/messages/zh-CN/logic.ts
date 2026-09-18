/**
 * 中文词典 · logic 域：逻辑层用户可见字符串（useTauriEvents / useToast /
 * stores/session/compat / stores/automation / stores/session/presets /
 * useUpdateChecker / usePlotParser / useParserEngine / parser/fields）。
 * 动态映射表（告警级别、错误标题组）只存 MessageKey，使用点 t() 求值。
 */
export const logic = {
  // ---- useToast：连接报错四类标题/动作（connectionErrorHint 按 code 分派）----
  'logic.toast.errBusyTitle': '端口可能已被占用',
  'logic.toast.errBusyAction': '请关闭其他串口工具后重试',
  'logic.toast.errNotFoundTitle': '端口已不可用',
  'logic.toast.errNotFoundAction': '刷新端口列表后重新选择',
  'logic.toast.errTimeoutTitle': '连接没有建立',
  'logic.toast.errTimeoutAction': '检查设备、电缆或网络地址后重试',
  'logic.toast.errConnTitle': '连接失败',
  'logic.toast.errConnAction': '检查参数后重试',

  // ---- useTauriEvents：连接/断开/热插拔 toast ----
  'logic.toast.connected': '连接成功',
  'logic.toast.disconnected': '连接已断开',
  'logic.toast.portArrived': '串口已接入 · {name}',
  'logic.toast.portRemoved': '串口已移除 · {name}',

  // ---- useTauriEvents：告警级别标签 + 摘录空行兜底 ----
  'logic.alert.levelInfo': '提示',
  'logic.alert.levelWarn': '警告',
  'logic.alert.levelErr': '错误',
  'logic.alert.emptyLine': '(空行)',

  // ---- useTauriEvents：现场捕获档案落成 ----
  'logic.capture.saved': '现场捕获已保存',

  // ---- stores/session/compat：打开日志失败 + 会话名回退（创建时刻求值）----
  'logic.log.openFailed': '无法打开日志文件',
  'logic.log.noLogFile': '当前会话尚未生成日志文件',
  'logic.session.offlineName': '离线日志',
  'logic.session.replayName': '回放日志',

  // ---- stores/automation：副本后缀 + 保存预检 ----
  'logic.scen.copySuffix': '{name} 副本',
  'logic.scen.emptyName': '名称不能为空',
  'logic.scen.validationFailed': '场景校验失败',

  // ---- stores/session/presets：默认预设名（创建时刻求值）----
  'logic.preset.defaultName': '{category} 预设',

  // ---- useUpdateChecker：检查更新错误（throw 点求值）----
  'logic.update.noReleases': '仓库还没有发布版本',
  'logic.update.apiStatus': 'GitHub API 返回 {status}',
  'logic.update.badResponse': '更新响应格式异常',

  // ---- usePlotParser：绘图配置校验 ----
  'logic.plot.needHeadOrTail': '需设置帧头或帧尾',
  'logic.plot.invalidLayout': '通道/字节配置无效',

  // ---- useParserEngine：framing 摘要标签 + 无脚本 ----
  'logic.parser.lenFixed': '定长 {n}B',
  'logic.parser.lenField': '长度域 {fmt}@{at}+{add}',
  'logic.parser.lenUntil': '分隔符 {hex}',
  'logic.parser.lenLine': '整行一帧',
  'logic.parser.srcBinary': '二进制',
  'logic.parser.sync': '同步 {hex}',
  'logic.parser.crc': 'CRC {algo}',
  'logic.parser.noScript': '没有已加载的脚本',

  // ---- parser/fields：type 类型名与字段位置标注 ----
  'logic.parser.typeUnknown': '未知',
  'logic.parser.outOfRange': '越界@{at}',
} as const
