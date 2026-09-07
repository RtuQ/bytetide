use chrono::{DateTime, Datelike, Local, Timelike};
use serde::Deserialize;

/// 日志路径模板与每行时间戳格式配置（与前端 camelCase 字段对应）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfig {
    /// 日志文件路径模板；空表示使用默认 app_data 路径
    pub log_path_template: Option<String>,
    /// 每行时间戳格式模板；空表示使用默认 %h:%m:%s.%t
    pub line_ts_format: Option<String>,
    /// 跨天（本地午夜）自动另起新分段文件；缺省 None 按 false 处理（旧 JSON 兼容）
    pub midnight_rotate: Option<bool>,
}

/// 文件名清洗：把 `\\ / : * ? " < > |` 与控制字符替换为 `_`。
/// 只用于 token 替换值（端口名/主机地址来自外部配置，可能含文件名非法字符）；
/// 模板本身书写的 `\` `/` 路径分隔符不经此清洗。
pub fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
            {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// 按自定义 token 替换模板（原始值、不清洗；每行时间戳列 `format_ts` 走此入口）。
/// 这些 token 与 chrono 的 format spec 不同（如 %D / %s / %t），故不直接喂 chrono：
///   %Y 年(4位)   %M 月(01-12)  %D 日(01-31)  %H 端口名
///   %S 主机地址（网络源 = host:port，串口源 = 端口名；本函数取 port 兜底）
///   %h 时(00-23) %m 分(00-59)  %s 秒(00-59)  %t 毫秒(000-999)
///   %% 字面 %
/// 未知 token（如 %x）原样保留为 "%x"；行尾孤立的 `%` 原样保留。
pub fn format_tokens(tmpl: &str, dt: &DateTime<Local>, port: &str) -> String {
    format_tokens_ex(tmpl, dt, port, port, false)
}

/// 日志路径解析入口：token 替换值经 `sanitize_filename` 清洗（模板本身的
/// `\` `/` 分隔符保留），供 connect 生成落盘路径用。
pub fn format_path(tmpl: &str, dt: &DateTime<Local>, port: &str, host: &str) -> String {
    format_tokens_ex(tmpl, dt, port, host, true)
}

fn format_tokens_ex(
    tmpl: &str,
    dt: &DateTime<Local>,
    port: &str,
    host: &str,
    sanitize: bool,
) -> String {
    // 仅清洗外部来源的替换值（端口名/主机地址）；时间戳等内部 token 本就安全
    let subst = |raw: String| {
        if sanitize {
            sanitize_filename(&raw)
        } else {
            raw
        }
    };
    let mut out = String::with_capacity(tmpl.len());
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(&next) = chars.peek() else {
            out.push('%');
            break;
        };
        let replaced: Option<String> = match next {
            'Y' => Some(format!("{:04}", dt.year())),
            'M' => Some(format!("{:02}", dt.month())),
            'D' => Some(format!("{:02}", dt.day())),
            'H' => Some(subst(port.to_string())),
            'S' => Some(subst(host.to_string())),
            'h' => Some(format!("{:02}", dt.hour())),
            'm' => Some(format!("{:02}", dt.minute())),
            's' => Some(format!("{:02}", dt.second())),
            't' => Some(format!("{:03}", dt.timestamp_subsec_millis())),
            '%' => Some(String::from("%")),
            _ => None,
        };
        match replaced {
            Some(s) => {
                chars.next();
                out.push_str(&s);
            }
            None => out.push('%'),
        }
    }
    out
}

/// 用当前本地时间格式化时间戳。
pub fn format_ts(format: &str) -> String {
    let now = Local::now();
    format_tokens(format, &now, "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt() -> DateTime<Local> {
        chrono::Local.with_ymd_and_hms(2026, 9, 7, 23, 5, 9).unwrap()
    }

    #[test]
    fn expands_basic_tokens() {
        let dt = dt();
        assert_eq!(
            format_tokens("%Y-%M-%D_%h-%m-%s-%t", &dt, "COM3"),
            "2026-09-07_23-05-09-000"
        );
        assert_eq!(format_tokens("%H", &dt, "COM3"), "COM3");
        assert_eq!(format_tokens("%%", &dt, "COM3"), "%");
    }

    #[test]
    fn host_token_s_network_and_serial() {
        let dt = dt();
        // 串口源：host 实参 = 端口名（format_tokens 默认取 port 兜底）
        assert_eq!(format_tokens("%S", &dt, "COM3"), "COM3");
        // 网络源：format_path 显式传 host:port，清洗后冒号转 _
        assert_eq!(
            format_path("%S.log", &dt, "x", "192.168.1.9:9000"),
            "192.168.1.9_9000.log"
        );
    }

    #[test]
    fn format_path_sanitizes_values_but_keeps_template_separators() {
        let dt = dt();
        // 模板里的 / 分隔符保留；token 值中的非法字符替换为 _
        assert_eq!(
            format_path("logs/%H/%S", &dt, "COM?", "1.2.3.4:9000"),
            "logs/COM_/1.2.3.4_9000"
        );
        // `\/:*?"<>|` 与控制字符（\u{7} BEL）逐一替换
        assert_eq!(
            sanitize_filename("a/b\\c:d*e?f\"g<h>i|j\u{7}"),
            "a_b_c_d_e_f_g_h_i_j_"
        );
        // 时间戳 token（format_ts 走 raw）不受影响
        assert_eq!(format_ts("%Y-%M-%D").len(), 10);
    }

    #[test]
    fn unknown_token_and_lone_percent_kept_verbatim() {
        let dt = dt();
        assert_eq!(format_tokens("%x", &dt, "p"), "%x");
        assert_eq!(format_tokens("abc%", &dt, "p"), "abc%");
        assert_eq!(format_path("%x/%Q", &dt, "p", "h"), "%x/%Q");
    }
}
