//! 用户可见错误消息的 `"{code}|{detail}"` 前缀约定（i18n 阶段 2）。
//!
//! 后端不再产出中文错误句子：`code` = 稳定 snake_case 错误码（前端词典
//! `errors.<code>` 承担文案，全集契约见 `src/ipc/errorCodes.ts`）；`detail` =
//! 技术细节（io::Error 文本、端口名、路径、会话 id 等，不翻译）。detail 允许为
//! 空，但分隔符恒在——前端 `parseCmdError` 依赖 `|` 识别码，无前缀回退 generic。

/// 组装 `"{code}|{detail}"` 错误串（code 稳定、detail 技术细节不翻译）。
pub(crate) fn err_msg(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}|{detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn err_msg_keeps_separator_even_with_empty_detail() {
        // detail 为空也保留 `|`：前端 parseCmdError 靠分隔符识别 code
        assert_eq!(err_msg("session_not_found", ""), "session_not_found|");
        assert_eq!(err_msg("read_failed", "boom"), "read_failed|boom");
        assert_eq!(err_msg("open_port_failed", 5), "open_port_failed|5");
    }
}
