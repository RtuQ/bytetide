//! 行数据路由：`/lines` `/follow` `/histogram` `/bookmarks` `/alerts` `/export`。

use std::collections::BTreeMap;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use super::{
    apply_filter, build_filter, filtered, not_found, BridgeCtx, FilterFields, FilterSpec,
    DEFAULT_LIMIT, MAX_LIMIT,
};
use bytetide_core::serial::manager::BridgeLine;
use bytetide_core::serial::port::Dir;

// =============================== /lines ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinesParams {
    #[serde(flatten)]
    filter: FilterFields,
    no: Option<u64>,
    from: Option<u64>,
    to: Option<u64>,
    last: Option<usize>,
    since_no: Option<u64>,
    around: Option<u64>,
    span: Option<u64>,
    limit: Option<usize>,
    offset: Option<usize>,
    format: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinesPage {
    lines: Vec<BridgeLine>,
    total: usize,
    first_no: u64,
    last_no: u64,
    size: usize,
    truncated: bool,
}

pub(crate) async fn lines(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<LinesParams>,
) -> Response {
    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let snap = match ctx.service.snapshot(&id) {
        Ok(s) => s,
        Err(_) => return not_found(),
    };
    // 过滤 + 命中
    let filt: Vec<BridgeLine> = snap
        .iter()
        .filter_map(|l| {
            let hit = apply_filter(l, &f)?;
            let mut bl = l.clone();
            bl.r#match = hit;
            Some(bl)
        })
        .collect();
    let first_no = snap.first().map(|l| l.no).unwrap_or(0);
    let last_no = snap.last().map(|l| l.no).unwrap_or(0);
    let size = snap.len();

    // 选择（优先级）
    let selected: Vec<&BridgeLine> = select_lines(&filt, &p);

    // 分页
    let offset = p.offset.unwrap_or(0);
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let total = selected.len();
    let truncated = total.saturating_sub(offset) > limit;
    let page: Vec<BridgeLine> = selected
        .into_iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect();

    let fmt = p.format.as_deref();
    if fmt == Some("csv") || fmt == Some("tsv") {
        let sep = if fmt == Some("csv") { ',' } else { '\t' };
        let mut s = format!("no{sep}ts{sep}dir{sep}text{sep}bytes{sep}epochMillis\n");
        for l in &page {
            let bytes = l
                .bytes
                .as_ref()
                .map(|b| {
                    b.iter()
                        .map(|x| format!("{:02X}", x))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            s.push_str(&format!(
                "{}{sep}{}{sep}{}{sep}{}{sep}{}{sep}{}\n",
                l.no,
                csv_escape(&l.ts, sep),
                dir_str(l.dir),
                csv_escape(&l.text, sep),
                bytes,
                l.epoch_millis
            ));
        }
        let mt = if sep == ',' {
            "text/csv; charset=utf-8"
        } else {
            "text/tab-separated-values; charset=utf-8"
        };
        let mut resp = s.into_response();
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(mt)
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("text/plain")),
        );
        return resp;
    }
    Json(LinesPage {
        lines: page,
        total,
        first_no,
        last_no,
        size,
        truncated,
    })
    .into_response()
}

fn dir_str(d: Dir) -> &'static str {
    match d {
        Dir::Rx => "rx",
        Dir::Tx => "tx",
    }
}

