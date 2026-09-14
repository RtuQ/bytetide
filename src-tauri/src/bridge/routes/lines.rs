//! 行数据路由：`/lines` `/follow` `/histogram` `/bookmarks` `/alerts` `/export`。
//!
//! 读取有界性（评审 P1-1）：`/lines` 与 `/histogram` 不再全量物化快照——
//! 无过滤把 `offset/limit` 直接下推为 no 区间页读（ring nos 连续、离线文件
//! nos 连续，`lines_after`/`lines_before`/`line_by_no` 一步定位）；有过滤走
//! 固定页大小流式扫描，只保留命中分页窗口与计数。对分页离线会话，任意
//! `limit` 请求的内存占用与文件行数无关。

use std::collections::{BTreeMap, VecDeque};

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
    apply_filter, build_filter, not_found, BridgeCtx, BridgeService, FilterFields, FilterSpec,
    DEFAULT_LIMIT, MAX_LIMIT,
};
use crate::bridge::ServiceError;
use bytetide_core::serial::manager::{BridgeLine, BridgeStats};
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
    let stats = match ctx.service.stats(&id) {
        Ok(s) => s,
        Err(_) => return not_found(),
    };
    let offset = p.offset.unwrap_or(0);
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let (page, total) = if f.is_noop() {
        match bounded_fetch(&*ctx.service, &id, &p, &stats, offset, limit) {
            Ok(x) => x,
            Err(_) => return not_found(),
        }
    } else {
        // Err = 会话缺失或页读失败（存储故障不与「无命中」混淆，统一 404 语义）
        match stream_scan(&*ctx.service, &id, &f, &p, offset, limit) {
            Ok(x) => x,
            Err(_) => return not_found(),
        }
    };
    let first_no = stats.first_no;
    let last_no = stats.last_no;
    let size = stats.size;
    let truncated = total.saturating_sub(offset) > limit;

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

/// 流式扫描页大小（有过滤路径；单次内存 = 一页行 + 命中窗口）。
const SCAN_PAGE: usize = 1000;
/// 命中窗口上限（offset+limit 的钳制）：防超大 offset 把滚动窗口撑爆。
/// 超出窗口的 offset 返回空页（total 仍精确）——正常分页 consumer 远用不到。
const SCAN_WINDOW_CAP: usize = 20_000;

/// 无过滤的有界读取：把 `offset/limit` 直接下推为 no 区间页读，不做任何
/// 全量扫描。选择语义与原 select_lines 一致（选择模式下 total 的口径保持
/// 原响应形状：no=0/1、区间=命中数、around=窗口长度、last=min(N, size)）。
/// nos 在 [first_no, last_no] 连续（ring 与离线文件皆然），区间计数用算术。
fn bounded_fetch(
    svc: &dyn BridgeService,
    id: &str,
    p: &LinesParams,
    stats: &BridgeStats,
    offset: usize,
    limit: usize,
) -> Result<(Vec<BridgeLine>, usize), ServiceError> {
    let (first, last, size) = (stats.first_no, stats.last_no, stats.size);
    let empty = Ok((Vec::new(), 0usize));
    if first == 0 || last == 0 {
        return empty;
    }
    // no=X：单行直取
    if let Some(no) = p.no {
        return Ok(match svc.line_by_no(id, no)? {
            Some(l) if offset == 0 => (vec![l], 1),
            Some(_) => (Vec::new(), 1),
            None => (Vec::new(), 0),
        });
    }
    // from+to：闭区间命中
    if let (Some(from), Some(to)) = (p.from, p.to) {
        let lo = from.max(first);
        let hi = to.min(last);
        if hi < lo {
            return empty;
        }
        let total = (hi - lo + 1) as usize;
        let start = lo.saturating_add(offset as u64);
        if start > hi {
            return Ok((Vec::new(), total));
        }
        let page = svc
            .lines_after(id, start - 1, limit)?
            .into_iter()
            .take_while(|l| l.no <= hi)
            .collect();
        return Ok((page, total));
    }
    // last=N：先确定最新 N 行组成的选择集，再在选择集内应用 offset/limit。
    if let Some(n) = p.last {
        let total = (n as u64).min(size as u64) as usize;
        if offset >= total {
            return Ok((Vec::new(), total));
        }
        let selected_first = last.saturating_sub(total as u64).saturating_add(1);
        let page_first = selected_first.saturating_add(offset as u64);
        let page = svc.lines_after(id, page_first.saturating_sub(1), limit)?;
        return Ok((page, total));
    }
    // since_no：no > since
    if let Some(since) = p.since_no {
        let start = since.max(first - 1).saturating_add(1);
        if start > last {
            return empty;
        }
        let total = (last - start + 1) as usize;
        let at = start.saturating_add(offset as u64);
        if at > last {
            return Ok((Vec::new(), total));
        }
        let page = svc
            .lines_after(id, at - 1, limit)?
            .into_iter()
            .take_while(|l| l.no <= last)
            .collect();
        return Ok((page, total));
    }
    // around：锚点 ±span（精确命中闭区间 [a-span, a+len) 镜像原 index 语义；
    // 锚点缺失取首个 no > around 的行（窗口长 2×span），越过表尾取最新 span 行）
    if let Some(around) = p.around {
        let span = p.span.unwrap_or(10);
        let (lo, hi) = match svc.line_by_no(id, around)? {
            Some(_) => (
                around.saturating_sub(span).max(first),
                around.saturating_add(span).min(last),
            ),
            None => match svc.lines_after(id, around, 1)?.first().map(|l| l.no) {
                Some(next) => (
                    next.saturating_sub(span).max(first),
                    next.saturating_add(span).saturating_sub(1).min(last),
                ),
                None => (last.saturating_sub(span.saturating_sub(1)).max(first), last),
            },
        };
        if lo > hi {
            return empty;
        }
        let total = (hi - lo + 1) as usize;
        let start = lo.saturating_add(offset as u64);
        if start > hi {
            return Ok((Vec::new(), total));
        }
        let page = svc
            .lines_after(id, start - 1, limit)?
            .into_iter()
            .take_while(|l| l.no <= hi)
            .collect();
        return Ok((page, total));
    }
    // from（无 to）：no >= from
    if let Some(from) = p.from {
        let lo = from.max(first);
        if lo > last {
            return empty;
        }
        let total = (last - lo + 1) as usize;
        let start = lo.saturating_add(offset as u64);
        if start > last {
            return Ok((Vec::new(), total));
        }
        let page = svc.lines_after(id, start - 1, limit)?;
        return Ok((page, total));
    }
    // 缺省：全量顺序分页
    let total = size;
    let start = first.saturating_add(offset as u64);
    if start > last {
        return Ok((Vec::new(), total));
    }
    Ok((svc.lines_after(id, start - 1, limit)?, total))
}

