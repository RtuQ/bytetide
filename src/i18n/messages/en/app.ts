import type { app as appZh } from '../zh-CN/app'

/** English · app 域（应用壳）。键集必须与 zh-CN 逐键对应（vue-tsc 强制）。 */
export const app: Record<keyof typeof appZh, string> = {
  'app.title': 'ByteTide',
  'app.settings.langToggle': 'Switch to 中文',
  'app.settings.langSub': 'Display language',
  'app.selftest.interp': 'value={v}',
}