/// RFC 4180 风格转义：含分隔符/引号/换行则加引号并把内部引号翻倍。
fn csv_escape(s: &str, sep: char) -> String {
    if s.contains(sep) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn select_lines<'a>(filt: &'a [BridgeLine], p: &LinesParams) -> Vec<&'a BridgeLine> {
    if let Some(no) = p.no {
        return filt.iter().filter(|l| l.no == no).take(1).collect();
    }
    if let (Some(from), Some(to)) = (p.from, p.to) {
        return filt.iter().filter(|l| l.no >= from && l.no <= to).collect();
    }
    if let Some(last) = p.last {
        let start = filt.len().saturating_sub(last);
        return filt[start..].iter().collect();
    }
    if let Some(since) = p.since_no {
        return filt.iter().filter(|l| l.no > since).collect();
    }
    if let Some(around) = p.around {
        let span = p.span.unwrap_or(10) as usize;
        // 精确命中
        if let Some(i) = filt.iter().position(|l| l.no == around) {
            let start = i.saturating_sub(span);
            let end = (i + span + 1).min(filt.len());
            return filt[start..end].iter().collect();
        }
        // 最近邻
        let idx = filt
            .iter()
            .position(|l| l.no > around)
            .unwrap_or(filt.len());
        let start = idx.saturating_sub(span);
        let end = (idx + span).min(filt.len());
        return filt[start..end].iter().collect();
    }
    // from + limit（无 to）：no >= from，靠分页 limit 截断
    if let Some(from) = p.from {
        return filt.iter().filter(|l| l.no >= from).collect();
    }
    filt.iter().collect()
}

// =============================== /follow ===============================