/// 有过滤的流式扫描：固定页大小 `lines_after` 走完全程，只保留「命中分页
/// 窗口」与计数——内存与命中总量无关（评审 P1-1）。选择语义镜像原
/// select_lines（对命中序列做 index 窗口）：
/// - no：line_by_no 单行（未命中过滤 → 空页）；
/// - last=N：选择集 = 最后 N 个命中 → 尾窗口（cap = min(N, offset+limit)，
///   再与 SCAN_WINDOW_CAP 取小）；
/// - 其余（from/to、since、from、缺省）：选择集 = 范围内全部命中 → 头窗口
///   （跳过前 offset 个命中后收集 limit 个，内存 ≤ limit）；
/// - around：命中序列中锚点的 ±span index 窗口（精确/最近邻/越表尾）。
fn stream_scan(
    svc: &dyn BridgeService,
    id: &str,
    f: &FilterSpec,
    p: &LinesParams,
    offset: usize,
    limit: usize,
) -> Result<(Vec<BridgeLine>, usize), ()> {
    // no=X：单行
    if let Some(no) = p.no {
        let line = svc.line_by_no(id, no).map_err(drop)?;
        return Ok(match line {
            Some(mut bl) => match apply_filter(&bl, f) {
                Some(hit) => {
                    bl.r#match = hit;
                    (if offset == 0 { vec![bl] } else { Vec::new() }, 1)
                }
                None => (Vec::new(), 0),
            },
            None => (Vec::new(), 0),
        });
    }
    // around：需要锚点命中序号，独立扫描分支
    if let Some(around) = p.around {
        let span = p.span.unwrap_or(10).min(SCAN_WINDOW_CAP as u64) as usize;
        return stream_scan_around(svc, id, f, around, span, offset, limit);
    }
    // 其余模式统一扫描：游标起点按选择模式跳过必不在窗口的前缀
    let (start_cursor, last_n) = if let (Some(from), Some(_to)) = (p.from, p.to) {
        (from.saturating_sub(1), None::<usize>) // to 截断在循环内按 no 判断
    } else if let Some(n) = p.last {
        (0, Some(n))
    } else if let Some(since) = p.since_no {
        (since, None)
    } else if let Some(from) = p.from {
        (from.saturating_sub(1), None)
    } else {
        (0, None)
    };
    // 尾窗口 cap（仅 last=N）：必须保留最后 N 个命中，才能在该选择集内从
    // offset 起分页；只保留 offset+limit 会错误地把页定位到全局命中序列末尾。
    // N 超过安全上限时仍只保留尾部上限窗口，窗口之外的页返回空但 total 精确。
    let tail_cap = last_n.map(|n| n.min(SCAN_WINDOW_CAP));
    let mut cursor = start_cursor;
    let mut keep: VecDeque<BridgeLine> = VecDeque::new();
    let mut matched = 0usize;
    loop {
        let page = svc.lines_after(id, cursor, SCAN_PAGE).map_err(drop)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|l| l.no).unwrap_or(cursor);
        for l in page {
            if let (Some(from), Some(to)) = (p.from, p.to) {
                if l.no < from || l.no > to {
                    continue;
                }
            }
            if let Some(hit) = apply_filter(&l, f) {
                let mut bl = l;
                bl.r#match = hit;
                matched += 1;
                match tail_cap {
                    // 头窗口：跳过前 offset 个命中，收集 limit 个（精确分页）
                    None => {
                        if matched > offset && keep.len() < limit {
                            keep.push_back(bl);
                        }
                    }
                    // 尾窗口：保留最后 cap 个命中
                    Some(cap) => {
                        if keep.len() == cap {
                            keep.pop_front();
                        }
                        keep.push_back(bl);
                    }
                }
            }
        }
    }
    let total = match last_n {
        Some(n) => matched.min(n),
        None => matched,
    };
    // 头窗口：keep 即命中[offset..offset+limit]（不足则全量）。
    // 尾窗口：keep = 最后 keep.len() 个命中，全局起点 win_first；选择集起点
    // sel_first = matched - total；分页起点落在窗口/选择集之前 → 空页
    let page = match tail_cap {
        None => keep.into_iter().collect(),
        Some(_) => {
            let win_first = matched.saturating_sub(keep.len());
            let sel_first = matched.saturating_sub(total);
            let skip_at = sel_first.max(win_first).saturating_add(offset);
            if skip_at < win_first || skip_at >= matched {
                Vec::new()
            } else {
                keep.into_iter()
                    .skip(skip_at - win_first)
                    .take(limit)
                    .collect()
            }
        }
    };
    Ok((page, total))
}

