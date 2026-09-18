// IPC 错误归一化：Tauri 命令失败时 reject 的是字符串（Rust Err(String)）。
// i18n 起后端错误走 "{code}|{detail}" 前缀约定（code = 稳定错误码，全集见
// src/ipc/errorCodes.ts；detail = 技术细节如 OS 文本，不翻译）；无合法前缀的
// 旧格式/异常对象按 generic 处理（detail=原文），向后兼容。用户可见展示统一走
// errorMessage（errors.<code> 词条 + 细节括注），勿再直接 String(e)。

import { tDynamic } from '../i18n'

/** 默认归一化：与既有 `String(e)` 调用点逐字等价（多数 IPC 错误本来就是字符串） */
export function normalizeIpcError(e: unknown): string {
  return String(e)
}

/** 取消息体：与既有 `String(e instanceof Error ? e.message : e)` 等价（Error 去掉前缀） */
export function ipcErrorDetail(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export interface CmdError {
  /** 稳定错误码（Rust 侧 cmd_err 生成；全集见 src/ipc/errorCodes.ts） */
  code: string
  /** 技术细节（OS/IO 文本、端口名等，不翻译，括注展示） */
  detail: string
}

const CODE_RE = /^[a-z][a-z0-9_]*$/

/** 解析 "{code}|{detail}"；无合法前缀 → generic（detail=原文） */
export function parseCmdError(e: unknown): CmdError {
  const raw = ipcErrorDetail(e)
  const i = raw.indexOf('|')
  if (i > 0) {
    const code = raw.slice(0, i)
    if (CODE_RE.test(code)) return { code, detail: raw.slice(i + 1) }
  }
  return { code: 'generic', detail: raw }
}

/** 本地化错误文案：`errors.<code>` 词条 + 技术细节括注；未知码/纯文本回退原样 */
export function errorMessage(e: unknown): string {
  const { code, detail } = parseCmdError(e)
  const msg = tDynamic(`errors.${code}`, '')
  if (!msg) return detail
  return detail ? `${msg} (${detail})` : msg
}
