//! 发送路由：`/sessions/:id/send` 与 `/sessions/:id/exchange`。
//!
//! 两者都由 `allowSend` 独立门控（403 文本与抽取前一致）；/exchange 的非法 matcher
//! 一律 400 + ApiError 信封，绝不静默降级为 match-all。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{bytes_find, line_bytes, mask_find, BridgeCtx};
use crate::bridge::error::ApiError;
use crate::bridge::service::BridgeService;
use bytetide_core::serial::manager::{BridgeLine, SendMode, SendRequest};
use bytetide_core::serial::port::Dir;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendBody {
    mode: Option<String>,
    text: String,
}

pub(crate) async fn send(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Response {
    if !ctx.cfg.read().allow_send {
        return (
            StatusCode::FORBIDDEN,
            "allowSend is disabled; enable it in the bridge panel".to_string(),
        )
            .into_response();
    }
    let mode = match body.mode.as_deref() {
        Some("hex") => SendMode::Hex,
        _ => SendMode::Ascii,
    };
    match ctx.service.send(
        &id,
        SendRequest {
            mode,
            text: body.text,
        },
    ) {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExchangeMatch {
    re: Option<String>,
    hex: Option<String>,
    mask: Option<String>,
    dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExchangeBody {
    send: SendBody,
    wait_ms: Option<u64>,
    r#match: Option<ExchangeMatch>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExchangePage {
    sent: bool,
    response: Option<BridgeLine>,
    waited_ms: u64,
}

/// 编译后的交换匹配器：`hex`/`mask` 为空 = 未启用该匹配器；`re` 为 None = 不限文本。
#[derive(Debug)]
struct CompiledExchangeMatch {
    dir: Dir,
    re: Option<Regex>,
    hex: Vec<u8>,
    mask: Vec<Option<u8>>,
}

/// 字段为空串/纯空白 = 未携带（与缺省等价）。
fn opt_non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// 交换匹配器编译：None → 默认任意 RX 行；空串字段视为未携带；
/// re/hex/mask 互斥（同时携带多于一个 → 400）；非法值返回稳定错误码
/// （invalid_regex/invalid_hex/invalid_mask/invalid_direction），绝不降级为 match-all。
fn compile_exchange_match(
    input: Option<&ExchangeMatch>,
) -> Result<CompiledExchangeMatch, ApiError> {
    let Some(m) = input else {
        return Ok(CompiledExchangeMatch {
            dir: Dir::Rx,
            re: None,
            hex: Vec::new(),
            mask: Vec::new(),
        });
    };
    let dir = match opt_non_empty(m.dir.as_deref()) {
        None | Some("rx") => Dir::Rx,
        Some("tx") => Dir::Tx,
        Some(other) => {
            return Err(ApiError::bad_request(
                "invalid_direction",
                format!("dir must be rx|tx, got {other:?}"),
            ));
        }
    };
    let (re_src, hex_src, mask_src) = (
        opt_non_empty(m.re.as_deref()),
        opt_non_empty(m.hex.as_deref()),
        opt_non_empty(m.mask.as_deref()),
    );
    if [re_src.is_some(), hex_src.is_some(), mask_src.is_some()]
        .into_iter()
        .filter(|set| *set)
        .count()
        > 1
    {
        return Err(ApiError::bad_request(
            "conflicting_matchers",
            "set at most one of re/hex/mask",
        ));
    }
    let re = re_src
        .map(Regex::new)
        .transpose()
        .map_err(|e| ApiError::bad_request("invalid_regex", format!("invalid regex: {e}")))?;
    let hex = match hex_src {
        Some(s) => parse_hex_strict(s)?,
        None => Vec::new(),
    };
    let mask = match mask_src {
        Some(s) => parse_mask_strict(s)?,
        None => Vec::new(),
    };
    Ok(CompiledExchangeMatch { dir, re, hex, mask })
}

/// 严格 hex：去空白后必须非空、偶数长度、纯 ASCII hex 对（宽松版 `parse_hex`
/// 会静默丢非法对，导致「看似过滤、实为 match-all」，此处一律 400）。
fn parse_hex_strict(s: &str) -> Result<Vec<u8>, ApiError> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_hex",
            "hex must not be empty",
        ));
    }
    if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "invalid_hex",
            format!("hex must be ASCII hex pairs, got {s:?}"),
        ));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(ApiError::bad_request(
            "invalid_hex",
            format!("hex must be even-length pairs, got {s:?}"),
        ));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    for i in (0..cleaned.len()).step_by(2) {
        out.push(u8::from_str_radix(&cleaned[i..i + 2], 16).expect("validated hex pair"));
    }
    Ok(out)
}

