// IPC 错误归一化：Tauri 命令失败时 reject 的是字符串（Rust Err(String)），
// 现状各调用点用 String(e) 呈现（toast/alert/lastError）。语义必须保持——
// String(字符串) 原样、String(Error) 带前缀，文案进提示框的形状不变。

/** 默认归一化：与既有 `String(e)` 调用点逐字等价（多数 IPC 错误本来就是字符串） */
export function normalizeIpcError(e: unknown): string {
  return String(e)
}

/** 取消息体：与既有 `String(e instanceof Error ? e.message : e)` 等价（Error 去掉前缀） */
export function ipcErrorDetail(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}
