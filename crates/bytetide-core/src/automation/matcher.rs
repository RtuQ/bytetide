//! 场景行匹配器与变量替换：serde 形状（[`LineMatcher`]）、预编译实现
//! （[`CompiledMatcher`]）与 `${name}` 模板替换原语（[`substitute`]）。
//!
//! hex/mask 严格解析语义对齐 src-tauri bridge 的 `parse_hex_strict`/`parse_mask_strict`
//! （去空白后必须非空、偶数长、纯 ASCII hex 对 / 每对为两位 hex 或 `??` 通配）——
//! core 不依赖 src-tauri，此处独立实现同语义版本。
//! 宽松解析会静默丢非法对导致「看似过滤、实为 match-all」，故一律报错。
//!
//! # 变量替换语法（V1 最小实现，无转义）
//! 替换 `${name}`，name 须匹配 `[A-Za-z_][A-Za-z0-9_]*`：
//! - `$$` 原样保留（不支持转义）；`$` 后跟非 `{` 原样保留；
//! - `${` 无闭合 `}` 或内容非合法变量名 → 整段按字面量保留（不算引用、不校验不替换）。
//!
//! 仅 `Send.text` 与 `Assert.message` 参与替换与引用检查；matcher 模式
//! （literal/regex/hex/mask）不做替换——保证能字面匹配含 `${` 的设备输出。

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::serial::port::Dir;

/// 行匹配器：`dir` 可选（`None` = RX/TX 都匹配）；`literal/regex/hex/mask` **恰一个**非空。
///
/// 反序列化宽松（缺字段走 `default`），「恰一个 pattern」由 [`compile_matcher`] 校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<Dir>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
}

/// 匹配器错误码（稳定字符串，GUI/CLI 按码分支，勿改字面量）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatcherErrorCode {
    /// 零个或多个 pattern 变体非空。
    Conflict,
    /// regex 编译失败。
    InvalidRegex,
    /// hex 非空/偶数长/纯 hex 对校验失败。
    InvalidHex,
    /// mask 每对须为 `??` 或两位 hex。
    InvalidMask,
}

impl MatcherErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "matcher_conflict",
            Self::InvalidRegex => "invalid_regex",
            Self::InvalidHex => "invalid_hex",
            Self::InvalidMask => "invalid_mask",
        }
    }
}

/// 匹配器编译错误（`compile_matcher`；validate 层包装为带步骤路径的
/// `ScenarioValidationError`，错误码一一映射）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatcherError {
    pub code: MatcherErrorCode,
    pub message: String,
}

impl MatcherError {
    fn new(code: MatcherErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for MatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for MatcherError {}

/// 预编译匹配器（validate 时编译一次，runner 直接复用，免每步重编译）。
#[derive(Debug, Clone)]
pub struct CompiledMatcher {
    pub dir: Option<Dir>,
    kind: MatcherKind,
}

#[derive(Debug, Clone)]
enum MatcherKind {
    Literal(String),
    Regex(Regex),
    Hex(Vec<u8>),
    Mask(Vec<Option<u8>>),
}

impl CompiledMatcher {
    /// 匹配器种类名（"literal"|"regex"|"hex"|"mask"，报告/调试用）。
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            MatcherKind::Literal(_) => "literal",
            MatcherKind::Regex(_) => "regex",
            MatcherKind::Hex(_) => "hex",
            MatcherKind::Mask(_) => "mask",
        }
    }

    /// regex 匹配器（捕获组提取用；其他种类为 `None`）。
    pub fn regex(&self) -> Option<&Regex> {
        match &self.kind {
            MatcherKind::Regex(re) => Some(re),
            _ => None,
        }
    }

