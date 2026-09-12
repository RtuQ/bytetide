//! 路由层公共部分：axum 共享状态 `BridgeCtx`、请求限额、404 文本助手，
//! 以及跨路由复用的**纯函数**——行过滤（build_filter/apply_filter 及其底层查找）
//! 与帧解码引擎（parse_frames，`/decode` `/value-hist` 共用）。
//! 暂放所用路由模块旁（Stage 2 Task 7 再归并共享 fixtures）；只依赖
//! `Arc<dyn BridgeService>`，不触 Tauri。

pub mod analysis;
pub mod annotations;
pub mod exchange;
pub mod lines;
pub mod metadata;

use std::borrow::Cow;
use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::server::BridgeConfig;
use super::service::BridgeService;
use bytetide_core::serial::manager::{BridgeLine, MatchHit, PlotConfig};
use bytetide_core::serial::port::Dir;

/// axum 共享状态：会话访问面（BridgeService）+ 配置（令牌/allowSend 按请求实时读）。
/// 不持有 AppHandle——router 可脱离 Tauri 构造（测试注入 fake service）。
#[derive(Clone)]
pub(crate) struct BridgeCtx {
    pub(crate) service: Arc<dyn BridgeService>,
    pub(crate) cfg: Arc<RwLock<BridgeConfig>>,
}

/// /lines 默认页大小与单页上限（follow 的 filterLimit 同上限）。
pub(crate) const DEFAULT_LIMIT: usize = 500;
pub(crate) const MAX_LIMIT: usize = 5000;

/// 统一的「会话不存在」404 文本响应（与抽取前逐字节一致，非 JSON 信封）。
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "session not found").into_response()
}

// =============================== 过滤 ===============================

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterFields {
    dir: Option<String>,
    /// 子串（逗号分隔多词，全部需命中）。
    q: Option<String>,
    ci: Option<bool>,
    re: Option<String>,
    /// 十六进制字节子串，如 `AA55`。
    hex: Option<String>,
    /// 十六进制 + `?` 通配，如 `AA55????CS`。
    mask: Option<String>,
    /// 负向：命中的行被排除。
    exclude: Option<String>,
    /// 时间窗（epoch 毫秒）。
    since_ms: Option<u64>,
    until_ms: Option<u64>,
}

pub(crate) struct FilterSpec {
    dir: Option<Dir>,
    qs: Vec<String>,
    ci: bool,
    re: Option<Regex>,
    hex: Vec<u8>,
    mask: Vec<Option<u8>>,
    exclude: Option<String>,
    since_ms: Option<u64>,
    until_ms: Option<u64>,
}

impl FilterSpec {
    /// 请求未携带任何过滤字段：/follow 据此走旧行为路径（不加 filterLimit）。
    pub(crate) fn is_noop(&self) -> bool {
        self.dir.is_none()
            && self.qs.is_empty()
            && self.re.is_none()
            && self.hex.is_empty()
            && self.mask.is_empty()
            && self.exclude.is_none()
            && self.since_ms.is_none()
            && self.until_ms.is_none()
    }
}

pub(crate) fn build_filter(f: &FilterFields) -> Result<FilterSpec, (StatusCode, String)> {
    let dir = match f.dir.as_deref() {
        Some("rx") => Some(Dir::Rx),
        Some("tx") => Some(Dir::Tx),
        Some(other) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("dir must be rx|tx, got {other}"),
            ))
        }
        _ => None,
    };
    let re = match f.re.as_deref() {
        Some(p) if !p.is_empty() => Some(
            Regex::new(p).map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid regex: {e}")))?,
        ),
        _ => None,
    };
    let qs =
        f.q.as_deref()
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();
    let hex = parse_hex(f.hex.as_deref().unwrap_or(""));
    let mask = parse_mask(f.mask.as_deref().unwrap_or(""));
    Ok(FilterSpec {
        dir,
        qs,
        ci: f.ci.unwrap_or(false),
        re,
        hex,
        mask,
        exclude: f.exclude.clone(),
        since_ms: f.since_ms,
        until_ms: f.until_ms,
    })
}

