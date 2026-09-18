import type { Messages } from '../zh-CN/index'
import { app } from './app'
import { logview } from './logview'
import { send } from './send'
import { rule } from './rule'
import { lib } from './lib'
import { scen } from './scen'
import { dock } from './dock'
import { logic } from './logic'
import { errors } from './errors'

/** English 词典：Record<MessageKey, string> 注解对全词典做「缺键/多键」编译期
 *  强制（每个 en 域文件另有对其 zh 域的键集约束，双保险）。 */
export const en: Messages = {
  ...app,
  ...logview,
  ...send,
  ...rule,
  ...lib,
  ...scen,
  ...dock,
  ...logic,
  ...errors,
}
