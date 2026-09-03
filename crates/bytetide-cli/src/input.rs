//! stdin 交互命令解析（纯函数）：普通行按当前模式发送，/hex /mode /quit 为命令。

/// 默认发送模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Ascii,
    Hex,
}

/// 解析后的交互命令
#[derive(Debug, PartialEq)]
pub enum InputCmd {
    /// 普通行：按当前模式发送
    Send { text: String },
    /// /hex 单次十六进制发送（原文，发送前校验）
    SendHex { text: String },
    /// /mode 切换默认模式
    SetMode(Mode),
    /// /quit 退出
    Quit,
    /// 空行/无操作
    Noop,
}

/// 解析一行 stdin 输入。命令匹配基于 trim 后文本；普通发送保留原文（仅去行尾 CR/LF）。
pub fn parse_input(line: &str) -> InputCmd {
    let t = line.trim();
    if t.is_empty() {
        return InputCmd::Noop; // 空行（含纯空白）不发送
    }
    if t == "/quit" || t == "/exit" {
        return InputCmd::Quit;
    }
    if let Some(rest) = t.strip_prefix("/hex") {
        // "/hex AA 01" 是命令；"/hexAA" 之类的非命令按普通文本发送
        if rest.is_empty() {
            return InputCmd::Noop;
        }
        if rest.starts_with(char::is_whitespace) {
            let hex = rest.trim();
            if hex.is_empty() {
                return InputCmd::Noop;
            }
            return InputCmd::SendHex { text: hex.to_string() };
        }
    }
    match t {
        "/mode ascii" => return InputCmd::SetMode(Mode::Ascii),
        "/mode hex" => return InputCmd::SetMode(Mode::Hex),
        _ => {}
    }
    // /mode 参数非法：不发送，避免把误敲的命令打到设备
    if t == "/mode" || t.starts_with("/mode ") {
        return InputCmd::Noop;
    }
    InputCmd::Send {
        text: line.trim_end_matches(['\r', '\n']).to_string(),
    }
}

/// 十六进制串 → 字节（忽略空白、两两成对）；非法输入返回 Err(原因)，不产生部分发送。
pub fn parse_hex_pairs(s: &str) -> Result<Vec<u8>, String> {
    let mut cleaned = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        if !c.is_ascii_hexdigit() {
            return Err(format!("无效十六进制字符 '{c}'"));
        }
        cleaned.push(c);
    }
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(format!("十六进制位数为奇数: {cleaned}"));
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .map_err(|e| format!("十六进制解析失败: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_send() {
        assert_eq!(parse_input("hello"), InputCmd::Send { text: "hello".into() });
        // 前后空白保留原样发送（仅去行尾 CR/LF）
        assert_eq!(parse_input(" hello "), InputCmd::Send { text: " hello ".into() });
    }

    #[test]
    fn empty_and_whitespace_noop() {
        assert_eq!(parse_input(""), InputCmd::Noop);
        assert_eq!(parse_input("   "), InputCmd::Noop);
        assert_eq!(parse_input("\t\r"), InputCmd::Noop);
    }

    #[test]
    fn quit_and_exit() {
        assert_eq!(parse_input("/quit"), InputCmd::Quit);
        assert_eq!(parse_input("  /quit  "), InputCmd::Quit);
        assert_eq!(parse_input("/exit"), InputCmd::Quit);
    }

    #[test]
    fn hex_command() {
        assert_eq!(
            parse_input("/hex AA 01 5a"),
            InputCmd::SendHex { text: "AA 01 5a".into() }
        );
        assert_eq!(parse_input("/hex\tAA"), InputCmd::SendHex { text: "AA".into() });
        assert_eq!(parse_input("/hex"), InputCmd::Noop);
        assert_eq!(parse_input("/hex   "), InputCmd::Noop);
        // /hexXX 不是命令：按普通文本发送
        assert_eq!(parse_input("/hexAA"), InputCmd::Send { text: "/hexAA".into() });
    }

    #[test]
    fn mode_command() {
        assert_eq!(parse_input("/mode ascii"), InputCmd::SetMode(Mode::Ascii));
        assert_eq!(parse_input("/mode hex"), InputCmd::SetMode(Mode::Hex));
        assert_eq!(parse_input("/mode bin"), InputCmd::Noop);
        assert_eq!(parse_input("/mode"), InputCmd::Noop);
        // "/model" 是普通文本，不被 /mode 吞掉
        assert_eq!(parse_input("/model X"), InputCmd::Send { text: "/model X".into() });
    }

    #[test]
    fn hex_pairs_valid() {
        assert_eq!(parse_hex_pairs("AA 01 5a"), Ok(vec![0xAA, 0x01, 0x5A]));
        assert_eq!(parse_hex_pairs("aabb"), Ok(vec![0xAA, 0xBB]));
        assert_eq!(parse_hex_pairs("AA\t01\n02"), Ok(vec![0xAA, 0x01, 0x02]));
        assert_eq!(parse_hex_pairs(""), Ok(vec![]));
        assert_eq!(parse_hex_pairs("   "), Ok(vec![]));
    }

    #[test]
    fn hex_pairs_invalid() {
        assert!(parse_hex_pairs("ABC").is_err()); // 奇数位
        assert!(parse_hex_pairs("GG").is_err()); // 非十六进制
        assert!(parse_hex_pairs("AA GG").is_err());
    }
}