pub(crate) fn parse_hex(s: &str) -> Vec<u8> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..cleaned.len())
        .step_by(2)
        .filter_map(|i| {
            cleaned
                .get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect()
}

/// `AA55????` -> `vec![0xAA,0x55, Any,Any,Any,Any]`。
pub(crate) fn parse_mask(s: &str) -> Vec<Option<u8>> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = cleaned.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '?' && chars[i + 1] == '?' {
            out.push(None);
            i += 2;
        } else {
            let h = format!("{}{}", chars[i], chars[i + 1]);
            out.push(u8::from_str_radix(&h, 16).ok());
            i += 2;
        }
    }
    out
}

/// 一行的字节视图（binary 优先 `bytes`，否则 text 编码；供 hex/mask/decode 用）。
pub(crate) fn line_bytes(l: &BridgeLine) -> Cow<'_, [u8]> {
    match &l.bytes {
        Some(b) => Cow::Borrowed(b.as_slice()),
        None => Cow::Owned(l.text.as_bytes().to_vec()),
    }
}

/// 返回 `Some(Some(hit))`=通过且带命中；`Some(None)`=通过无命中；`None`=拒绝。
pub(crate) fn apply_filter(l: &BridgeLine, f: &FilterSpec) -> Option<Option<MatchHit>> {
    if let Some(d) = f.dir {
        if l.dir != d {
            return None;
        }
    }
    if let Some(lo) = f.since_ms {
        if l.epoch_millis < lo {
            return None;
        }
    }
    if let Some(hi) = f.until_ms {
        if l.epoch_millis > hi {
            return None;
        }
    }
    if let Some(ex) = &f.exclude {
        if str_find(&l.text, ex, f.ci).is_some() {
            return None;
        }
    }
    let mut hit: Option<MatchHit> = None;
    for q in &f.qs {
        let off = str_find(&l.text, q, f.ci)?;
        if hit.is_none() {
            hit = Some(MatchHit {
                offset: off as u64,
                length: q.len() as u64,
                field: "text".into(),
            });
        }
    }
    if let Some(re) = &f.re {
        match re_find(&l.text, re) {
            Some((off, len)) => {
                if hit.is_none() {
                    hit = Some(MatchHit {
                        offset: off as u64,
                        length: len as u64,
                        field: "text".into(),
                    });
                }
            }
            None => return None,
        }
    }
    if !f.hex.is_empty() {
        let b = line_bytes(l);
        let off = bytes_find(&b, &f.hex)?;
        if hit.is_none() {
            hit = Some(MatchHit {
                offset: off as u64,
                length: f.hex.len() as u64,
                field: "bytes".into(),
            });
        }
    }
    if !f.mask.is_empty() {
        let b = line_bytes(l);
        let off = mask_find(&b, &f.mask)?;
        if hit.is_none() {
            hit = Some(MatchHit {
                offset: off as u64,
                length: f.mask.len() as u64,
                field: "bytes".into(),
            });
        }
    }
    Some(hit)
}

fn str_find(hay: &str, needle: &str, ci: bool) -> Option<usize> {
    if ci {
        hay.to_lowercase().find(&needle.to_lowercase())
    } else {
        hay.find(needle)
    }
}

fn re_find(hay: &str, re: &Regex) -> Option<(usize, usize)> {
    re.find(hay).map(|m| (m.start(), m.len()))
}

fn bytes_find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn mask_find(hay: &[u8], mask: &[Option<u8>]) -> Option<usize> {
    if mask.is_empty() || hay.len() < mask.len() {
        return None;
    }
    'outer: for i in 0..=hay.len() - mask.len() {
        for (k, m) in mask.iter().enumerate() {
            if let Some(v) = m {
                if hay[i + k] != *v {
                    continue 'outer;
                }
            }
        }
        return Some(i);
    }
    None
}

