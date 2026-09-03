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
}

/// 按自定义 token 替换模板。
/// 这些 token 与 chrono 的 format spec 不同（如 %D / %s / %t），故不直接喂 chrono：
///   %Y 年(4位)   %M 月(01-12)  %D 日(01-31)  %H 端口名
///   %h 时(00-23) %m 分(00-59)  %s 秒(00-59)  %t 毫秒(000-999)
///   %% 字面 %
/// 未知 token（如 %x）原样保留为 "%x"。
pub fn format_tokens(tmpl: &str, dt: &DateTime<Local>, port: &str) -> String {
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
            'H' => Some(port.to_string()),
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