/// 严格 mask：去空白后每对要么 `??` 要么两位 hex；奇数残留/非法字符报错。
fn parse_mask_strict(s: &str) -> Result<Vec<Option<u8>>, ApiError> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return Err(ApiError::bad_request(
            "invalid_mask",
            "mask must not be empty",
        ));
    }
    // 先确保纯 ASCII（hex 或 ?），后续按字节切片才安全
    if !cleaned.chars().all(|c| c == '?' || c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(
            "invalid_mask",
            format!("mask pairs must be hex digits or ??, got {s:?}"),
        ));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(ApiError::bad_request(
            "invalid_mask",
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
                u8::from_str_radix(pair, 16).expect("validated hex pair"),
            ));
        } else {
            return Err(ApiError::bad_request(
                "invalid_mask",
                format!("mask pairs must be hex digits or ??, got {s:?}"),
            ));
        }
        i += 2;
    }
    Ok(out)
}

/// 按编译好的匹配器找第一条命中行：dir 不等跳过；re 有则 is_match(text)；
/// hex/mask 非空则按行字节（优先原始 bytes，缺失回退 text UTF-8）查找。
fn find_exchange_response(lines: &[BridgeLine], m: &CompiledExchangeMatch) -> Option<BridgeLine> {
    lines
        .iter()
        .find(|l| {
            if l.dir != m.dir {
                return false;
            }
            if let Some(re) = &m.re {
                if !re.is_match(&l.text) {
                    return false;
                }
            }
            if !m.hex.is_empty() && bytes_find(&line_bytes(l), &m.hex).is_none() {
                return false;
            }
            if !m.mask.is_empty() && mask_find(&line_bytes(l), &m.mask).is_none() {
                return false;
            }
            true
        })
        .cloned()
}

/// 交换轮询间隔：真实路径 30ms（测试传 1ms 提速）。
const EXCHANGE_POLL: std::time::Duration = std::time::Duration::from_millis(30);

/// 交换编排：编译 matcher → 取基线（last_no，先于发送）→ 发送 → 轮询 follow 找命中。
/// 独立成函数 + `calls` 记录调用顺序，供 fake-service 回归测试断言
/// last_no → send → follow，且快响应（no=baseline+1）在首次轮询即命中。
/// 长轮询由 `lines_after` + 游标推进实现（高水位取本轮返回行末行 no，
/// 与抽取前 `(lines, lastNo)` 原子对语义一致）。
async fn run_exchange<S: BridgeService + ?Sized>(
    svc: &S,
    id: &str,
    body: &ExchangeBody,
    poll: std::time::Duration,
    calls: Option<&std::sync::Mutex<Vec<&'static str>>>,
) -> Result<Response, ApiError> {
    let trace = |name: &'static str| {
        if let Some(c) = calls {
            c.lock().expect("test calls mutex").push(name);
        }
    };
    let matcher = compile_exchange_match(body.r#match.as_ref())?;
    trace("last_no");
    let baseline = svc
        .last_no(id)
        .map_err(|_| ApiError::not_found("session_not_found"))?;
    let mode = match body.send.mode.as_deref() {
        Some("hex") => SendMode::Hex,
        _ => SendMode::Ascii,
    };
    trace("send");
    svc.send(
        id,
        SendRequest {
            mode,
            text: body.send.text.clone(),
        },
    )
    .map_err(|e| ApiError::bad_request("send_failed", e.to_string()))?;
    let start = std::time::Instant::now();
    let wait = std::time::Duration::from_millis(body.wait_ms.unwrap_or(2000).min(30_000));
    let deadline = tokio::time::Instant::now() + wait;
    // 轮询游标从基线起步：send 前已存在的行（no ≤ baseline）永不参与匹配
    let mut cursor = baseline;
    loop {
        trace("follow");
        let lines = match svc.lines_after(id, cursor, usize::MAX) {
            Ok(l) => l,
            Err(_) => return Err(ApiError::not_found("session_not_found")),
        };
        if let Some(l) = find_exchange_response(&lines, &matcher) {
            return Ok(Json(ExchangePage {
                sent: true,
                response: Some(l),
                waited_ms: start.elapsed().as_millis() as u64,
            })
            .into_response());
        }
        if let Some(last) = lines.last() {
            cursor = last.no;
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(Json(ExchangePage {
                sent: true,
                response: None,
                waited_ms: start.elapsed().as_millis() as u64,
            })
            .into_response());
        }
        tokio::time::sleep(poll).await;
    }
}