/// 过滤快照，返回带（可选）命中信息的 owned 行（保留原序）；会话缺失返回 None。
pub(crate) fn filtered(ctx: &BridgeCtx, id: &str, f: &FilterSpec) -> Option<Vec<BridgeLine>> {
    let snap = ctx.service.snapshot(id).ok()?;
    Some(
        snap.iter()
            .filter_map(|l| {
                let hit = apply_filter(l, f)?;
                let mut bl = l.clone();
                bl.r#match = hit;
                Some(bl)
            })
            .collect(),
    )
}

// =============================== 帧解码引擎（/decode、/value-hist 共用） ===============================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeFrame {
    pub(crate) no: u64,
    pub(crate) idx: u32,
    pub(crate) values: Vec<f64>,
    pub(crate) raw_hex: String,
    pub(crate) ts: String,
    pub(crate) epoch_millis: u64,
    pub(crate) valid: bool,
    pub(crate) error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodePage {
    pub(crate) frames: Vec<DecodeFrame>,
    pub(crate) frame_count: u32,
    pub(crate) last_error: String,
    pub(crate) scanned: usize,
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{:02X}", b));
    }
    out
}

fn line_frame_bytes(l: &BridgeLine, source: &str) -> Vec<u8> {
    if source == "ascii-hex" {
        let mut out = vec![];
        let chars: Vec<char> = l.text.chars().collect();
        let mut i = 0;
        while i + 1 < chars.len() {
            let h = format!("{}{}", chars[i], chars[i + 1]);
            if let Ok(b) = u8::from_str_radix(&h, 16) {
                out.push(b);
            }
            i += 2;
        }
        return out;
    }
    line_bytes(l).into_owned()
}

enum Checksum {
    None,
    Sum,
    Xor,
}

impl Checksum {
    fn from_str(s: &str) -> Self {
        match s {
            "sum" => Self::Sum,
            "xor" => Self::Xor,
            _ => Self::None,
        }
    }
    fn compute(&self, data: &[u8]) -> u8 {
        match self {
            Self::None => 0,
            Self::Sum => {
                let mut s = 0u16;
                for &b in data {
                    s = s.wrapping_add(b as u16);
                }
                (s & 0xff) as u8
            }
            Self::Xor => {
                let mut x = 0u8;
                for &b in data {
                    x ^= b;
                }
                x
            }
        }
    }
}

pub(crate) fn parse_value(bytes: &[u8], off: usize, len: usize, endian: &str, signed: bool) -> f64 {
    if off + len > bytes.len() {
        return 0.0;
    }
    let mut v: u128 = 0;
    if endian == "little" {
        for k in 0..len {
            v = v * 256 + bytes[off + len - 1 - k] as u128;
        }
    } else {
        for k in 0..len {
            v = v * 256 + bytes[off + k] as u128;
        }
    }
    let mut val = v as f64;
    if signed {
        let max = 256f64.powi(len as i32);
        if v as f64 >= max / 2.0 {
            val = v as f64 - max;
        }
    }
    val
}