    /// 行匹配语义（与 bridge 交换等待一致）：dir 不等跳过；literal/regex 对文本；
    /// hex/mask 对行字节——优先原始 `bytes`（仅该行含非法 UTF-8 时才附带），
    /// 缺失回退 text 的 UTF-8 编码；禁止把 lossy 文本（U+FFFD）再编码当字节用。
    pub fn matches(&self, line_dir: Dir, text: &str, bytes: Option<&[u8]>) -> bool {
        if let Some(d) = self.dir {
            if d != line_dir {
                return false;
            }
        }
        match &self.kind {
            MatcherKind::Literal(l) => text.contains(l.as_str()),
            MatcherKind::Regex(re) => re.is_match(text),
            MatcherKind::Hex(pat) => {
                let hay = bytes.unwrap_or(text.as_bytes());
                contains_subslice(hay, pat)
            }
            MatcherKind::Mask(mask) => {
                let hay = bytes.unwrap_or(text.as_bytes());
                mask_matches(hay, mask)
            }
        }
    }
}

/// 编译行匹配器：恰一个 pattern 变体非空 + 语法严格校验。
pub fn compile_matcher(input: &LineMatcher) -> Result<CompiledMatcher, MatcherError> {
    let set = usize::from(input.literal.is_some())
        + usize::from(input.regex.is_some())
        + usize::from(input.hex.is_some())
        + usize::from(input.mask.is_some());
    if set != 1 {
        return Err(MatcherError::new(
            MatcherErrorCode::Conflict,
            format!("exactly one of literal/regex/hex/mask must be set, got {set}"),
        ));
    }
    let kind = if let Some(literal) = &input.literal {
        MatcherKind::Literal(literal.clone())
    } else if let Some(pattern) = &input.regex {
        let re = Regex::new(pattern).map_err(|e| {
            MatcherError::new(
                MatcherErrorCode::InvalidRegex,
                format!("invalid regex {pattern:?}: {e}"),
            )
        })?;
        MatcherKind::Regex(re)
    } else if let Some(hex) = &input.hex {
        MatcherKind::Hex(compile_hex(hex)?)
    } else if let Some(mask) = &input.mask {
        MatcherKind::Mask(compile_mask(mask)?)
    } else {
        unreachable!("exactly one pattern variant is set");
    };
    Ok(CompiledMatcher {
        dir: input.dir,
        kind,
    })
}

/// 严格 hex：去空白后必须非空、偶数长、纯 ASCII hex 对（对齐 bridge parse_hex_strict）。
fn compile_hex(s: &str) -> Result<Vec<u8>, MatcherError> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidHex,
            "hex must not be empty",
        ));
    }
    if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidHex,
            format!("hex must be ASCII hex pairs, got {s:?}"),
        ));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidHex,
            format!("hex must be even-length pairs, got {s:?}"),
        ));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for i in (0..cleaned.len()).step_by(2) {
        out.push(u8::from_str_radix(&cleaned[i..i + 2], 16).expect("validated hex pair"));
    }
    Ok(out)
}

/// 严格 mask：去空白后非空、偶数长，每对为 `??`（通配）或两位 hex（对齐 bridge）。
fn compile_mask(s: &str) -> Result<Vec<Option<u8>>, MatcherError> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidMask,
            "mask must not be empty",
        ));
    }
    // 先确保纯 ASCII（hex 或 ?），后续按字节切片才安全
    if !cleaned.chars().all(|c| c == '?' || c.is_ascii_hexdigit()) {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidMask,
            format!("mask pairs must be hex digits or ??, got {s:?}"),
        ));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(MatcherError::new(
            MatcherErrorCode::InvalidMask,
            format!("mask must be even-length pairs, got {s:?}"),
        ));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let mut i = 0;
    while i < cleaned.len() {
        let pair = &cleaned[i..i + 2];
        if pair == "??" {
            out.push(None);
        } else if pair.chars().all(|c| c.is_ascii_hexdigit()) {
            out.push(Some(
                u8::from_str_radix(pair, 16).expect("validated mask pair"),
            ));
        } else {
            return Err(MatcherError::new(
                MatcherErrorCode::InvalidMask,
                format!("mask pairs must be hex digits or ??, got {s:?}"),
            ));
        }
        i += 2;
    }
    Ok(out)
}

fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    needle.len() <= hay.len() && hay.windows(needle.len()).any(|w| w == needle)
}

fn mask_matches(hay: &[u8], mask: &[Option<u8>]) -> bool {
    if mask.len() > hay.len() {
        return false;
    }
    hay.windows(mask.len()).any(|w| {
        w.iter()
            .zip(mask)
            .all(|(byte, m)| m.is_none_or(|v| *byte == v))
    })
}

// ============ 变量替换（文本层原语：校验的引用扫描与 runner 的展开共用） ============

/// 校验变量名：`[A-Za-z_][A-Za-z0-9_]*`。
pub(crate) fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 扫描 `${name}` 引用：返回（`${name}` 全段 span, name），按出现序。
/// `$` 后非 `{`、`${` 无闭合 `}`、内容非合法变量名 → 不是引用（字面量保留）。
pub(crate) fn find_refs(input: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let body_start = i + 2; // '{' 是单字节 → 切片边界安全
            if let Some(rel) = input[body_start..].find('}') {
                let name = &input[body_start..body_start + rel];
                if is_valid_var_name(name) {
                    let end = body_start + rel + 1;
                    out.push((i..end, name));
                    i = end;
                    continue;
                }
            }
        }
        i += 1; // 非引用：只跳过 '$'（ASCII，步进后仍是边界）
    }
    out
}

/// 展开 `input` 中的 `${name}` 引用（语法与边界见模块注释）。
/// 未定义引用报 [`VariableError`]——静态可达性只做保守估计，运行期此处兜底。
pub fn substitute(input: &str, vars: &BTreeMap<String, String>) -> Result<String, VariableError> {
    let refs = find_refs(input);
    if refs.is_empty() {
        return Ok(input.to_string());
    }
    let mut out = String::with_capacity(input.len());
    let mut last = 0;
    for (span, name) in refs {
        out.push_str(&input[last..span.start]);
        match vars.get(name) {
            Some(value) => out.push_str(value),
            None => {
                return Err(VariableError {
                    variable: name.to_string(),
                })
            }
        }
        last = span.end;
    }
    out.push_str(&input[last..]);
    Ok(out)
}

/// 变量替换错误：`${name}` 引用了未定义变量（如 Repeat `times=0` 时体内捕获的
/// 变量实际未定义——静态校验保守放行的场景在此兜底）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableError {
    pub variable: String,
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "undefined variable: ${{{}}}", self.variable)
    }
}