pub(crate) async fn exchange(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Result<Response, ApiError> {
    if !ctx.cfg.read().allow_send {
        return Ok((
            StatusCode::FORBIDDEN,
            "allowSend is disabled; enable it in the bridge panel".to_string(),
        )
            .into_response());
    }
    run_exchange(&*ctx.service, &id, &body, EXCHANGE_POLL, None).await
}

#[cfg(test)]
mod tests {
    //! /exchange 严格 matcher（非法输入 400，绝不降级 match-all）与
    //! run_exchange：fake-service 排序回归（基线先行 + 快响应不丢）。
    //!
    //! matcher 向量全部来自共享黄金样本 `testdata/protocol/matcher-v1.json`
    //! （TS 消费：`src/__tests__/protocol-fixtures.test.ts`）——错误码是 Stage 1
    //! 冻结的跨语言契约，任一侧漂移即红。

    use super::*;
    use crate::bridge::error::ServiceError;
    use crate::bridge::routes::mk_line;
    use bytetide_core::serial::manager::{
        BridgeAlert, BridgeAnnotation, BridgeBookmark, BridgeStats, PlotConfig, SessionSnap,
    };
    use serde::Deserialize;
    use std::path::PathBuf;

    // ---------------- 共享黄金样本（matcher-v1.json） ----------------

    /// fixture 顶层（serde 白名单取键，未知键如 `$about`/`version` 自动忽略）。
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MatcherFixture {
        valid_matchers: FxValidMatchers,
        invalid_matchers: FxInvalidMatchers,
        hex_strict: FxStrict,
        mask_strict: FxStrict,
        find_cases: FxFindCases,
    }

    #[derive(Deserialize)]
    struct FxValidMatchers {
        cases: Vec<FxValidCase>,
    }

    #[derive(Deserialize)]
    struct FxValidCase {
        name: String,
        #[serde(rename = "match")]
        match_: Option<FxMatch>,
        compiled: FxCompiled,
    }

    #[derive(Deserialize)]
    struct FxMatch {
        re: Option<String>,
        hex: Option<String>,
        mask: Option<String>,
        dir: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxCompiled {
        dir: String,
        re_source: Option<String>,
        hex_bytes: Vec<u8>,
        mask: Vec<Option<u8>>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxInvalidMatchers {
        cases: Vec<FxInvalidCase>,
        stable_codes: Vec<String>,
    }

    #[derive(Deserialize)]
    struct FxInvalidCase {
        name: String,
        #[serde(rename = "match")]
        match_: FxMatch,
        code: String,
    }

    #[derive(Deserialize)]
    struct FxStrict {
        cases: Vec<FxStrictCase>,
    }

    /// hexStrict 期望 `bytes`、maskStrict 期望 `mask`，二选一存在。
    #[derive(Deserialize)]
    struct FxStrictCase {
        input: String,
        ok: bool,
        bytes: Option<Vec<u8>>,
        mask: Option<Vec<Option<u8>>>,
        code: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxFindCases {
        cases: Vec<FxFindCase>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxFindCase {
        name: String,
        #[serde(rename = "match")]
        match_: FxMatch,
        lines: Vec<FxFindLine>,
        expected_no: Option<u64>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FxFindLine {
        no: u64,
        dir: String,
        text: String,
        bytes: Option<Vec<u8>>,
        epoch_millis: u64,
    }

    fn matcher_fixture() -> MatcherFixture {
        // 相对本文件 4 级上跳到仓库根（routes → bridge → src → src-tauri → 根）
        serde_json::from_str(include_str!(
            "../../../../testdata/protocol/matcher-v1.json"
        ))
        .expect("matcher-v1.json fixture parses")
    }

    fn to_exchange_match(m: &FxMatch) -> ExchangeMatch {
        ExchangeMatch {
            re: m.re.clone(),
            hex: m.hex.clone(),
            mask: m.mask.clone(),
            dir: m.dir.clone(),
        }
    }

    fn fixture_dir(s: &str) -> Dir {
        if s == "tx" {
            Dir::Tx
        } else {
            Dir::Rx
        }
    }

    fn compile_fixture(m: Option<&FxMatch>) -> Result<CompiledExchangeMatch, ApiError> {
        match m {
            Some(m) => compile_exchange_match(Some(&to_exchange_match(m))),
            None => compile_exchange_match(None),
        }
    }

    #[test]
    fn matcher_fixture_valid_cases_compile_to_expected_shapes() {
        let fx = matcher_fixture();
        assert!(
            !fx.valid_matchers.cases.is_empty(),
            "fixture has valid cases"
        );
        for (i, c) in fx.valid_matchers.cases.iter().enumerate() {
            let tag = format!("validMatchers[{i}] {}", c.name);
            let compiled =
                compile_fixture(c.match_.as_ref()).unwrap_or_else(|e| panic!("{tag}: {e:?}"));
            assert_eq!(compiled.dir, fixture_dir(&c.compiled.dir), "{tag} dir");
            match &c.compiled.re_source {
                Some(src) => {
                    let re = compiled
                        .re
                        .as_ref()
                        .unwrap_or_else(|| panic!("{tag}: re missing"));
                    assert_eq!(re.as_str(), src, "{tag} re source");
                }
                None => assert!(compiled.re.is_none(), "{tag}: re should be absent"),
            }
            assert_eq!(compiled.hex, c.compiled.hex_bytes, "{tag} hex");
            assert_eq!(compiled.mask, c.compiled.mask, "{tag} mask");
        }
    }

    #[test]
    fn matcher_fixture_invalid_cases_rejected_with_stable_codes() {
        let fx = matcher_fixture();
        assert_eq!(
            fx.invalid_matchers.stable_codes,
            [
                "invalid_regex",
                "invalid_hex",
                "invalid_mask",
                "invalid_direction",
                "conflicting_matchers",
            ],
            "stable error codes are a frozen cross-language contract"
        );
        for (i, c) in fx.invalid_matchers.cases.iter().enumerate() {
            let tag = format!("invalidMatchers[{i}] {}", c.name);
            assert!(
                fx.invalid_matchers.stable_codes.contains(&c.code),
                "{tag}: code not in stableCodes"
            );
            let err = compile_fixture(Some(&c.match_)).expect_err(&tag);
            assert_eq!(err.status, StatusCode::BAD_REQUEST, "{tag} status");
            assert_eq!(err.code, c.code, "{tag} code");
        }
    }

    #[test]
    fn matcher_fixture_hex_strict_vectors() {
        let fx = matcher_fixture();
        for (i, c) in fx.hex_strict.cases.iter().enumerate() {
            let tag = format!("hexStrict[{i}] {:?}", c.input);
            match parse_hex_strict(&c.input) {
                Ok(bytes) => {
                    assert!(c.ok, "{tag}: expected rejection");
                    assert_eq!(bytes, c.bytes.as_deref().unwrap_or(&[]), "{tag} bytes");
                }
                Err(e) => {
                    assert!(!c.ok, "{tag}: expected ok");
                    assert_eq!(e.code, c.code.as_deref().unwrap_or(""), "{tag} code");
                }
            }
        }
    }

    #[test]
    fn matcher_fixture_mask_strict_vectors() {
        let fx = matcher_fixture();
        for (i, c) in fx.mask_strict.cases.iter().enumerate() {
            let tag = format!("maskStrict[{i}] {:?}", c.input);
            match parse_mask_strict(&c.input) {
                Ok(mask) => {
                    assert!(c.ok, "{tag}: expected rejection");
                    assert_eq!(mask, c.mask.clone().unwrap_or_default(), "{tag} mask");
                }
                Err(e) => {
                    assert!(!c.ok, "{tag}: expected ok");
                    assert_eq!(e.code, c.code.as_deref().unwrap_or(""), "{tag} code");
                }
            }
        }
    }

    #[test]
    fn matcher_fixture_find_cases_bytes_priority() {
        let fx = matcher_fixture();
        assert!(!fx.find_cases.cases.is_empty(), "fixture has find cases");
        for (i, c) in fx.find_cases.cases.iter().enumerate() {
            let tag = format!("findCases[{i}] {}", c.name);
            let m = compile_fixture(Some(&c.match_))
                .unwrap_or_else(|e| panic!("{tag}: compile failed: {e:?}"));
            let lines: Vec<BridgeLine> = c
                .lines
                .iter()
                .map(|l| {
                    // fixture 语义：空 bytes 数组视同缺失（回退 text UTF-8）
                    let bytes = l.bytes.clone().filter(|b| !b.is_empty());
                    mk_line(l.no, fixture_dir(&l.dir), &l.text, bytes, l.epoch_millis)
                })
                .collect();
            let hit = find_exchange_response(&lines, &m);
            assert_eq!(hit.map(|l| l.no), c.expected_no, "{tag}");
        }
    }

    // ---------------- run_exchange：fake-service 排序回归（基线先行 + 快响应不丢） ----------------

    /// 测试替身：std Mutex 存行（行数极少，同步锁足够）。
    struct FakeService {
        lines: std::sync::Mutex<Vec<BridgeLine>>,
        /// send 时立刻追加的回包（no 需 = baseline+1），模拟快响应设备。
        respond: Option<BridgeLine>,
        /// 置位时 send 报错（验证 400 send_failed 路径）。
        fail_send: bool,
    }

    impl Default for FakeService {
        fn default() -> Self {
            Self {
                lines: std::sync::Mutex::new(Vec::new()),
                respond: None,
                fail_send: false,
            }
        }
    }

    impl BridgeService for FakeService {
        fn list_sessions(&self) -> Vec<SessionSnap> {
            vec![]
        }
        fn session(&self, _id: &str) -> Result<SessionSnap, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn stats(&self, _id: &str) -> Result<BridgeStats, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn snapshot(&self, _id: &str) -> Result<Vec<BridgeLine>, ServiceError> {
            Ok(self.lines.lock().unwrap().clone())
        }
        fn lines_after(
            &self,
            _id: &str,
            no: u64,
            max: usize,
        ) -> Result<Vec<BridgeLine>, ServiceError> {
            let lines = self.lines.lock().unwrap();
            Ok(lines
                .iter()
                .filter(|l| l.no > no)
                .take(max)
                .cloned()
                .collect())
        }
        fn line_by_no(&self, _id: &str, no: u64) -> Result<Option<BridgeLine>, ServiceError> {
            Ok(self
                .lines
                .lock()
                .unwrap()
                .iter()
                .find(|l| l.no == no)
                .cloned())
        }
        fn last_no(&self, _id: &str) -> Result<u64, ServiceError> {
            Ok(self.lines.lock().unwrap().last().map(|l| l.no).unwrap_or(0))
        }
        fn log_path(&self, _id: &str) -> Result<PathBuf, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn plot(&self, _id: &str) -> Result<PlotConfig, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn set_plot(&self, _id: &str, _config: PlotConfig) -> Result<(), ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn bookmarks(&self, _id: &str) -> Result<Vec<BridgeBookmark>, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn alerts(&self, _id: &str) -> Result<Vec<BridgeAlert>, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn annotations(&self, _id: &str) -> Result<Vec<BridgeAnnotation>, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn set_annotations(
            &self,
            _id: &str,
            _values: Vec<BridgeAnnotation>,
        ) -> Result<(), ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn send(&self, _id: &str, _req: SendRequest) -> Result<(), ServiceError> {
            if self.fail_send {
                return Err(ServiceError::Backend("port gone".into()));
            }
            if let Some(l) = &self.respond {
                self.lines.lock().unwrap().push(l.clone());
            }
            Ok(())
        }
        fn notify_annotations_changed(&self, _session_id: &str, _annotations: &[BridgeAnnotation]) {
        }
        fn notify_plot_updated(&self, _session_id: &str, _config: &PlotConfig) {}
    }

    fn ex_body(text: &str, wait_ms: u64) -> ExchangeBody {
        ExchangeBody {
            send: SendBody {
                mode: None,
                text: text.into(),
            },
            wait_ms: Some(wait_ms),
            r#match: None,
        }
    }

    #[tokio::test]
    async fn run_exchange_baseline_precedes_send_and_hits_fast_response() {
        // 回归：基线必须先于 send 捕获。send 后立刻出现的 no=baseline+1 回行
        // 要在首次 follow 轮询命中（旧实现 send 后才 bridge_follow(id,0)，快响应被跳过）。
        let svc = FakeService {
            respond: Some(mk_line(8, Dir::Rx, "ACK", None, 2)),
            ..Default::default()
        };
        svc.lines
            .lock()
            .unwrap()
            .push(mk_line(7, Dir::Rx, "idle", None, 1)); // baseline = 7
        let calls = std::sync::Mutex::new(Vec::<&'static str>::new());
        let body = ex_body("ping", 500);
        let resp = run_exchange(
            &svc,
            "s1",
            &body,
            std::time::Duration::from_millis(1),
            Some(&calls),
        )
        .await
        .expect("exchange ok");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(*calls.lock().unwrap(), vec!["last_no", "send", "follow"]);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["sent"], serde_json::json!(true));
        assert_eq!(page["response"]["no"], serde_json::json!(8));
        assert_eq!(page["response"]["text"], serde_json::json!("ACK"));
    }

    #[tokio::test]
    async fn run_exchange_send_failure_is_400_send_failed() {
        let svc = FakeService {
            fail_send: true,
            ..Default::default()
        };
        let calls = std::sync::Mutex::new(Vec::<&'static str>::new());
        let body = ex_body("ping", 100);
        let err = run_exchange(
            &svc,
            "s1",
            &body,
            std::time::Duration::from_millis(1),
            Some(&calls),
        )
        .await
        .expect_err("send failed");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "send_failed");
        // last_no 已记录；send 失败后不再进入 follow 轮询
        assert_eq!(*calls.lock().unwrap(), vec!["last_no", "send"]);
        // 错误响应体为稳定 JSON 信封 {"error":{"code","message"}}
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], serde_json::json!("send_failed"));
        assert!(json["error"]["message"].is_string());
    }

    #[tokio::test]
    async fn run_exchange_times_out_with_null_response() {
        // 无回包：轮询到超时，sent=true、response=null
        let svc = FakeService::default();
        let body = ex_body("ping", 20);
        let resp = run_exchange(&svc, "s1", &body, std::time::Duration::from_millis(1), None)
            .await
            .expect("exchange ok");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let page: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["response"], serde_json::Value::Null);
        assert_eq!(page["sent"], serde_json::json!(true));
    }
}