/// 移植 `usePlotParser.parseFrames`：帧布局 `[HEADER][DATA(channels×bytes)][CS?][TAIL?]`。
pub(crate) fn parse_frames(cfg: &PlotConfig, lines: &[BridgeLine], limit: usize) -> DecodePage {
    let channels = cfg.channels.max(1) as usize;
    let bpc = cfg.bytes_per_channel.max(1) as usize;
    let data_len = channels * bpc;
    let cs_len = if cfg.checksum == "none" { 0 } else { 1 };
    let head = parse_hex(&cfg.frame_head);
    let tail = parse_hex(&cfg.frame_tail);
    let tail_len = tail.len();
    let frame_len = data_len + cs_len + tail_len;
    let cs = Checksum::from_str(&cfg.checksum);

    if head.is_empty() && tail_len == 0 {
        return DecodePage {
            frames: vec![],
            frame_count: 0,
            last_error: "need frame head or tail".into(),
            scanned: 0,
        };
    }
    if frame_len == 0 {
        return DecodePage {
            frames: vec![],
            frame_count: 0,
            last_error: "invalid channels/bytes".into(),
            scanned: 0,
        };
    }

    // 拼字节流 + 每行区间（带 no/ts/epoch）
    let mut bytes: Vec<u8> = Vec::new();
    let mut ranges: Vec<(usize, usize, u64, String, u64)> = vec![]; // off0,off1,no,ts,epoch
    for l in lines {
        let b = line_frame_bytes(l, &cfg.source);
        if b.is_empty() {
            continue;
        }
        let off0 = bytes.len();
        bytes.extend_from_slice(&b);
        let off1 = bytes.len();
        ranges.push((off0, off1, l.no, l.ts.clone(), l.epoch_millis));
    }
    let scanned = lines.len();
    let total = bytes.len();
    if total < frame_len + head.len() {
        return DecodePage {
            frames: vec![],
            frame_count: 0,
            last_error: String::new(),
            scanned,
        };
    }

    let head_len = head.len();
    let mut frames: Vec<DecodeFrame> = vec![];
    let mut idx = 0u32;
    let mut last_error = String::new();

    if head_len > 0 {
        let need = head_len + frame_len;
        let mut i = 0;
        while i + need <= total {
            if frames.len() >= limit {
                break;
            }
            let mut ok = true;
            for k in 0..head_len {
                if bytes[i + k] != head[k] {
                    ok = false;
                    break;
                }
            }
            if !ok {
                i += 1;
                continue;
            }
            let data_start = i + head_len;
            let frame_end = data_start + frame_len;
            if tail_len > 0 {
                let mut t = true;
                for k in 0..tail_len {
                    if bytes[frame_end - tail_len + k] != tail[k] {
                        t = false;
                        break;
                    }
                }
                if !t {
                    i += 1;
                    continue;
                }
            }
            if cs_len > 0 {
                let expect = cs.compute(&bytes[data_start..data_start + data_len]);
                if bytes[data_start + data_len] != expect {
                    last_error = format!(
                        "checksum mismatch @{} (got {:02X}, exp {:02X})",
                        i,
                        bytes[data_start + data_len],
                        expect
                    );
                    i += 1;
                    continue;
                }
            }
            idx += 1;
            push_decode_frame(
                &mut frames,
                idx,
                &bytes,
                &ranges,
                head_len,
                channels,
                bpc,
                cfg,
                i,
                frame_end,
            );
            i = frame_end;
        }
    } else {
        // 无帧头：按帧尾反向定位
        let mut i = 0;
        while i + tail_len <= total {
            if frames.len() >= limit {
                break;
            }
            let mut t = true;
            for k in 0..tail_len {
                if bytes[i + k] != tail[k] {
                    t = false;
                    break;
                }
            }
            if !t {
                i += 1;
                continue;
            }
            let frame_end = i + tail_len;
            if frame_end < frame_len {
                i += 1;
                continue;
            }
            let frame_start = frame_end - frame_len;
            let data_start = frame_start;
            if cs_len > 0 {
                let expect = cs.compute(&bytes[data_start..data_start + data_len]);
                if bytes[data_start + data_len] != expect {
                    last_error = format!("checksum mismatch @{}", frame_start);
                    i += 1;
                    continue;
                }
            }
            idx += 1;
            push_decode_frame(
                &mut frames,
                idx,
                &bytes,
                &ranges,
                0,
                channels,
                bpc,
                cfg,
                frame_start,
                frame_end,
            );
            i = frame_end;
        }
    }

    DecodePage {
        frames,
        frame_count: idx,
        last_error,
        scanned,
    }
}

/// 二分找包含 `off` 的行区间 -> `(no, ts, epoch)`。
fn line_at_range(ranges: &[(usize, usize, u64, String, u64)], off: usize) -> (u64, String, u64) {
    let mut lo = 0usize;
    let mut hi = ranges.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (o0, o1, _, _, _) = &ranges[mid];
        if off < *o0 {
            hi = mid;
        } else if off >= *o1 {
            lo = mid + 1;
        } else {
            let (_, _, no, ts, ep) = &ranges[mid];
            return (*no, ts.clone(), *ep);
        }
    }
    (0, String::new(), 0)
}

