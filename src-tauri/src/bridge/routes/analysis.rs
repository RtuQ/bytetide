//! 分析路由：`/sessions/:id/plot-config`（GET/POST）、`/timing`、`/decode`、
//! `/value-hist`、`/infer`。帧解码引擎与过滤纯函数在 `super`（routes/mod.rs）。

use std::collections::BTreeMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use super::{
    build_filter, filtered, not_found, parse_frames, BridgeCtx, DecodePage, DEFAULT_LIMIT,
    MAX_LIMIT,
};
use bytetide_core::serial::manager::{BridgeLine, PlotConfig};
use bytetide_core::serial::port::Dir;

// =============================== /plot-config ===============================

pub(crate) async fn plot_config(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.service.plot(&id) {
        Ok(c) => Json(c).into_response(),
        Err(_) => not_found(),
    }
}

/// 写回绘图文法（整包替换——先 GET 当前值再改字段）。后端留存供 `/decode` 复用，
/// 同时发 `bridge-plot-updated` 事件让应用界面即时采纳。不受 allowSend 门控（不触串口）。
pub(crate) async fn plot_config_set(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(mut cfg): Json<PlotConfig>,
) -> Response {
    if let Err(e) = sanitize_plot_config(&mut cfg) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    if ctx.service.set_plot(&id, cfg.clone()).is_err() {
        return not_found();
    }
    ctx.service.notify_plot_updated(&id, &cfg);
    Json(cfg).into_response()
}

/// 校验并归一化 REST 写回的绘图文法：枚举字段限定取值、head/tail 须为合法 hex、
/// channels 夹取 1..=16、bytesPerChannel 限 1/2/4、maxPoints 夹取 1..=100_000。
fn sanitize_plot_config(cfg: &mut PlotConfig) -> Result<(), String> {
    let s = cfg.source.trim().to_ascii_lowercase();
    if s != "binary" && s != "ascii-hex" {
        return Err(format!(
            "source must be binary|ascii-hex, got {:?}",
            cfg.source
        ));
    }
    cfg.source = s;
    let c = cfg.checksum.trim().to_ascii_lowercase();
    if c != "none" && c != "sum" && c != "xor" {
        return Err(format!(
            "checksum must be none|sum|xor, got {:?}",
            cfg.checksum
        ));
    }
    cfg.checksum = c;
    let e = cfg.endian.trim().to_ascii_lowercase();
    if e != "big" && e != "little" {
        return Err(format!("endian must be big|little, got {:?}", cfg.endian));
    }
    cfg.endian = e;
    sanitize_hex_field(&mut cfg.frame_head, "head")?;
    sanitize_hex_field(&mut cfg.frame_tail, "tail")?;
    if !matches!(cfg.bytes_per_channel, 1 | 2 | 4) {
        return Err(format!(
            "bytesPerChannel must be 1|2|4, got {}",
            cfg.bytes_per_channel
        ));
    }
    cfg.channels = cfg.channels.clamp(1, 16);
    cfg.max_points = cfg.max_points.clamp(1, 100_000);
    Ok(())
}

/// hex 字段归一：去空白、转大写；空串合法；否则须为偶数长度纯 hex。
fn sanitize_hex_field(field: &mut String, name: &str) -> Result<(), String> {
    let cleaned: String = field.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        field.clear();
        return Ok(());
    }
    if !cleaned.len().is_multiple_of(2) || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{name} must be hex pairs, got {:?}", field));
    }
    *field = cleaned.to_ascii_uppercase();
    Ok(())
}

// =============================== /timing ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimingParams {
    #[serde(flatten)]
    filter: super::FilterFields,
    gap_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Gap {
    from_no: u64,
    to_no: u64,
    duration_ms: u64,
    from_ts: String,
    to_ts: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimingPage {
    count: usize,
    min_gap: u64,
    max_gap: u64,
    avg_gap: u64,
    p95_gap: u64,
    gaps: Vec<Gap>,
}

pub(crate) async fn timing(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<TimingParams>,
) -> Response {
    let mut f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    if f.dir.is_none() {
        f.dir = Some(Dir::Rx);
    }
    let filt = match filtered(&ctx, &id, &f) {
        Some(v) => v,
        None => return not_found(),
    };
    // 相邻到达间隔（毫秒）
    let mut diffs: Vec<(u64, &BridgeLine, &BridgeLine)> = vec![];
    for w in filt.windows(2) {
        let a = &w[0];
        let b = &w[1];
        if b.epoch_millis >= a.epoch_millis {
            diffs.push((b.epoch_millis - a.epoch_millis, a, b));
        }
    }
    let mut only_ms: Vec<u64> = diffs.iter().map(|(d, _, _)| *d).collect();
    only_ms.sort_unstable();
    let p95 = percentile(&only_ms, 95);
    let gap = p.gap_ms.unwrap_or(p95);
    let gaps: Vec<Gap> = diffs
        .iter()
        .filter(|(d, _, _)| *d > gap)
        .map(|(d, a, b)| Gap {
            from_no: a.no,
            to_no: b.no,
            duration_ms: *d,
            from_ts: a.ts.clone(),
            to_ts: b.ts.clone(),
        })
        .collect();
    let count = only_ms.len();
    let min_gap = only_ms.iter().copied().min().unwrap_or(0);
    let max_gap = only_ms.iter().copied().max().unwrap_or(0);
    let avg_gap = if count > 0 {
        only_ms.iter().sum::<u64>() / count as u64
    } else {
        0
    };
    Json(TimingPage {
        count,
        min_gap,
        max_gap,
        avg_gap,
        p95_gap: p95,
        gaps,
    })
    .into_response()
}

fn percentile(sorted: &[u64], pct: u8) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64) * (pct as f64) / 100.0).ceil() as usize;
    sorted[idx.saturating_sub(1).min(sorted.len() - 1)]
}

