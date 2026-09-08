//! 会话规则评估（自动回复 / 告警）：纯函数匹配与状态机，供 stream_loop 逐行调用。
//! 规则配置由前端推送（`set_live_rules_cmd`），评估在后端读线程内完成——
//! 设备交互的正确性不依赖 WebView 存活（拉模型下前端只负责视图）。

use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::HashMap;

// ============ 配置（serde camelCase 对齐前端类型） ============

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoReplyRuleCfg {
    pub id: String,
    pub trigger: String,
    pub reply: String,
    pub use_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub append_newline: bool,
    /// "ascii" | "hex"
    pub reply_mode: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoReplyCfg {
    pub enabled: bool,
    pub rules: Vec<AutoReplyRuleCfg>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertRuleCfg {
    pub id: String,
    pub pattern: String,
    pub use_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub min_count: u32,
    pub window_sec: u64,
    pub cooldown_sec: u64,
    /// "info" | "warn" | "err"（透传给前端显示）
    pub level: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AlertCfg {
    pub enabled: bool,
    pub rules: Vec<AlertRuleCfg>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureRuleCfg {
    pub id: String,
    pub pattern: String,
    pub use_regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub enabled: bool,
}

/// 触发式现场捕获配置：命中规则把 ring 里前 pre_ms 窗口转储为档案，
/// 并继续写 post_ms 后续窗口（行车记录仪语义）。评估在读线程内逐行进行。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureCfg {
    pub enabled: bool,
    /// 断连/读错误时也回溯抓一段（设备重启/掉线现场）
    pub on_disconnect: bool,
    /// 触发前回溯窗口（毫秒）
    pub pre_ms: u64,
    /// 触发后继续录制窗口（毫秒）
    pub post_ms: u64,
    pub rules: Vec<CaptureRuleCfg>,
}

// ============ 匹配（语义对齐前端 buildTestMatcher） ============

/// 非正则=字面量转义；whole_word 仅对字面量包 `\b`（与前端一致）；
/// 非大小写敏感 → case_insensitive；非法正则返回 None（规则被跳过）。
pub fn build_test_matcher(
    pattern: &str,
    use_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
) -> Option<Regex> {
    if pattern.is_empty() {
        return None;
    }
    let src = if use_regex {
        pattern.to_string()
    } else {
        let esc = regex::escape(pattern);
        if whole_word {
            format!(r"\b{esc}\b")
        } else {
            esc
        }
    };
    RegexBuilder::new(&src)
        .case_insensitive(!case_sensitive)
        .build()
        .ok()
}

/// 自动回复：返回命中规则的回复负载（ascii 依 appendNewline 补 \n；hex 透传原文）。
/// 每行只按第一条命中规则回复一次（与前端 break 语义一致）。
pub fn auto_reply_payload(cfg: &AutoReplyCfg, text: &str) -> Option<(String, String)> {
    if !cfg.enabled {
        return None;
    }
    for r in &cfg.rules {
        if !r.enabled || r.trigger.is_empty() {
            continue;
        }
        if let Some(re) = build_test_matcher(&r.trigger, r.use_regex, r.case_sensitive, r.whole_word)
        {
            if re.is_match(text) {
                let payload = if r.reply_mode == "ascii" && r.append_newline {
                    format!("{}\n", r.reply)
                } else {
                    r.reply.clone()
                };
                if payload.is_empty() {
                    return None;
                }
                return Some((payload, r.reply_mode.clone()));
            }
        }
    }
    None
}

// ============ 告警状态机（窗口计数 + 冷却） ============

#[derive(Debug, Default, Clone)]
pub struct AlertWinState {
    pub win_start: u64,
    pub count: u32,
    pub last_fire: u64,
}

/// 评估一行 RX 文本，返回触发的规则（可能多个，按规则序）。
/// `states` 以 rule.id 为键（每会话独立持有）；容量控制由调用方负责。
pub fn alert_eval(
    cfg: &AlertCfg,
    states: &mut HashMap<String, AlertWinState>,
    text: &str,
    now_ms: u64,
) -> Vec<AlertRuleCfg> {
    let mut fired = Vec::new();
    if !cfg.enabled {
        return fired;
    }
    for r in &cfg.rules {
        if !r.enabled || r.pattern.is_empty() {
            continue;
        }
        let Some(re) = build_test_matcher(&r.pattern, r.use_regex, r.case_sensitive, r.whole_word)
        else {
            continue;
        };
        if !re.is_match(text) {
            continue;
        }
        let st = states.entry(r.id.clone()).or_default();
        let win_ms = r.window_sec.saturating_mul(1000);
        if win_ms == 0 || now_ms.saturating_sub(st.win_start) > win_ms {
            st.win_start = now_ms;
            st.count = 0;
        }
        st.count += 1;
        let need = r.min_count.max(1);
        if st.count < need {
            continue;
        }
        if r.cooldown_sec > 0
            && st.last_fire > 0
            && now_ms.saturating_sub(st.last_fire) < r.cooldown_sec.saturating_mul(1000)
        {
            continue;
        }
        st.last_fire = now_ms;
        st.count = 0;
        fired.push(r.clone());
    }
    fired
}

// ============ 现场捕获触发（无窗口/冷却——重复触发由“armed 顺延”合并） ============

/// 评估一行 RX 文本，返回命中的捕获规则（可能多个，按规则序）。
pub fn capture_eval(cfg: &CaptureCfg, text: &str) -> Vec<CaptureRuleCfg> {
    let mut hit = Vec::new();
    if !cfg.enabled {
        return hit;
    }
    for r in &cfg.rules {
        if !r.enabled || r.pattern.is_empty() {
            continue;
        }
        if let Some(re) = build_test_matcher(&r.pattern, r.use_regex, r.case_sensitive, r.whole_word)
        {
            if re.is_match(text) {
                hit.push(r.clone());
            }
        }
    }
    hit
}

/// 窗口钳制：pre/post 下限 1s（避免 0 造成瞬时关闭）、上限 30min（防档案无限膨胀）。
pub fn clamp_capture_window(ms: u64) -> u64 {
    ms.clamp(1_000, 1_800_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ar_rule(trigger: &str, reply: &str) -> AutoReplyRuleCfg {
        AutoReplyRuleCfg {
            id: "r1".into(),
            trigger: trigger.into(),
            reply: reply.into(),
            use_regex: false,
            case_sensitive: false,
            whole_word: false,
            append_newline: false,
            reply_mode: "ascii".into(),
            enabled: true,
        }
    }

    fn al_rule(pattern: &str) -> AlertRuleCfg {
        AlertRuleCfg {
            id: "a1".into(),
            pattern: pattern.into(),
            use_regex: false,
            case_sensitive: false,
            whole_word: false,
            min_count: 1,
            window_sec: 0,
            cooldown_sec: 0,
            level: "warn".into(),
            enabled: true,
        }
    }

    #[test]
    fn auto_reply_matches_literal_and_appends_newline() {
        let cfg = AutoReplyCfg {
            enabled: true,
            rules: vec![ar_rule("ERROR", "RESET")],
        };
        let (payload, mode) = auto_reply_payload(&cfg, "x ERROR occurred").unwrap();
        assert_eq!(payload, "RESET"); // 大小写不敏感命中
        assert_eq!(mode, "ascii");

        // 追加换行语义
        let mut r = ar_rule("ok", "ACK");
        r.append_newline = true;
        let cfg2 = AutoReplyCfg { enabled: true, rules: vec![r] };
        assert_eq!(auto_reply_payload(&cfg2, "ok").unwrap().0, "ACK\n");

        // 禁用/无命中/空回复
        let off = AutoReplyCfg { enabled: false, rules: vec![ar_rule("ERROR", "R")] };
        assert!(auto_reply_payload(&off, "ERROR").is_none());
        let nohit = AutoReplyCfg { enabled: true, rules: vec![ar_rule("ERROR", "R")] };
        assert!(auto_reply_payload(&nohit, "all good").is_none());
    }

    #[test]
    fn auto_reply_first_matching_rule_wins() {
        let cfg = AutoReplyCfg {
            enabled: true,
            rules: vec![ar_rule("a", "FIRST"), ar_rule("b", "SECOND")],
        };
        // 行同时含 a 和 b：按规则序只回第一条
        assert_eq!(auto_reply_payload(&cfg, "a+b").unwrap().0, "FIRST");
    }

    #[test]
    fn alert_window_count_and_cooldown() {
        let mut r = al_rule("fail");
        r.min_count = 2; // 窗口内 2 次才触发
        r.window_sec = 10;
        r.cooldown_sec = 60;
        let cfg = AlertCfg { enabled: true, rules: vec![r] };
        let mut states = HashMap::new();

        // 第 1 次命中：不足阈值
        assert!(alert_eval(&cfg, &mut states, "fail", 1_000).is_empty());
        // 第 2 次（同窗口）：触发
        assert_eq!(alert_eval(&cfg, &mut states, "fail", 2_000).len(), 1);
        // 冷却期内再触发：被抑制（计数已清零，重新累计也只到 1）
        assert!(alert_eval(&cfg, &mut states, "fail", 3_000).is_empty());
        // 冷却过后：计数 1 不足阈值
        assert!(alert_eval(&cfg, &mut states, "fail", 70_000).is_empty());
        // 第 2 次：再次触发
        assert_eq!(alert_eval(&cfg, &mut states, "fail", 71_000).len(), 1);
    }

    #[test]
    fn alert_window_expiry_resets_count() {
        let mut r = al_rule("fail");
        r.min_count = 2;
        r.window_sec = 10;
        let cfg = AlertCfg { enabled: true, rules: vec![r] };
        let mut states = HashMap::new();
        assert!(alert_eval(&cfg, &mut states, "fail", 1_000).is_empty());
        // 窗口已过（>10s）：计数清零重新开始
        assert!(alert_eval(&cfg, &mut states, "fail", 20_000).is_empty());
        // 新窗口内第 2 次命中：达到阈值触发（fired 后计数清零）
        assert_eq!(alert_eval(&cfg, &mut states, "fail", 21_000).len(), 1);
        // 触发后计数清零：下一行不足阈值
        assert!(alert_eval(&cfg, &mut states, "fail", 22_000).is_empty());
    }

    #[test]
    fn alert_regex_and_case_flag() {
        let mut r = al_rule("(?i)error");
        r.use_regex = true;
        let cfg = AlertCfg { enabled: true, rules: vec![r] };
        let mut states = HashMap::new();
        assert_eq!(alert_eval(&cfg, &mut states, "SeVeRe ErRoR", 1).len(), 1);
    }

    #[test]
    fn alert_disabled_or_bad_pattern_never_fires() {
        let off = AlertCfg { enabled: false, rules: vec![al_rule("x")] };
        let mut s = HashMap::new();
        assert!(alert_eval(&off, &mut s, "x", 1).is_empty());
        let bad = AlertCfg {
            enabled: true,
            rules: vec![al_rule("(unclosed")],
        };
        assert!(alert_eval(&bad, &mut s, "x", 1).is_empty());
    }

    // ---------- 现场捕获触发 ----------

    fn cap_rule(pattern: &str, use_regex: bool) -> CaptureRuleCfg {
        CaptureRuleCfg {
            id: "c1".into(),
            pattern: pattern.into(),
            use_regex,
            case_sensitive: false,
            whole_word: false,
            enabled: true,
        }
    }

    #[test]
    fn capture_eval_matches_and_respects_enable() {
        let cfg = CaptureCfg {
            enabled: true,
            on_disconnect: false,
            pre_ms: 60_000,
            post_ms: 30_000,
            rules: vec![cap_rule("ERROR|Fault", true), cap_rule("overtemp", false)],
        };
        let hits = capture_eval(&cfg, "ERROR: OVERTEMP");
        assert_eq!(hits.len(), 2); // 正则命中 ERROR + 字面量命中 OVERTEMP
        assert!(capture_eval(&cfg, "all normal").is_empty());

        let off = CaptureCfg { enabled: false, ..cfg.clone() };
        assert!(capture_eval(&off, "ERROR").is_empty());

        // 非法正则被跳过；disabled 规则被跳过
        let bad = CaptureCfg {
            enabled: true,
            rules: vec![cap_rule("(unclosed", true)],
            ..Default::default()
        };
        assert!(capture_eval(&bad, "x").is_empty());
        let mut disabled = cap_rule("x", false);
        disabled.enabled = false;
        let cfg2 = CaptureCfg { enabled: true, rules: vec![disabled], ..Default::default() };
        assert!(capture_eval(&cfg2, "x").is_empty());
    }

    #[test]
    fn capture_window_clamped() {
        assert_eq!(clamp_capture_window(0), 1_000);
        assert_eq!(clamp_capture_window(999), 1_000);
        assert_eq!(clamp_capture_window(60_000), 60_000);
        assert_eq!(clamp_capture_window(u64::MAX), 1_800_000);
    }
}