#[allow(clippy::too_many_arguments)]
fn push_decode_frame(
    frames: &mut Vec<DecodeFrame>,
    idx: u32,
    bytes: &[u8],
    ranges: &[(usize, usize, u64, String, u64)],
    head_len: usize,
    channels: usize,
    bpc: usize,
    cfg: &PlotConfig,
    i: usize,
    frame_end: usize,
) {
    let data_start = i + head_len;
    let mut values = Vec::with_capacity(channels);
    for ch in 0..channels {
        values.push(parse_value(
            bytes,
            data_start + ch * bpc,
            bpc,
            &cfg.endian,
            cfg.signed,
        ));
    }
    let (no, ts, ep) = line_at_range(ranges, frame_end.saturating_sub(1));
    frames.push(DecodeFrame {
        no,
        idx,
        values,
        raw_hex: to_hex(&bytes[i..frame_end]),
        ts,
        epoch_millis: ep,
        valid: true,
        error: None,
    });
}

// =============================== 测试 ===============================

/// 测试行构造（routes 各子模块测试共用）。
#[cfg(test)]
pub(crate) fn mk_line(
    no: u64,
    dir: Dir,
    text: &str,
    bytes: Option<Vec<u8>>,
    epoch: u64,
) -> BridgeLine {
    BridgeLine {
        no,
        ts: "00:00:00.000".into(),
        dir,
        text: text.into(),
        bytes,
        epoch_millis: epoch,
        r#match: None,
    }
}

#[cfg(test)]
mod tests {
    //! 纯函数单测：过滤/查找/帧解码的黄金用例与边界。
    //! 仅测私有纯函数（同模块可见），不触网/不经 axum/不建 manager。
    use super::*;

    fn mk_plot() -> PlotConfig {
        PlotConfig::default()
    }

    // ---------------- parse_hex / parse_mask ----------------

    #[test]
    fn parse_hex_ignores_whitespace_and_odd_drop() {
        assert_eq!(parse_hex("AA 55"), vec![0xAA, 0x55]);
        assert_eq!(parse_hex("aa55"), vec![0xAA, 0x55]);
        assert_eq!(parse_hex("A"), Vec::<u8>::new()); // 奇数位 -> 不足一对，丢弃
        assert_eq!(parse_hex("A5G3"), vec![0xA5]); // G3 非法 -> 丢，A5 留
    }

    #[test]
    fn parse_mask_wildcards_and_literals() {
        assert_eq!(parse_mask("AA55"), vec![Some(0xAA), Some(0x55)]);
        assert_eq!(parse_mask("AA??55"), vec![Some(0xAA), None, Some(0x55)]);
        assert_eq!(parse_mask("AA 55"), vec![Some(0xAA), Some(0x55)]); // 空白忽略
        assert_eq!(parse_mask("A"), vec![]); // 奇数位 -> 空白忽略后仅 1 字符，无对
    }

    // ---------------- apply_filter ----------------

    fn spec(
        dir: Option<Dir>,
        qs: Vec<&str>,
        hex: Vec<u8>,
        mask: Vec<Option<u8>>,
        exclude: Option<&str>,
        since: Option<u64>,
        until: Option<u64>,
    ) -> FilterSpec {
        FilterSpec {
            dir,
            qs: qs.into_iter().map(String::from).collect(),
            ci: false,
            re: None,
            hex,
            mask,
            exclude: exclude.map(String::from),
            since_ms: since,
            until_ms: until,
        }
    }

    #[test]
    fn apply_filter_no_filter_passes_as_some_none() {
        let f = spec(None, vec![], vec![], vec![], None, None, None);
        let l = mk_line(1, Dir::Rx, "anything", None, 0);
        assert!(matches!(apply_filter(&l, &f), Some(None)));
    }

    #[test]
    fn apply_filter_dir_rejects_mismatch() {
        let f = spec(Some(Dir::Tx), vec![], vec![], vec![], None, None, None);
        assert!(apply_filter(&mk_line(1, Dir::Rx, "x", None, 0), &f).is_none());
        assert!(matches!(
            apply_filter(&mk_line(2, Dir::Tx, "x", None, 0), &f),
            Some(None)
        ));
    }