/// follow 过滤批次：施加 FilterSpec 与 filterLimit，返回 (返回行, truncated, lastNo)。
/// - 未截断：lastNo = high（扫描高水位，**含未匹配行**——客户端 sinceNo 据此前进过
///   不匹配行，否则游标永不前进会无限重扫同一段）
/// - 截断：lastNo = 最后一条**返回行**的 no，未消费区间留给下一轮
fn filter_follow_batch(
    lines: Vec<BridgeLine>,
    f: &FilterSpec,
    limit: usize,
    high: u64,
) -> (Vec<BridgeLine>, bool, u64) {
    let mut matched = Vec::new();
    for l in lines {
        if let Some(hit) = apply_filter(&l, f) {
            let mut bl = l;
            bl.r#match = hit;
            matched.push(bl);
        }
    }
    if matched.len() <= limit {
        return (matched, false, high);
    }
    let last_no = matched[limit - 1].no;
    matched.truncate(limit);
    (matched, true, last_no)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FollowParams {
    since_no: Option<u64>,
    timeout_ms: Option<u64>,
    /// 服务端过滤：与 /lines 同一套 FilterFields（re/q/dir/exclude/hex/mask/ci/时间窗）。
    /// 无任何过滤字段时走旧行为（响应逐字节不变，不施加 filterLimit）。
    #[serde(flatten)]
    filter: FilterFields,
    /// 单次响应最多返回的匹配行数（默认 500，上限同 /lines 的 MAX_LIMIT）；
    /// 截断时 truncated=true 且 lastNo=最后一条返回行的 no。
    filter_limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FollowPage {
    lines: Vec<BridgeLine>,
    last_no: u64,
    timed_out: bool,
    truncated: bool,
}

pub(crate) async fn follow(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<FollowParams>,
) -> Response {
    let since = p.since_no.unwrap_or(0);
    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    // 无过滤字段：旧行为逐字节不变（含不施加 filterLimit）
    let noop = f.is_noop();
    let limit = p.filter_limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let timeout = std::time::Duration::from_millis(p.timeout_ms.unwrap_or(5000).min(30_000));
    let deadline = tokio::time::Instant::now() + timeout;
    // 本地扫描游标：过滤路径下每轮空转后推进到高水位、只扫增量。
    // 若固定 since 每 50ms 重扫全窗口，高吞吐下 15s 超时最多 20k 行 × 数百次迭代。
    // 单调推进（不回退）：ring 清空后 lastNo 归 0 的瞬间不丢游标。
    // 高水位取本轮返回行的末行 no——服务一次调用返回「no > scanned 的全部行」，
    // 末行 no 即该瞬间的 lastNo（与抽取前 (lines, lastNo) 原子对语义一致）。
    let mut scanned = since;
    loop {
        match ctx.service.lines_after(&id, scanned, usize::MAX) {
            Ok(lines) => {
                let high = lines.last().map(|l| l.no).unwrap_or(scanned);
                if noop {
                    if !lines.is_empty() {
                        return Json(FollowPage {
                            lines,
                            last_no: high,
                            timed_out: false,
                            truncated: false,
                        })
                        .into_response();
                    }
                } else {
                    let (batch, truncated, last_no) = filter_follow_batch(lines, &f, limit, high);
                    if !batch.is_empty() {
                        return Json(FollowPage {
                            lines: batch,
                            last_no,
                            timed_out: false,
                            truncated,
                        })
                        .into_response();
                    }
                }
                if high > scanned {
                    scanned = high;
                }
            }
            Err(_) => return not_found(),
        }
        if tokio::time::Instant::now() >= deadline {
            let last_no = ctx.service.last_no(&id).unwrap_or(0);
            return Json(FollowPage {
                lines: vec![],
                last_no,
                timed_out: true,
                truncated: false,
            })
            .into_response();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// =============================== /histogram ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistParams {
    #[serde(flatten)]
    filter: FilterFields,
    bucket: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistBucket {
    bucket_start: u64,
    count: u64,
}

pub(crate) async fn histogram(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<HistParams>,
) -> Response {
    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let filt = match filtered(&ctx, &id, &f) {
        Some(v) => v,
        None => return not_found(),
    };
    let bucket = p.bucket.unwrap_or(1000).max(1);
    let mut map: BTreeMap<u64, u64> = BTreeMap::new();
    for l in &filt {
        let b = (l.epoch_millis / bucket) * bucket;
        *map.entry(b).or_insert(0) += 1;
    }
    let out: Vec<HistBucket> = map
        .into_iter()
        .map(|(bucket_start, count)| HistBucket {
            bucket_start,
            count,
        })
        .collect();
    Json(out).into_response()
}

// =============================== /bookmarks & /alerts（只读镜像） ===============================

/// 用户在应用里标记的书签（前端推送的只读镜像；`no` 为 UI 行号）。
pub(crate) async fn bookmarks(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.service.bookmarks(&id) {
        Ok(b) => Json(b).into_response(),
        Err(_) => not_found(),
    }
}

/// 告警历史（前端推送的只读镜像，环形 100 条、新的在前；`no` 为 UI 行号）。
pub(crate) async fn alerts(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.service.alerts(&id) {
        Ok(a) => Json(a).into_response(),
        Err(_) => not_found(),
    }
}

// =============================== /export ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportParams {
    /// 只返回 `{ path, sizeBytes, missing }` 元信息而不拉流；宽松取值 1/true/yes/on。
    info: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportInfo {
    path: String,
    size_bytes: u64,
    missing: bool,
}

/// 全量历史导出：流式返回会话落盘日志（追加写 TSV：`ts<TAB>dir<TAB>text`，
/// 含 ring 已淘汰的行、无原始字节；`clearLog` 会截断该文件）。离线会话返回其来源文件。
pub(crate) async fn export_log(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<ExportParams>,
) -> Response {
    let path = match ctx.service.log_path(&id) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => return not_found(),
    };
    let meta = std::fs::metadata(&path);
    let want_info = p
        .info
        .as_deref()
        .map(|s| {
            matches!(
                s.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    if want_info {
        return Json(ExportInfo {
            size_bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            missing: meta.is_err(),
            path,
        })
        .into_response();
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                "log file not found on disk (cleared or never written)",
            )
                .into_response()
        }
    };
    let stream = ReaderStream::with_capacity(file, 64 * 1024);
    let mut resp = Response::new(Body::from_stream(stream));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"serialtool-log.tsv\""),
    );
    resp
}

#[cfg(test)]
mod tests {
    //! /follow 服务端过滤的纯函数投影（filter_follow_batch）。
    use super::*;
    use crate::bridge::routes::mk_line;

    fn mk_ff(re: Option<&str>, dir: Option<&str>, exclude: Option<&str>) -> FilterFields {
        FilterFields {
            re: re.map(Into::into),
            dir: dir.map(Into::into),
            exclude: exclude.map(Into::into),
            ..Default::default()
        }
    }

    fn mk_batch(nos: &[u64], text: &str) -> Vec<BridgeLine> {
        nos.iter()
            .map(|&n| mk_line(n, Dir::Rx, text, None, n * 1000))
            .collect()
    }

    #[test]
    fn follow_filter_noop_passes_all_and_keeps_high() {
        // 用例 1 的纯函数投影：无过滤字段 → 全行返回、lastNo=高水位、不截断
        let spec = build_filter(&FilterFields::default()).ok().unwrap();
        assert!(spec.is_noop());
        let (batch, truncated, last_no) =
            filter_follow_batch(mk_batch(&[3, 4, 5], "any"), &spec, 500, 5);
        assert_eq!(batch.len(), 3);
        assert!(!truncated);
        assert_eq!(last_no, 5);
    }

    #[test]
    fn follow_filter_zero_match_returns_empty_but_advances() {
        // 用例 2：零匹配 → 空行集，lastNo 仍推进到高水位（防 livelock）
        let spec = build_filter(&mk_ff(Some("error"), None, None))
            .ok()
            .unwrap();
        assert!(!spec.is_noop());
        let (batch, truncated, last_no) =
            filter_follow_batch(mk_batch(&[3, 4, 5], "heartbeat ok"), &spec, 500, 5);
        assert!(batch.is_empty());
        assert!(!truncated);
        assert_eq!(last_no, 5);
    }

    #[test]
    fn follow_filter_truncated_lastno_is_last_returned_line() {
        // 用例 3：全匹配超 filterLimit → truncated、lastNo=最后一条返回行的 no
        let spec = build_filter(&mk_ff(Some("beat"), None, None)).ok().unwrap();
        let (batch, truncated, last_no) =
            filter_follow_batch(mk_batch(&[1, 2, 3, 4, 5], "heartbeat"), &spec, 2, 5);
        assert_eq!(batch.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1, 2]);
        assert!(truncated);
        assert_eq!(last_no, 2); // 未消费的 3..=5 留给下一轮
    }

    #[test]
    fn follow_filter_exactly_at_limit_not_truncated() {
        // 边界：匹配数 == limit → 不截断，lastNo = 高水位
        let spec = build_filter(&mk_ff(Some("beat"), None, None)).ok().unwrap();
        let (batch, truncated, last_no) =
            filter_follow_batch(mk_batch(&[1, 2], "heartbeat"), &spec, 2, 2);
        assert_eq!(batch.len(), 2);
        assert!(!truncated);
        assert_eq!(last_no, 2);
    }

    #[test]
    fn follow_filter_regex_inline_case_insensitive() {
        // 用例 5：(?i) 行内 flag 对 text 生效（小写 error 命中大写 ERROR）
        let spec = build_filter(&mk_ff(Some("(?i)error"), None, None))
            .ok()
            .unwrap();
        let lines = vec![
            mk_line(1, Dir::Rx, "ERROR found", None, 1000),
            mk_line(2, Dir::Rx, "all good", None, 2000),
        ];
        let (batch, _, _) = filter_follow_batch(lines, &spec, 500, 2);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].no, 1);
    }

    #[test]
    fn follow_filter_dir_and_exclude_vocabulary() {
        // 全套词汇：dir 定向 + exclude 排噪（OTA 场景：只要 tx 且排除心跳）
        let spec = build_filter(&mk_ff(None, Some("tx"), Some("HEARTBEAT")))
            .ok()
            .unwrap();
        let mut lines = vec![
            mk_line(1, Dir::Tx, "OTA chunk 12", None, 1000),
            mk_line(2, Dir::Tx, "HEARTBEAT 3000ms", None, 2000),
            mk_line(3, Dir::Rx, "OTA chunk ack", None, 3000),
        ];
        let (batch, _, last_no) = filter_follow_batch(std::mem::take(&mut lines), &spec, 500, 3);
        assert_eq!(batch.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1]);
        assert_eq!(last_no, 3); // 未匹配行 2/3 也计入高水位
    }

    #[test]
    fn follow_filter_bad_regex_rejected_by_build_filter() {
        // 用例 4：非法正则在 build_filter 即被拒（handler 转 400 + 编译错误）
        let err = build_filter(&mk_ff(Some("(unclosed"), None, None))
            .err()
            .unwrap();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("invalid regex"));
    }
}