impl std::error::Error for VariableError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(dir: Option<Dir>, build: impl FnOnce(&mut LineMatcher)) -> LineMatcher {
        let mut m = LineMatcher {
            dir,
            literal: None,
            regex: None,
            hex: None,
            mask: None,
        };
        build(&mut m);
        m
    }

    fn compiled(dir: Option<Dir>, build: impl FnOnce(&mut LineMatcher)) -> CompiledMatcher {
        compile_matcher(&matcher(dir, build)).expect("matcher must compile")
    }

    #[test]
    fn compiles_each_kind() {
        let m = compiled(None, |m| m.literal = Some("OK".into()));
        assert_eq!(m.kind_name(), "literal");
        assert!(m.regex().is_none());

        let m = compiled(Some(Dir::Rx), |m| m.regex = Some("^PONG [0-9]+".into()));
        assert_eq!(m.kind_name(), "regex");
        assert!(m.regex().is_some());
        assert_eq!(m.dir, Some(Dir::Rx));

        let m = compiled(None, |m| m.hex = Some("50 4f 47".into()));
        assert_eq!(m.kind_name(), "hex");
        assert!(matches!(&m.kind, MatcherKind::Hex(b) if b == &vec![0x50, 0x4f, 0x47]));

        let m = compiled(None, |m| m.mask = Some("5a ?? 4f".into()));
        assert_eq!(m.kind_name(), "mask");
        assert!(matches!(&m.kind, MatcherKind::Mask(v) if *v == [Some(0x5a), None, Some(0x4f)]));
    }

    #[test]
    fn conflict_on_zero_or_multiple_patterns() {
        for bad in [
            matcher(None, |_| {}),
            matcher(None, |m| {
                m.literal = Some("a".into());
                m.regex = Some("a".into());
            }),
            matcher(None, |m| {
                m.hex = Some("00".into());
                m.mask = Some("??".into());
            }),
            matcher(None, |m| {
                m.literal = Some("a".into());
                m.hex = Some("00".into());
                m.regex = Some("a".into());
                m.mask = Some("??".into());
            }),
        ] {
            let err = compile_matcher(&bad).unwrap_err();
            assert_eq!(err.code, MatcherErrorCode::Conflict);
            assert_eq!(err.code.as_str(), "matcher_conflict");
        }
    }

    #[test]
    fn invalid_regex_reports_code() {
        let err =
            compile_matcher(&matcher(None, |m| m.regex = Some("(unclosed".into()))).unwrap_err();
        assert_eq!(err.code, MatcherErrorCode::InvalidRegex);
        assert_eq!(err.code.as_str(), "invalid_regex");
    }

    #[test]
    fn hex_strict_edges() {
        for bad in ["", "   ", "abc", "zz", "1 2 3", "0x12"] {
            let err = compile_matcher(&matcher(None, |m| m.hex = Some(bad.into()))).unwrap_err();
            assert_eq!(err.code, MatcherErrorCode::InvalidHex, "case {bad:?}");
        }
        // 空白容忍 + 大小写
        let m = compiled(None, |m| m.hex = Some("\t0a 0B\n".into()));
        assert!(matches!(&m.kind, MatcherKind::Hex(b) if b == &vec![0x0a, 0x0b]));
    }

    #[test]
    fn mask_strict_edges() {
        for bad in ["", "abc", "5a ?x", "4?", "g0", "5a ?"] {
            let err = compile_matcher(&matcher(None, |m| m.mask = Some(bad.into()))).unwrap_err();
            assert_eq!(err.code, MatcherErrorCode::InvalidMask, "case {bad:?}");
        }
    }

    #[test]
    fn matches_literal_with_dir_filter() {
        let any = compiled(None, |m| m.literal = Some("OK".into()));
        assert!(any.matches(Dir::Rx, "rx: OK", None));
        assert!(any.matches(Dir::Tx, "tx: OK", None));
        assert!(!any.matches(Dir::Rx, "rx: ERR", None));

        let rx_only = compiled(Some(Dir::Rx), |m| m.literal = Some("OK".into()));
        assert!(rx_only.matches(Dir::Rx, "OK", None));
        assert!(!rx_only.matches(Dir::Tx, "OK", None), "dir 不等必须短路");
    }

    #[test]
    fn matches_regex() {
        let m = compiled(None, |m| m.regex = Some(r"^PONG ([0-9A-Fa-f]{2})$".into()));
        assert!(m.matches(Dir::Rx, "PONG 4f", None));
        assert!(!m.matches(Dir::Rx, "PONG zz", None));
    }

    #[test]
    fn matches_hex_prefers_raw_bytes_and_falls_back_to_text() {
        let m = compiled(None, |m| m.hex = Some("00 ff".into()));
        // 原始字节命中（bytes 缺失时该行 text 是 lossy 的，编码回退必不命中——这正是
        // 二进制行必须带原始 bytes 的原因）
        assert!(m.matches(Dir::Rx, "lossy \u{fffd}", Some(&[0x01, 0x00, 0xff])));
        assert!(!m.matches(Dir::Rx, "lossy \u{fffd}", Some(&[0x01, 0x02])));

        // bytes 缺失回退 text 的 UTF-8 编码（合法 UTF-8 行可无损恢复字节）
        let utf8 = compiled(None, |m| m.hex = Some("61 e4 b8 ad".into()));
        assert!(utf8.matches(Dir::Rx, "xa中y", None));
        assert!(!utf8.matches(Dir::Rx, "x中y", None));
    }

    #[test]
    fn matches_mask_wildcard() {
        let m = compiled(None, |m| m.mask = Some("5a ??".into()));
        assert!(m.matches(Dir::Rx, "text", Some(&[0x5a, 0x99])));
        assert!(!m.matches(Dir::Rx, "text", Some(&[0x5b, 0x99])));
        assert!(!m.matches(Dir::Rx, "text", Some(&[0x5a])));

        let m = compiled(None, |m| m.mask = Some("5a ?? 4f".into()));
        assert!(m.matches(Dir::Tx, "", Some(&[0x5a, 0x00, 0x4f])));
        assert!(!m.matches(Dir::Tx, "", Some(&[0x5a, 0x00, 0x50])));
    }

    #[test]
    fn error_display_contains_code_and_message() {
        let err = compile_matcher(&matcher(None, |_| {})).unwrap_err();
        let display = err.to_string();
        assert!(display.starts_with("matcher_conflict:"), "got {display}");
        assert!(display.contains("exactly one"), "got {display}");
    }

    #[test]
    fn matcher_error_code_strings_are_stable() {
        assert_eq!(MatcherErrorCode::Conflict.as_str(), "matcher_conflict");
        assert_eq!(MatcherErrorCode::InvalidRegex.as_str(), "invalid_regex");
        assert_eq!(MatcherErrorCode::InvalidHex.as_str(), "invalid_hex");
        assert_eq!(MatcherErrorCode::InvalidMask.as_str(), "invalid_mask");
    }

    // ---------- substitute ----------

    #[test]
    fn substitute_expands_references() {
        let mut vars = BTreeMap::new();
        vars.insert("cmd".to_string(), "PING".to_string());
        vars.insert("tail".to_string(), "!!".to_string());
        assert_eq!(
            substitute("${cmd} ${cmd}${tail}", &vars).unwrap(),
            "PING PING!!"
        );
        assert_eq!(substitute("no refs", &vars).unwrap(), "no refs");
        assert_eq!(substitute("", &vars).unwrap(), "");
    }

    #[test]
    fn substitute_undefined_variable_error() {
        let vars = BTreeMap::new();
        let e = substitute("a ${ghost} b", &vars).unwrap_err();
        assert_eq!(e.variable, "ghost");
        assert_eq!(e.to_string(), "undefined variable: ${ghost}");
    }

    #[test]
    fn substitute_dollar_edge_cases() {
        // 最小实现：无转义；$ 后非 { 原样保留；非法名/无闭合 } 按字面量保留
        let vars = BTreeMap::new();
        assert_eq!(substitute("$", &vars).unwrap(), "$");
        assert_eq!(substitute("$$", &vars).unwrap(), "$$");
        assert_eq!(substitute("$x", &vars).unwrap(), "$x");
        assert_eq!(substitute("${}", &vars).unwrap(), "${}");
        assert_eq!(substitute("${1x}", &vars).unwrap(), "${1x}");
        assert_eq!(substitute("${x", &vars).unwrap(), "${x");
        assert_eq!(substitute("价 ${值} 格", &vars).unwrap(), "价 ${值} 格");
        // 合法名但未定义 → 报错（区别于非法名）
        assert!(substitute("${x}", &vars).is_err());
    }

    #[test]
    fn find_refs_and_valid_names_agree() {
        // 引用扫描只认合法变量名；非法名序列不是引用
        let refs: Vec<&str> = find_refs("${a} ${_b9} ${9x} ${} ${ok")
            .iter()
            .map(|(_, n)| *n)
            .collect();
        assert_eq!(refs, vec!["a", "_b9"]);
        assert!(is_valid_var_name("_"));
        assert!(is_valid_var_name("Z9"));
        assert!(!is_valid_var_name(""));
        assert!(!is_valid_var_name("9x"));
        assert!(!is_valid_var_name("a-b"));
        assert!(!is_valid_var_name("ó"));
    }
}