    #[test]
    fn apply_filter_q_multiple_all_must_match() {
        let f = spec(None, vec!["OK", "ERR"], vec![], vec![], None, None, None);
        assert!(matches!(
            apply_filter(&mk_line(1, Dir::Rx, "OK ERR foo", None, 0), &f),
            Some(Some(_))
        ));
        assert!(apply_filter(&mk_line(2, Dir::Rx, "OK foo", None, 0), &f).is_none());
    }

    #[test]
    fn apply_filter_hex_substring() {
        let f = spec(None, vec![], vec![0xAA, 0x55], vec![], None, None, None);
        assert!(matches!(
            apply_filter(
                &mk_line(1, Dir::Rx, "", Some(vec![0x00, 0xAA, 0x55, 0x99]), 0),
                &f
            ),
            Some(Some(_))
        ));
        assert!(apply_filter(&mk_line(2, Dir::Rx, "", Some(vec![0xAA, 0x99]), 0), &f).is_none());
    }

    #[test]
    fn apply_filter_mask_wildcard() {
        let f = spec(
            None,
            vec![],
            vec![],
            vec![Some(0xAA), None],
            None,
            None,
            None,
        ); // AA??
        assert!(matches!(
            apply_filter(&mk_line(1, Dir::Rx, "", Some(vec![0xAA, 0x12]), 0), &f),
            Some(Some(_))
        ));
        assert!(apply_filter(&mk_line(2, Dir::Rx, "", Some(vec![0x55, 0x12]), 0), &f).is_none());
    }

    #[test]
    fn apply_filter_exclude_negative() {
        let f = spec(
            None,
            vec!["DATA"],
            vec![],
            vec![],
            Some("NOISE"),
            None,
            None,
        );
        assert!(apply_filter(&mk_line(1, Dir::Rx, "DATA NOISE", None, 0), &f).is_none());
        assert!(matches!(
            apply_filter(&mk_line(2, Dir::Rx, "DATA here", None, 0), &f),
            Some(Some(_))
        ));
    }

    #[test]
    fn apply_filter_time_window() {
        let f = spec(None, vec![], vec![], vec![], None, Some(100), Some(200));
        assert!(apply_filter(&mk_line(1, Dir::Rx, "x", None, 50), &f).is_none()); // before
        assert!(matches!(
            apply_filter(&mk_line(2, Dir::Rx, "x", None, 150), &f),
            Some(None)
        )); // inside
        assert!(apply_filter(&mk_line(3, Dir::Rx, "x", None, 250), &f).is_none());
        // after
    }

    // ---------------- build_filter ----------------

    #[test]
    fn build_filter_ok_and_bad_dir() {
        let ff = FilterFields {
            dir: Some("rx".into()),
            q: Some("a,b".into()),
            ci: Some(true),
            re: None,
            hex: Some("AA55".into()),
            mask: None,
            exclude: None,
            since_ms: None,
            until_ms: None,
        };
        let spec = build_filter(&ff).expect("valid");
        assert_eq!(spec.dir, Some(Dir::Rx));
        assert_eq!(spec.qs, vec!["a".to_string(), "b".to_string()]);
        assert!(spec.ci);
        assert_eq!(spec.hex, vec![0xAA, 0x55]);

        let bad = FilterFields {
            dir: Some("xx".into()),
            ..Default::default()
        };
        assert!(build_filter(&bad).is_err());
    }

    // ---------------- line_bytes（黄金样本 plot-v1.json lineBytesSamples） ----------------

