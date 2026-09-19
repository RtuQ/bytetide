/**
 * 中文词典 · app 域：应用壳（App.vue / TitleBar / TabBar / StatusBar / SplitView /
 * SessionColumn / SettingsPopover 语言切换项）。键的唯一事实来源（MessageKey 由
 * 全部域合并而来）；en 域必须逐键对应（vue-tsc + 单测双重强制）。
 */
export const app = {
  'app.title': 'ByteTide · 字节潮',
  'app.settings.langToggle': '切换到 English',
  'app.settings.langSub': '界面显示语言',
  // 仅供 i18n 单测覆盖 {name} 插值，勿在业务里使用（删除需同步删测试）
  'app.selftest.interp': '值={v}',

  // ---- 通用（标题行窗口控制 / 弹层关闭 / 标签关闭共用的「关闭」）----
  'app.common.close': '关闭',

  // ---- 视图四态（viewbar seg + 日志↔图表分割条）----
  'app.view.group': '中心视图模式',
  'app.view.log': '日志',
  'app.view.logTitle': '仅日志',
  'app.view.split': '分屏',
  'app.view.splitTitle': '日志与图表同屏，可拖分割条调整高度',
  'app.view.plot': '图表',
  'app.view.plotTitle': '仅图表',
  'app.view.compare': '对比',
  'app.view.compareTitle': '双会话时间对齐对比（占中心区）',
  'app.view.compareDisabledTitle': '双会话时间对齐对比：需先打开第二个会话（离线日志也可）',
  'app.view.splitAria': '调整日志与图表高度',
  'app.view.splitDrag': '拖动调整日志/图表高度',

  // ---- 欢迎屏（无活动会话的空状态）----
  'app.welcome.heading': '开始调试你的串口',
  'app.welcome.sub': '连接设备或打开日志文件，实时查看、搜索和分析字节流。',
  'app.welcome.newConn': '新建连接',
  'app.welcome.openLog': '打开日志文件',
  'app.welcome.hintTransport': '支持串口、TCP、UDP',
  'app.welcome.hintOffline': '日志可离线分析',

  // ---- 侧栏分组头 / 拖宽手柄 ----
  'app.group.find': '查找',
  'app.group.rules': '规则',
  'app.group.data': '数据',
  'app.group.library': '库',
  'app.sidebar.resizeTitle': '拖动调整侧栏宽度',
  'app.sidebar.resizeAria': '调整侧栏宽度',
  'app.sidebar.expand': '展开侧栏',
  'app.sidebar.collapse': '收起侧栏',

  // ---- 标题行：更新面板 / 窗口控制 ----
  'app.update.check': '检查更新',
  'app.update.closeAria': '关闭更新面板',
  'app.update.newVersion': '发现新版本',
  'app.update.currentVer': '当前 v{version}',
  'app.update.goDownload': '前往下载页',
  'app.update.dismiss': '忽略此版本',
  'app.update.checking': '正在检查更新…',
  'app.update.latest': '已是最新版本（v{version}）',
  'app.update.errorPrefix': '检查失败：{msg}',
  'app.update.netUnavailable': '网络不可用',
  'app.update.retry': '重试',
  'app.update.goReleases': '前往 Releases 页',
  'app.update.unconfigured': '更新仓库尚未配置，请前往 GitHub Releases 页手动下载新版本。',
  'app.update.currentLine': '当前版本 v{version}',
  'app.win.minimize': '最小化',
  'app.win.maximize': '最大化 / 还原',
  'app.win.maximizeAria': '最大化或还原',

  // ---- 标签行 ----
  'app.tab.closeAria': '关闭标签',
  'app.tab.openLogAria': '打开日志',
  'app.tab.openLogTitle': '打开日志文件进行离线分析',
  'app.tab.opening': '打开中…',

  // ---- 状态栏（含会话状态 code→词条映射）----
  'app.status.connected': '已连接',
  'app.status.connecting': '连接中',
  'app.status.disconnected': '已断开',
  'app.status.error': '错误',
  'app.status.offline': '离线',
  'app.status.rxRate': '接收速率',
  'app.status.txRate': '发送速率',
  'app.status.dropped': '丢行 {count}',
  'app.status.droppedTitle': '前端缓冲裁剪掉的行数（重连迁移保留）',
  'app.status.ringDropped': 'Ring 丢 {count}',
  'app.status.ringDroppedTitle': '后端 ring 容量窗口内未来得及拉取就被覆盖的行（前端停顿过长时发生）',
  'app.status.capturing': '捕获中',
  'app.status.capturingTitle': '现场捕获进行中：命中「{pattern}」，正在写后续窗口，完成后自动存档',
  'app.status.lag': '滞后 {lag}ms · 批均 {batch}ms',
  'app.status.lagTitle': '显示滞后=当前墙钟−最新行后端时间戳；批均=单批次处理耗时',
  'app.status.lines': 'RX {rx} · TX {tx} 行',
  'app.status.noSession': '无活动会话',

  // ---- 分屏列（SplitView / SessionColumn）----
  'app.col.add': '增加一列',
  'app.col.pickTitle': '选择该列显示的会话',
  'app.col.pick': '选择会话',
  'app.col.remove': '删除该列',
  'app.col.searchPlaceholder': '搜索…',
  'app.col.regex': '正则',
  'app.col.newline': '换行',
  'app.col.send': '发送',
  'app.col.hexPlaceholder': 'Hex，如 41 42 43',
  'app.col.textPlaceholder': '发送内容（Ctrl+Enter 发送）',
  'app.col.history': '发送历史 ({count})',
  'app.col.empty': '在顶部选择一个会话',
} as const
