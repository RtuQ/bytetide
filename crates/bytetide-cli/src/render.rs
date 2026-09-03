//! 数据行渲染（纯函数）：默认 `RX/TX  文本` 两空格列；--ts 加时间戳列；--json 逐行 JSON。

use bytetide_core::serial::manager::BridgeLine;
use bytetide_core::serial::port::Dir;

/// 人类可读单行。color=false 输出纯文本（测试按字节断言）；
/// 彩色模式：RX 绿（≈#34d399）、TX 琥珀（≈#fbbf24）、时间戳暗灰。
/// TX 回显文本结尾的换行（ASCII 发送自动补的 \n）不重复占一行。
pub fn render_line(line: &BridgeLine, show_ts: bool, color: bool) -> String {
    let text = line.text.strip_suffix('\n').unwrap_or(&line.text);
    let mut s = String::with_capacity(text.len() + 24);
    if show_ts {
        if color {
            s.push_str(&console::Style::new().dim().apply_to(&line.ts).to_string());
        } else {
            s.push_str(&line.ts);
        }
        s.push_str("  ");
    }
    let dir = if line.dir == Dir::Rx { "RX" } else { "TX" };
    if color {
        let style = if line.dir == Dir::Rx {
            console::Style::new().green()
        } else {
            console::Style::new().yellow()
        };
        s.push_str(&style.apply_to(dir).to_string());
    } else {
        s.push_str(dir);
    }
    s.push_str("  ");
    s.push_str(text);
    s
}

/// JSON Lines 模式（忽略 --ts）：BridgeLine 原样序列化（camelCase，bytes 仅非 UTF-8 行携带）
pub fn render_json(line: &BridgeLine) -> String {
    serde_json::to_string(line).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(no: u64, dir: Dir, text: &str) -> BridgeLine {
        BridgeLine {
            no,
            ts: "14:32:07.118".into(),
            dir,
            text: text.into(),
            bytes: None,
            epoch_millis: 1234,
            r#match: None,
        }
    }

    #[test]
    fn plain_columns_exact() {
        assert_eq!(render_line(&line(1, Dir::Rx, "hello"), false, false), "RX  hello");
        assert_eq!(render_line(&line(2, Dir::Tx, "ping"), false, false), "TX  ping");
    }

    #[test]
    fn ts_column_exact() {
        assert_eq!(
            render_line(&line(1, Dir::Rx, "hello"), true, false),
            "14:32:07.118  RX  hello"
        );
        assert_eq!(
            render_line(&line(2, Dir::Tx, "ping"), true, false),
            "14:32:07.118  TX  ping"
        );
    }

    #[test]
    fn tx_echo_trailing_newline_not_doubled() {
        // ASCII 发送自动补的 \n 会进 TX 回显文本：显示时去掉，避免多出空行
        assert_eq!(render_line(&line(1, Dir::Tx, "ping\n"), false, false), "TX  ping");
    }

    #[test]
    fn color_mode_keeps_content() {
        // 彩色输出含 ANSI 码（由 console 按 tty 决定），只断言内容在场
        let s = render_line(&line(1, Dir::Rx, "hello"), false, true);
        assert!(s.contains("RX") && s.contains("hello"));
    }

    #[test]
    fn json_exact() {
        assert_eq!(
            render_json(&line(1, Dir::Rx, "hello")),
            r#"{"no":1,"ts":"14:32:07.118","dir":"rx","text":"hello","epochMillis":1234}"#
        );
    }

    #[test]
    fn json_with_bytes() {
        let mut l = line(2, Dir::Tx, "\u{fffd}");
        l.bytes = Some(vec![0xAA, 0x55]);
        let s = render_json(&l);
        assert!(s.contains(r#""bytes":[170,85]"#));
        assert!(s.contains(r#""dir":"tx""#));
    }
}
