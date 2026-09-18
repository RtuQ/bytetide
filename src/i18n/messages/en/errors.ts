import type { errors as errorsZh } from '../zh-CN/errors'

/** English · errors 域（后端错误码文案，code 全集见 src/ipc/errorCodes.ts）。 */
export const errors: Record<keyof typeof errorsZh, string> = {
  'errors.generic': 'Operation failed',
}