// =============================== /decode ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeParams {
    #[serde(flatten)]
    filter: super::FilterFields,
    head: Option<String>,
    tail: Option<String>,
    checksum: Option<String>,
    channels: Option<u32>,
    bytes: Option<u32>,
    endian: Option<String>,
    signed: Option<bool>,
    source: Option<String>,
    from: Option<u64>,
    to: Option<u64>,
    limit: Option<usize>,
}

pub(crate) async fn decode(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<DecodeParams>,
) -> Response {
    // 基础文法取已存 PlotConfig，参数覆盖
    let mut base = ctx.service.plot(&id).unwrap_or_default();
    if let Some(ref h) = p.head {
        base.frame_head = h.clone();
    }
    if let Some(ref t) = p.tail {
        base.frame_tail = t.clone();
    }
    if let Some(ref c) = p.checksum {
        base.checksum = c.clone();
    }
    if let Some(c) = p.channels {
        base.channels = c;
    }
    if let Some(b) = p.bytes {
        base.bytes_per_channel = b;
    }
    if let Some(ref e) = p.endian {
        base.endian = e.clone();
    }
    if let Some(s) = p.signed {
        base.signed = s;
    }
    if let Some(ref s) = p.source {
        base.source = s.clone();
    }

    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let mut f = f;
    if f.dir.is_none() {
        f.dir = Some(Dir::Rx);
    }
    let filt = match filtered(&ctx, &id, &f) {
        Some(v) => v,
        None => return not_found(),
    };
    // from/to 在 no 上裁剪
    let filt: Vec<BridgeLine> = filt
        .into_iter()
        .filter(|l| p.from.is_none_or(|x| l.no >= x) && p.to.is_none_or(|x| l.no <= x))
        .collect();

    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let page = parse_frames(&base, &filt, limit);
    Json(page).into_response()
}

