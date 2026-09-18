import { app } from './app'
import { logview } from './logview'
import { send } from './send'
import { rule } from './rule'
import { lib } from './lib'
import { scen } from './scen'
import { dock } from './dock'
import { logic } from './logic'
import { errors } from './errors'

/** 中文词典（键的唯一事实来源）：MessageKey = 全部域键的并集。
 *  词典按域拆文件（app/logview/send/rule/lib/scen/dock/logic/errors）——
 *  i18n 作业按域并行，新增词条只写自己域的一对 zh/en 文件，本入口不改。 */
export const zhCN = {
  ...app,
  ...logview,
  ...send,
  ...rule,
  ...lib,
  ...scen,
  ...dock,
  ...logic,
  ...errors,
} as const

export type MessageKey = keyof typeof zhCN
export type Messages = Record<MessageKey, string>
