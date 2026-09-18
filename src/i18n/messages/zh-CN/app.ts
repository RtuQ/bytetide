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
} as const