/// around 的流式窗口：锚点在命中序列中的 index i（精确=该行命中时的序号，
/// 否则=首个 no > around 的命中序号，越表尾=总命中数），窗口 = [i-span, i+span]
/// （精确）或 [i-span, i+span-1]（最近邻），index 越界端截断。扫描期锚点前只保
/// 留最后 span+1 个命中，锚点后收集 span 个。
#[allow(clippy::too_many_arguments)]
fn stream_scan_around(
    svc: &dyn BridgeService,
    id: &str,
    f: &FilterSpec,
    around: u64,
    span: usize,
    offset: usize,
    limit: usize,
) -> Result<(Vec<BridgeLine>, usize), ()> {
    let mut cursor = 0u64;
    let mut win: VecDeque<BridgeLine> = VecDeque::new();
    let mut le = 0usize; // no ≤ around 的命中数（锚点精确命中时含锚点）
    let mut exact_idx: Option<usize> = None;
    let mut post = 0usize; // 锚点后已收集的命中数
    loop {
        let page = svc.lines_after(id, cursor, SCAN_PAGE).map_err(drop)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|l| l.no).unwrap_or(cursor);
        for l in page {
            let Some(hit) = apply_filter(&l, f) else {
                continue;
            };
            let mut bl = l;
            bl.r#match = hit;
            if bl.no <= around {
                le += 1;
                if bl.no == around {
                    exact_idx = Some(le - 1);
                }
                if win.len() == span + 1 {
                    win.pop_front();
                }
                win.push_back(bl);
            } else if post < span {
                post += 1;
                win.push_back(bl);
            }
        }
    }
    // 命中序列 index 窗口 [start, end]（含端点）
    let anchor_idx = exact_idx.unwrap_or(le);
    let start = anchor_idx.saturating_sub(span);
    let end = anchor_idx
        .saturating_add(span)
        .saturating_sub(usize::from(exact_idx.is_none()));
    let total_hits = le + post;
    let win_first = total_hits.saturating_sub(win.len());
    let skip = start.saturating_sub(win_first);
    let take = end
        .saturating_sub(start)
        .saturating_add(1)
        .min(win.len().saturating_sub(skip));
    // 窗口切片后按 offset/limit 分页（镜像原 select_lines 的窗口+分页两步）
    let total = take;
    let page = win
        .into_iter()
        .skip(skip)
        .take(take)
        .skip(offset)
        .take(limit)
        .collect();
    Ok((page, total))
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
    let bucket = p.bucket.unwrap_or(1000).max(1);
    // 流式扫描计数（评审 P1-1 同源）：命中只累加桶计数，不物化命中行列表
    let mut cursor = 0u64;
    let mut map: BTreeMap<u64, u64> = BTreeMap::new();
    loop {
        let page = match ctx.service.lines_after(&id, cursor, SCAN_PAGE) {
            Ok(v) => v,
            Err(_) => return not_found(),
        };
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|l| l.no).unwrap_or(cursor);
        for l in &page {
            if apply_filter(l, &f).is_some() {
                let b = (l.epoch_millis / bucket) * bucket;
                *map.entry(b).or_insert(0) += 1;
            }
        }
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
                .into_response();
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
