//! 命令层错误码 helper（i18n 阶段 2）。
//!
//! Tauri 命令 `Result<T, String>` 的 Err 字符串统一走 `"{code}|{detail}"` 前缀约定：
//! `code` = 稳定 snake_case 错误码（前端词典 `errors.<code>` 承担文案，全集契约见
//! `src/ipc/errorCodes.ts`）；`detail` = 技术细节（io::Error 文本、端口名、会话 id、
//! core 透传的稳定码串等，不翻译）。core 侧同形错误经 `bytetide_core::errors` 生成、
//! `map_err(to_string)` 原样透传，本 helper 只用于命令层自产的错误。detail 允许为
//! 空，但分隔符恒在——前端 `parseCmdError` 依赖 `|` 识别码，无前缀回退 generic。

/// 组装 `"{code}|{detail}"` 命令错误串。
pub(crate) fn cmd_err(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}|{detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_err_keeps_separator_even_with_empty_detail() {
        assert_eq!(cmd_err("session_not_found", ""), "session_not_found|");
        assert_eq!(cmd_err("unknown_signal", "com9"), "unknown_signal|com9");
    }
}