    /// 共享黄金样本顶层（serde 白名单取键，未知键如 `$about`/`version`/`declSample`
    /// 自动忽略）。向量与期望值以 fixture 为准，与 TS 侧
    /// `usePlotParser.test.ts` / `lineBytes.test.ts` 同源——任一侧实现漂移即红。
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PlotFixture {
        plot_cases: FxPlotCases,
        value_boundaries: FxValueBoundaries,
        checksum_vectors: FxChecksumVectors,
        line_bytes_samples: FxLineBytesSamples,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxPlotCases {
        groups: Vec<FxPlotGroup>,
    }

    #[derive(Deserialize)]
    struct FxPlotGroup {
        name: String,
        /// 直接反序列化进 `PlotConfig`（camelCase 对齐，键全集在 fixture 中）。
        config: serde_json::Value,
        cases: Vec<FxPlotCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxPlotCase {
        name: String,
        lines: Vec<FxPlotLine>,
        expected: FxPlotExpected,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxPlotLine {
        dir: String,
        text: String,
        bytes: Option<Vec<u8>>,
        epoch_millis: u64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxPlotExpected {
        frame_count: u32,
        points: Vec<FxPlotPoint>,
        last_error: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxPlotPoint {
        values: Vec<f64>,
        raw_hex: String,
        epoch_millis: u64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxValueBoundaries {
        cases: Vec<FxValueCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxValueCase {
        bytes: Vec<u8>,
        offset: usize,
        len: usize,
        endian: String,
        signed: bool,
        expected: f64,
    }

    #[derive(Deserialize)]
    struct FxChecksumVectors {
        sum: FxCkVector,
        xor: FxCkVector,
        none: FxCkVector,
    }

    #[derive(Deserialize)]
    struct FxCkVector {
        bytes: Vec<u8>,
        expected: u8,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxLineBytesSamples {
        cases: Vec<FxLineBytesCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxLineBytesCase {
        name: String,
        source: String,
        line: FxLineBytesLine,
        expected_bytes: Vec<u8>,
    }

    #[derive(Deserialize)]
    struct FxLineBytesLine {
        text: String,
        bytes: Option<Vec<u8>>,
    }

    fn plot_fixture() -> PlotFixture {
        // 相对本文件 4 级上跳到仓库根（routes → bridge → src → src-tauri → 根）
        serde_json::from_str(include_str!("../../../../testdata/protocol/plot-v1.json"))
            .expect("plot-v1.json fixture parses")
    }

    #[test]
    fn line_bytes_matches_fixture_binary_samples() {
        let fx = plot_fixture();
        for (i, c) in fx.line_bytes_samples.cases.iter().enumerate() {
            if c.source != "binary" {
                // ascii-hex 抽取向量留在 TS 侧：Rust `line_frame_bytes` 按下标配对、
                // 宽松度与 TS 正则扫描不同；两侧一致的部分经 plotCases ascii-hex
                // 组（紧凑 hex 文本）覆盖
                continue;
            }
            // fixture 语义：空 bytes 数组视同缺失；Rust `None` 即缺失
            let bytes = c.line.bytes.clone().filter(|b| !b.is_empty());
            let l = mk_line(1, Dir::Rx, &c.line.text, bytes, 0);
            assert_eq!(
                &*line_bytes(&l),
                &c.expected_bytes[..],
                "lineBytesSamples[{i}] {}",
                c.name
            );
        }
    }

    // ---------------- parse_frames：共享黄金样本（plot-v1.json plotCases） ----------------

    #[test]
    fn parse_frames_matches_plot_fixture_golden_cases() {
        let fx = plot_fixture();
        for group in &fx.plot_cases.groups {
            let cfg: PlotConfig = serde_json::from_value(group.config.clone())
                .unwrap_or_else(|e| panic!("{} config: {e}", group.name));
            for (ci, case) in group.cases.iter().enumerate() {
                // points < frameCount 的用例编码了 TS 侧 maxPoints 窗口裁剪（前端
                // 消费者行为）；parse_frames 逐帧产出，页 limit 截断另测
                // （parse_frames_limit_truncates）——该类用例 Rust 不适用
                if case.expected.points.len() as u32 != case.expected.frame_count {
                    continue;
                }
                let lines: Vec<BridgeLine> = case
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(li, l)| {
                        let dir = if l.dir == "tx" { Dir::Tx } else { Dir::Rx };
                        mk_line(
                            (li + 1) as u64,
                            dir,
                            &l.text,
                            l.bytes.clone(),
                            l.epoch_millis,
                        )
                    })
                    .collect();
                let page = parse_frames(&cfg, &lines, 500);
                let tag = format!("plotCases {}[{}] {}", group.name, ci, case.name);
                assert_eq!(page.frame_count, case.expected.frame_count, "{tag}");
                assert_eq!(page.frames.len(), case.expected.points.len(), "{tag}");
                for (pi, pt) in case.expected.points.iter().enumerate() {
                    assert_eq!(page.frames[pi].values, pt.values, "{tag} point[{pi}]");
                    assert_eq!(page.frames[pi].raw_hex, pt.raw_hex, "{tag} point[{pi}]");
                    assert_eq!(
                        page.frames[pi].epoch_millis, pt.epoch_millis,
                        "{tag} point[{pi}]"
                    );
                }
                if case.expected.frame_count > 0 {
                    assert!(page.last_error.is_empty(), "{tag}: {:?}", page.last_error);
                } else if !case.expected.last_error.is_empty() {
                    assert_eq!(page.last_error, case.expected.last_error, "{tag}");
                } else {
                    // 0 帧且 fixture lastError 为空（TS 静默跳帧语义）；Rust 记
                    // checksum mismatch 但同样不产出帧（plotCases.$about 的跨语言差异项）
                    assert!(
                        page.last_error.is_empty() || page.last_error.contains("checksum mismatch"),
                        "{tag}: {:?}",
                        page.last_error
                    );
                }
            }
        }
    }

    #[test]
    fn parse_frames_limit_truncates() {
        let mut cfg = mk_plot();
        cfg.frame_head = "AA".into();
        cfg.channels = 1;
        cfg.bytes_per_channel = 1;
        cfg.endian = "big".into();
        // AA 01 AA 02 AA 03 -> 3 帧（head=AA,data=1B）
        let mk = || {
            mk_line(
                1,
                Dir::Rx,
                "",
                Some(vec![0xAA, 0x01, 0xAA, 0x02, 0xAA, 0x03]),
                0,
            )
        };
        // 不限 -> 全 3 帧
        let full = parse_frames(&cfg, &[mk()], 500);
        assert_eq!(full.frame_count, 3);
        assert_eq!(full.frames[2].raw_hex, "AA 03");
        // limit=2 -> 截断到 2（break 在循环体首，idx 也停在 2）
        let page = parse_frames(&cfg, &[mk()], 2);
        assert_eq!(page.frame_count, 2);
        assert_eq!(page.frames.len(), 2);
        assert_eq!(page.frames[1].raw_hex, "AA 02");
    }

    #[test]
    fn parse_frames_needs_head_or_tail() {
        let cfg = mk_plot(); // head="" tail=""
        let line = mk_line(1, Dir::Rx, "", Some(vec![0xAA]), 0);
        let page = parse_frames(&cfg, &[line], 500);
        assert_eq!(page.frame_count, 0);
        assert_eq!(page.last_error, "need frame head or tail");
    }

    // ---------------- parse_value / 校验和（黄金样本向量） ----------------

    #[test]
    fn parse_value_matches_fixture_value_boundaries() {
        let fx = plot_fixture();
        for (i, c) in fx.value_boundaries.cases.iter().enumerate() {
            assert_eq!(
                parse_value(&c.bytes, c.offset, c.len, &c.endian, c.signed),
                c.expected,
                "valueBoundaries[{i}] (endian={}, signed={}, len={})",
                c.endian,
                c.signed,
                c.len
            );
        }
    }

    #[test]
    fn checksum_vectors_match_fixture() {
        let fx = plot_fixture();
        let v = &fx.checksum_vectors;
        assert_eq!(
            Checksum::from_str("sum").compute(&v.sum.bytes),
            v.sum.expected,
            "sum vector"
        );
        assert_eq!(
            Checksum::from_str("xor").compute(&v.xor.bytes),
            v.xor.expected,
            "xor vector"
        );
        assert_eq!(
            Checksum::from_str("none").compute(&v.none.bytes),
            v.none.expected,
            "none vector"
        );
    }
}
