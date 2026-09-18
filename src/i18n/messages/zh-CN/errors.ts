/**
 * 中文词典 · errors 域：后端错误码文案（`errors.<code>`，code 全集见
 * src/ipc/errorCodes.ts，由 Rust 错误码机制生成）。展示统一走
 * src/ipc/errors.ts 的 errorMessage（词条 + 技术细节括注）。
 */
export const errors = {
  'errors.generic': '操作失败',
} as const