// =============================== /value-hist ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValueHistParams {
    #[serde(flatten)]
    filter: super::FilterFields,
    head: Option<String>,
    tail: Option<String>,
    checksum: Option<String>,
    channels: Option<u32>,
    bytes: Option<u32>,
    endian: Option<String>,
    signed: Option<bool>,
    source: Option<String>,
    channel: Option<usize>,
    from: Option<u64>,
    to: Option<u64>,
    top_n: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValueDist {
    value: f64,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValueHistPage {
    channel: usize,
    samples: usize,
    distinct: usize,
    min: f64,
    max: f64,
    mean: f64,
    distribution: Vec<ValueDist>,
}

/// 值直方图统计结果（`value_stats` 输出，便于纯函数单测）。
struct ValueStats {
    samples: usize,
    distinct: usize,
    min: f64,
    max: f64,
    mean: f64,
    distribution: Vec<ValueDist>,
}

/// 值直方图聚合：样本/去重数（值量化到 4 位小数后去重）、min/max/mean、按频次降序的 top-N 分布。
fn value_stats(vals: &[f64], top_n: usize) -> ValueStats {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    let mut counts: BTreeMap<u64, u64> = BTreeMap::new();
    for v in vals {
        min = min.min(*v);
        max = max.max(*v);
        sum += *v;
        // 以量化后的整数键聚合（保留 4 位小数）
        let key = (v * 10000.0).round() as i64 as u64;
        *counts.entry(key).or_insert(0) += 1;
    }
    let samples = vals.len();
    let mut dist: Vec<(u64, u64)> = counts.iter().map(|(k, c)| (*k, *c)).collect();
    dist.sort_by_key(|b| std::cmp::Reverse(b.1));
    ValueStats {
        samples,
        distinct: counts.len(),
        min: if samples > 0 { min } else { 0.0 },
        max: if samples > 0 { max } else { 0.0 },
        mean: if samples > 0 {
            sum / samples as f64
        } else {
            0.0
        },
        distribution: dist
            .into_iter()
            .take(top_n)
            .map(|(k, c)| ValueDist {
                value: k as f64 / 10000.0,
                count: c,
            })
            .collect(),
    }
}

pub(crate) async fn value_hist(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<ValueHistParams>,
) -> Response {
    let mut base = ctx.service.plot(&id).unwrap_or_default();
    if let Some(ref h) = p.head {
        base.frame_head = h.clone();
    }
    if let Some(ref t) = p.tail {
        base.frame_tail = t.clone();
    }
    if let Some(ref c) = p.checksum {
        base.checksum = c.clone();
    }
    if let Some(c) = p.channels {
        base.channels = c;
    }
    if let Some(b) = p.bytes {
        base.bytes_per_channel = b;
    }
    if let Some(ref e) = p.endian {
        base.endian = e.clone();
    }
    if let Some(s) = p.signed {
        base.signed = s;
    }
    if let Some(ref s) = p.source {
        base.source = s.clone();
    }

    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let mut f = f;
    if f.dir.is_none() {
        f.dir = Some(Dir::Rx);
    }
    let filt = match filtered(&ctx, &id, &f) {
        Some(v) => v,
        None => return not_found(),
    };
    let filt: Vec<BridgeLine> = filt
        .into_iter()
        .filter(|l| p.from.is_none_or(|x| l.no >= x) && p.to.is_none_or(|x| l.no <= x))
        .collect();

    let page: DecodePage = parse_frames(&base, &filt, MAX_LIMIT);
    let channel = p.channel.unwrap_or(0);
    let mut vals: Vec<f64> = vec![];
    for fr in &page.frames {
        if let Some(v) = fr.values.get(channel) {
            vals.push(*v);
        }
    }
    let s = value_stats(&vals, p.top_n.unwrap_or(20));
    Json(ValueHistPage {
        channel,
        samples: s.samples,
        distinct: s.distinct,
        min: s.min,
        max: s.max,
        mean: s.mean,
        distribution: s.distribution,
    })
    .into_response()
}

// =============================== /infer ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InferParams {
    #[serde(flatten)]
    filter: super::FilterFields,
    min_repeat: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HexCount {
    hex: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChecksumCount {
    kind: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InferPage {
    heads: Vec<HexCount>,
    tails: Vec<HexCount>,
    checksums: Vec<ChecksumCount>,
    suggested_frame_len: Option<usize>,
}

pub(crate) async fn infer(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<InferParams>,
) -> Response {
    let mut f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    if f.dir.is_none() {
        f.dir = Some(Dir::Rx);
    }
    let min_rep = p.min_repeat.unwrap_or(2);
    let filt = match filtered(&ctx, &id, &f) {
        Some(v) => v,
        None => return not_found(),
    };

    // 1B/2B 前缀频次
    let mut heads1: BTreeMap<u8, u64> = BTreeMap::new();
    let mut heads2: BTreeMap<u16, u64> = BTreeMap::new();
    let mut tails1: BTreeMap<u8, u64> = BTreeMap::new();
    let mut lens: BTreeMap<usize, u64> = BTreeMap::new();
    let mut sum_ok = 0u64;
    let mut xor_ok = 0u64;
    let mut total = 0u64;

    for l in &filt {
        let b = super::line_bytes(l);
        if b.is_empty() {
            continue;
        }
        total += 1;
        *heads1.entry(b[0]).or_insert(0) += 1;
        if b.len() >= 2 {
            *heads2
                .entry(((b[0] as u16) << 8) | b[1] as u16)
                .or_insert(0) += 1;
        }
        *tails1.entry(b[b.len() - 1]).or_insert(0) += 1;
        *lens.entry(b.len()).or_insert(0) += 1;
        // 校验和试探：末字节 vs 数据段（除末字节）的 sum/xor
        if b.len() >= 2 {
            let data = &b[..b.len() - 1];
            let last = b[b.len() - 1];
            let s = data.iter().fold(0u16, |a, x| a.wrapping_add(*x as u16)) & 0xff;
            if s as u8 == last {
                sum_ok += 1;
            }
            let x = data.iter().fold(0u8, |a, x| a ^ *x);
            if x == last {
                xor_ok += 1;
            }
        }
    }

    let mut heads: Vec<HexCount> = vec![];
    for (h, c) in &heads2 {
        if *c >= min_rep {
            heads.push(HexCount {
                hex: format!("{:02X}{:02X}", h >> 8, h & 0xff),
                count: *c,
            });
        }
    }
    if heads.is_empty() {
        for (h, c) in &heads1 {
            if *c >= min_rep {
                heads.push(HexCount {
                    hex: format!("{:02X}", h),
                    count: *c,
                });
            }
        }
    }
    heads.sort_by_key(|b| std::cmp::Reverse(b.count));
    heads.truncate(8);

    let mut tails: Vec<HexCount> = tails1
        .iter()
        .filter(|(_, c)| **c >= min_rep)
        .map(|(t, c)| HexCount {
            hex: format!("{:02X}", t),
            count: *c,
        })
        .collect();
    tails.sort_by_key(|b| std::cmp::Reverse(b.count));
    tails.truncate(8);

    let mut checksums = vec![];
    if sum_ok > 0 {
        checksums.push(ChecksumCount {
            kind: "sum".into(),
            count: sum_ok,
        });
    }
    if xor_ok > 0 {
        checksums.push(ChecksumCount {
            kind: "xor".into(),
            count: xor_ok,
        });
    }

    let suggested_frame_len = lens.iter().max_by_key(|(_, c)| **c).and_then(|(len, c)| {
        if *c >= min_rep && total > 0 && (*c as f64 / total as f64) >= 0.5 {
            Some(*len)
        } else {
            None
        }
    });

    Json(InferPage {
        heads,
        tails,
        checksums,
        suggested_frame_len,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    //! 绘图文法校验与值直方图聚合的纯函数单测。
    use super::*;

    // ---------------- sanitize_plot_config ----------------

    #[test]
    fn sanitize_plot_config_normalizes_and_clamps() {
        let mut cfg = PlotConfig {
            source: "ASCII-HEX".into(),
            checksum: " Xor ".into(),
            endian: "Little".into(),
            frame_head: "aa 55".into(),
            channels: 99,
            max_points: 0,
            ..Default::default()
        };
        sanitize_plot_config(&mut cfg).expect("valid");
        assert_eq!(cfg.source, "ascii-hex");
        assert_eq!(cfg.checksum, "xor");
        assert_eq!(cfg.endian, "little");
        assert_eq!(cfg.frame_head, "AA55"); // 去空白 + 大写
        assert_eq!(cfg.channels, 16);
        assert_eq!(cfg.max_points, 1);
    }

    #[test]
    fn sanitize_plot_config_rejects_bad_enum_hex_and_bytes() {
        let mut bad = PlotConfig {
            source: "raw".into(),
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig {
            checksum: "crc8".into(),
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig {
            endian: "middle".into(),
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig {
            frame_tail: "AA5".into(), // 奇数长度
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig {
            frame_head: "ZZ".into(), // 非 hex
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig {
            bytes_per_channel: 3,
            ..Default::default()
        };
        assert!(sanitize_plot_config(&mut bad).is_err());

        // 空 head/tail 合法（无帧头模式）
        let mut ok = PlotConfig::default();
        ok.frame_head.clear();
        ok.frame_tail.clear();
        assert!(sanitize_plot_config(&mut ok).is_ok());
    }

    // ---------------- value_stats ----------------

    #[test]
    fn value_stats_counts_distinct_not_samples() {
        // 重复值占多数：distinct 应为 3 而非样本数 5
        let s = value_stats(&[14381.0, 14381.0, 14381.0, 23840.0, 11321.0], 20);
        assert_eq!(s.samples, 5);
        assert_eq!(s.distinct, 3);
        assert_eq!(s.min, 11321.0);
        assert_eq!(s.max, 23840.0);
        assert_eq!(s.mean, (14381.0 * 3.0 + 23840.0 + 11321.0) / 5.0);
        // 按频次降序：众数在前
        assert_eq!(s.distribution[0].value, 14381.0);
        assert_eq!(s.distribution[0].count, 3);
    }

    #[test]
    fn value_stats_top_n_truncates_and_empty_edge() {
        let vals = [1.0, 2.0, 3.0];
        let s = value_stats(&vals, 2);
        assert_eq!(s.distinct, 3);
        assert_eq!(s.distribution.len(), 2); // topN 截断，distinct 不受影响

        let e = value_stats(&[], 20);
        assert_eq!(e.samples, 0);
        assert_eq!(e.distinct, 0);
        assert_eq!(e.min, 0.0);
        assert_eq!(e.max, 0.0);
        assert_eq!(e.mean, 0.0);
        assert!(e.distribution.is_empty());
    }

    #[test]
    fn value_stats_quantizes_to_four_decimals() {
        // 浮点噪声 1.00000001 与 1.0 应归并为同一个去重值
        let s = value_stats(&[1.0, 1.00000001], 20);
        assert_eq!(s.distinct, 1);
        assert_eq!(s.distribution[0].value, 1.0);
        assert_eq!(s.distribution[0].count, 2);
    }
}
