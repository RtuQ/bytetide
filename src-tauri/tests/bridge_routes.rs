//! REST 桥路由层集成测试（外部 crate 视角，只经 `serial_tool_lib::bridge` 的 pub 项）。
//!
//! 内存 fake service + `bridge::router(service, config)` 构造 axum Router，
//! 经 `tower::ServiceExt::oneshot` 直接喂 Request（不起真监听、不触串口）。
//! 覆盖 plan Task 7 Step 1 指定用例：匿名 /health、令牌门禁、allowSend 门禁、
//! 非法 matcher 400（ApiError 信封）、立即响应回归、会话缺失、运行态 serde 形状。

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use parking_lot::RwLock;
use tower::ServiceExt;

use bytetide_core::serial::manager::{
    BridgeAlert, BridgeAnnotation, BridgeBookmark, BridgeLine, BridgeStats, PlotConfig,
    SendRequest, SessionSnap, RING_CAP,
};
use bytetide_core::serial::port::{Dir, PortConfig};
use serial_tool_lib::bridge::{
    router, BridgeConfig, BridgeRuntime, BridgeService, BridgeView, RuntimeState, ServiceError,
};

/// 测试令牌：固定 64 hex（与生产 `new_token` 输出同形状）。
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SESSION_ID: &str = "s1";

// =============================== fake service ===============================

#[derive(Default)]
struct FakeState {
    /// 会话是否存在（false = /exchange 404 / /send 错误文案路径）。
    present: bool,
    lines: Vec<BridgeLine>,
    /// send 时立即追加的 RX 回包文本（no = baseline+1，模拟快响应设备）。
    reply: Option<String>,
    /// snapshot 调用计数（/lines、/annotations 必须走有界读取面，P1-1 回归）。
    snapshot_calls: usize,
    /// lines_after 调用记录 (since, max)：/lines 有界读取断言用。
    lines_after_calls: Vec<(u64, usize)>,
}

/// 内存 fake：单会话行表；`notify_*` no-op（路由层测试不关心前端事件）。
struct FakeService {
    inner: Mutex<FakeState>,
}

impl FakeService {
    /// 存在的会话：基线 1 行（no=7），send 后回 ACK（no=8）。
    fn live_with_reply() -> Self {
        Self {
            inner: Mutex::new(FakeState {
                present: true,
                lines: vec![mk_line(7, Dir::Rx, "idle")],
                reply: Some("ACK".into()),
                snapshot_calls: 0,
                lines_after_calls: Vec::new(),
            }),
        }
    }

    /// 不存在的会话：全部访问面返回 None / 错误。
    fn missing() -> Self {
        Self {
            inner: Mutex::new(FakeState::default()),
        }
    }

    /// 存在的会话：nos 1..=n，`no % 3 == 0` 的行文本为 noise（过滤用例）。
    fn populated(n: u64) -> Self {
        Self::populated_from(1, n)
    }

    /// 同 populated 但 nos 从 `first` 起（first_no > 1 模拟 ring 头部已淘汰）。
    fn populated_from(first: u64, n: u64) -> Self {
        let lines = (first..first + n)
            .map(|no| {
                let text = if no % 3 == 0 {
                    format!("noise-{no}")
                } else {
                    format!("hit-{no}")
                };
                mk_line(no, Dir::Rx, &text)
            })
            .collect();
        Self {
            inner: Mutex::new(FakeState {
                present: true,
                lines,
                reply: None,
                snapshot_calls: 0,
                lines_after_calls: Vec::new(),
            }),
        }
    }
}

fn mk_line(no: u64, dir: Dir, text: &str) -> BridgeLine {
    BridgeLine {
        no,
        ts: "00:00:00.000".into(),
        dir,
        text: text.into(),
        bytes: None,
        epoch_millis: no * 1000,
        r#match: None,
    }
}

impl BridgeService for FakeService {
    fn list_sessions(&self) -> Vec<SessionSnap> {
        let g = self.inner.lock().expect("fake mutex");
        if !g.present {
            return vec![];
        }
        vec![SessionSnap {
            id: SESSION_ID.into(),
            config: PortConfig::default(),
            status: "connected".into(),
            last_error: None,
            line_count: g.lines.len(),
            ring_cap: RING_CAP,
        }]
    }

    fn session(&self, id: &str) -> Result<SessionSnap, ServiceError> {
        self.list_sessions()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(ServiceError::NotFound)
    }

    fn stats(&self, _id: &str) -> Result<BridgeStats, ServiceError> {
        let g = self.inner.lock().expect("fake mutex");
        if !g.present {
            return Err(ServiceError::NotFound);
        }
        let (first_no, last_no) = match (g.lines.first(), g.lines.last()) {
            (Some(f), Some(l)) => (f.no, l.no),
            _ => (0, 0),
        };
        Ok(BridgeStats {
            rx_lines: 0,
            tx_lines: 0,
            rx_bytes: 0,
            tx_bytes: 0,
            first_no,
            last_no,
            first_ts: String::new(),
            last_ts: String::new(),
            first_epoch: 0,
            last_epoch: 0,
            ring_cap: RING_CAP,
            size: g.lines.len(),
        })
    }

    /// 有界读取计数（评审 P1-1 回归）：/lines、/annotations 不得再调 snapshot。
    fn snapshot(&self, _id: &str) -> Result<Vec<BridgeLine>, ServiceError> {
        let mut g = self.inner.lock().expect("fake mutex");
        g.snapshot_calls += 1;
        let st = g.present;
        let lines = g.lines.clone();
        drop(g);
        if st {
            Ok(lines)
        } else {
            Err(ServiceError::NotFound)
        }
    }

    fn lines_after(&self, _id: &str, no: u64, max: usize) -> Result<Vec<BridgeLine>, ServiceError> {
        let mut g = self.inner.lock().expect("fake mutex");
        g.lines_after_calls.push((no, max));
        if !g.present {
            return Err(ServiceError::NotFound);
        }
        Ok(g.lines
            .iter()
            .filter(|l| l.no > no)
            .take(max)
            .cloned()
            .collect())
    }

    fn line_by_no(&self, _id: &str, no: u64) -> Result<Option<BridgeLine>, ServiceError> {
        let g = self.inner.lock().expect("fake mutex");
        if !g.present {
            return Err(ServiceError::NotFound);
        }
        Ok(g.lines.iter().find(|l| l.no == no).cloned())
    }

    fn last_no(&self, _id: &str) -> Result<u64, ServiceError> {
        let g = self.inner.lock().expect("fake mutex");
        if g.present {
            Ok(g.lines.last().map(|l| l.no).unwrap_or(0))
        } else {
            Err(ServiceError::NotFound)
        }
    }

    fn log_path(&self, _id: &str) -> Result<std::path::PathBuf, ServiceError> {
        Err(ServiceError::Backend("offline".into()))
    }

    fn plot(&self, _id: &str) -> Result<PlotConfig, ServiceError> {
        Err(ServiceError::NotFound)
    }

    fn set_plot(&self, _id: &str, _cfg: PlotConfig) -> Result<(), ServiceError> {
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

    fn set_annotations(&self, _id: &str, _v: Vec<BridgeAnnotation>) -> Result<(), ServiceError> {
        Err(ServiceError::NotFound)
    }

    fn send(&self, _id: &str, _req: SendRequest) -> Result<(), ServiceError> {
        let mut g = self.inner.lock().expect("fake mutex");
        if !g.present {
            return Err(ServiceError::Backend(
                "session not found: no such session".into(),
            ));
        }
        // 立即回包：no = baseline+1（回归点：/exchange 基线先行，快响应不被跳过）
        if let Some(text) = g.reply.clone() {
            let no = g.lines.last().map(|l| l.no).unwrap_or(0) + 1;
            g.lines.push(mk_line(no, Dir::Rx, &text));
        }
        Ok(())
    }

    fn notify_annotations_changed(&self, _session_id: &str, _annotations: &[BridgeAnnotation]) {}

    fn notify_plot_updated(&self, _session_id: &str, _config: &PlotConfig) {}
}

// =============================== 测试基建 ===============================

/// 构造 (router, fake)：token 固定 64 hex，allowSend 按用例切换。
fn app(allow_send: bool, svc: FakeService) -> (axum::Router, Arc<FakeService>) {
    let svc = Arc::new(svc);
    let cfg = Arc::new(RwLock::new(BridgeConfig {
        enabled: true,
        bind: "127.0.0.1".into(),
        port: 8765,
        token: TOKEN.into(),
        allow_send,
    }));
    (router(svc.clone(), cfg), svc)
}

fn get(uri: &str, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::empty()).expect("build GET request")
}

fn post_json(uri: &str, json: &str, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(json.to_string()))
        .expect("build POST request")
}

/// 响应体反序列化为 JSON（错误体即 ApiError 信封 `{"error":{"code","message"}}`）。
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("body is JSON")
}

// =============================== 用例 ===============================

/// 1. /health 匿名可达（不走 token）。
#[tokio::test]
async fn health_is_reachable_without_token() {
    let (app, _svc) = app(false, FakeService::missing());
    let resp = app.oneshot(get("/health", None)).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["ok"], serde_json::json!(true));
    assert_eq!(json["allowSend"], serde_json::json!(false));
}

/// 2. /sessions：无 token 401、错误 token 401、正确 Bearer 200 且含 fake 会话。
#[tokio::test]
async fn sessions_gated_by_bearer_token() {
    let (app, _svc) = app(false, FakeService::live_with_reply());

    // 无 token
    let resp = app
        .clone()
        .oneshot(get("/sessions", None))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 错误 token
    let wrong = "b".repeat(64);
    let resp = app
        .clone()
        .oneshot(get("/sessions", Some(wrong.as_str())))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // 正确 Bearer
    let resp = app
        .oneshot(get("/sessions", Some(TOKEN)))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json[0]["id"], serde_json::json!(SESSION_ID));
    assert_eq!(json[0]["status"], serde_json::json!("connected"));
}

/// 3. allowSend=false 时 /send、/exchange 一律 403（写侧独立门控）。
#[tokio::test]
async fn send_and_exchange_forbidden_when_allow_send_disabled() {
    let (app, _svc) = app(false, FakeService::live_with_reply());
    let bearer = Some(TOKEN);

    let resp = app
        .clone()
        .oneshot(post_json("/sessions/s1/send", r#"{"text":"ping"}"#, bearer))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let resp = app
        .clone()
        .oneshot(post_json(
            "/sessions/s1/exchange",
            r#"{"send":{"text":"ping"},"waitMs":50}"#,
            bearer,
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// 4. /exchange 非法 matcher（re="("）→ 400 + ApiError 信封 code=invalid_regex。
#[tokio::test]
async fn exchange_invalid_matcher_is_400_invalid_regex() {
    let (app, _svc) = app(true, FakeService::live_with_reply());
    let resp = app
        .oneshot(post_json(
            "/sessions/s1/exchange",
            r#"{"send":{"text":"ping"},"match":{"re":"("}}"#,
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], serde_json::json!("invalid_regex"));
    assert!(json["error"]["message"].is_string());
}

/// 5. /exchange 立即响应：send 后首帧即含 no=baseline+1 的 RX 行 → 200 且 response 非 null。
#[tokio::test]
async fn exchange_returns_immediate_response_line() {
    let (app, _svc) = app(true, FakeService::live_with_reply());
    let resp = app
        .oneshot(post_json(
            "/sessions/s1/exchange",
            // 基线 = 7；fake send 立即追加 no=8 的 "ACK"，小 wait_ms 也须首轮命中
            r#"{"send":{"text":"ping"},"waitMs":200}"#,
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["sent"], serde_json::json!(true));
    assert_eq!(json["response"]["no"], serde_json::json!(8));
    assert_eq!(json["response"]["text"], serde_json::json!("ACK"));
}

/// 6. 会话不存在：/exchange → 404 session_not_found；/send → 400 + 发送错误文案。
#[tokio::test]
async fn missing_session_paths() {
    let (app, _svc) = app(true, FakeService::missing());

    let resp = app
        .clone()
        .oneshot(post_json(
            "/sessions/s1/exchange",
            r#"{"send":{"text":"ping"},"waitMs":50}"#,
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = body_json(resp).await;
    assert_eq!(
        json["error"]["code"],
        serde_json::json!("session_not_found")
    );

    let resp = app
        .oneshot(post_json(
            "/sessions/s1/send",
            r#"{"text":"ping"}"#,
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    let text = String::from_utf8(bytes.to_vec()).expect("utf8 body");
    assert!(text.contains("session not found"), "got: {text}");
}

/// 7. 监听运行态错误序列化：`BridgeView{runtime: Error + last_error}` 的 serde 输出
///    须为 `lastError`（camelCase）与小写 state（路由层无对应端点，直接断言 serde 形状）。
#[test]
fn bridge_view_runtime_error_serializes_camel_case_and_lowercase_state() {
    let view = BridgeView {
        config: BridgeConfig::default(),
        runtime: BridgeRuntime {
            state: RuntimeState::Error,
            bound: None,
            last_error: Some("bind 127.0.0.1:8765 failed: permission denied".into()),
        },
    };
    let json = serde_json::to_value(&view).expect("serialize");
    assert_eq!(json["runtime"]["state"], serde_json::json!("error"));
    assert_eq!(
        json["runtime"]["lastError"],
        serde_json::json!("bind 127.0.0.1:8765 failed: permission denied")
    );
    assert_eq!(json["runtime"]["bound"], serde_json::Value::Null);
    // 配置侧同为 camelCase（顺带锁定信封整体形状）
    assert_eq!(json["config"]["allowSend"], serde_json::json!(false));
}

// =============================== /lines 有界读取（评审 P1-1） ===============================

/// 无过滤 + limit：offset/limit 直接下推为单次有界页读，绝不 snapshot。
#[tokio::test]
async fn lines_limit_pushes_down_and_never_snapshots() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    let resp = app
        .oneshot(get("/sessions/s1/lines?limit=10", Some(TOKEN)))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    // 页 = 首 10 行；total/truncated 与原全量物化口径一致
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (1..=10).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(10_000));
    assert_eq!(v["truncated"], serde_json::json!(true));
    assert_eq!(v["firstNo"], serde_json::json!(1));
    assert_eq!(v["lastNo"], serde_json::json!(10_000));
    assert_eq!(v["size"], serde_json::json!(10_000));
    // 有界性：0 次 snapshot；一次 max=10 的 lines_after（百万行文件同样有界）
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0, "无过滤 /lines 不得 snapshot");
    assert_eq!(g.lines_after_calls, vec![(0, 10)]);
}

/// 无过滤 + offset/limit：分页起点按下推游标落位。
#[tokio::test]
async fn lines_offset_limit_pushes_down() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    let resp = app
        .oneshot(get("/sessions/s1/lines?offset=9990&limit=20", Some(TOKEN)))
        .await
        .expect("oneshot");
    let v = body_json(resp).await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (9991..=10_000).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(10_000));
    assert_eq!(v["truncated"], serde_json::json!(false));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0);
}

/// 有过滤：固定页大小流式扫描，只保留命中窗口，绝不 snapshot。
#[tokio::test]
async fn lines_filtered_streams_without_snapshot() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    let resp = app
        .oneshot(get("/sessions/s1/lines?re=hit&limit=3", Some(TOKEN)))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3);
    let nos: Vec<u64> = lines.iter().map(|l| l["no"].as_u64().unwrap()).collect();
    assert_eq!(nos, vec![1, 2, 4]);
    // total = 全部命中数（nos 1..=10000 中非 3 的倍数）
    assert_eq!(v["total"], serde_json::json!(10_000 - 10_000 / 3));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0, "有过滤 /lines 不得 snapshot");
    // 流式页读有界：单次 max ≤ 1000（SCAN_PAGE）；10 页扫完 + 1 次拉空确认
    assert!(g.lines_after_calls.iter().all(|(_, max)| *max <= 1000));
    assert_eq!(g.lines_after_calls.len(), 11);
}

/// 选择模式（last/no/around/since）在无过滤下全部有界。
#[tokio::test]
async fn lines_selection_modes_stay_bounded() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    // last=5 → 最新 5 行
    let v = body_json(
        app.clone()
            .oneshot(get("/sessions/s1/lines?last=5", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (9996..=10_000).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(5));
    // no=42 → 单行
    let v = body_json(
        app.clone()
            .oneshot(get("/sessions/s1/lines?no=42", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    assert_eq!(v["lines"][0]["no"], serde_json::json!(42));
    assert_eq!(v["total"], serde_json::json!(1));
    // around=50&span=2 → 48..=52
    let v = body_json(
        app.clone()
            .oneshot(get("/sessions/s1/lines?around=50&span=2", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (48..=52).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(5));
    // since_no=9998 → 9999、10000
    let v = body_json(
        app.oneshot(get("/sessions/s1/lines?sinceNo=9998", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, vec![9999, 10_000]);
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0);
}

/// `last=N` 先形成最后 N 行的选择集，再在该选择集内应用 offset/limit。
#[tokio::test]
async fn lines_last_paginates_inside_selected_tail() {
    let (app, _svc) = app(false, FakeService::populated(10_000));
    let resp = app
        .oneshot(get(
            "/sessions/s1/lines?last=100&offset=20&limit=10",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (9_921..=9_930).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(100));
    assert_eq!(v["truncated"], serde_json::json!(true));
}

/// 过滤后的 `last=N` 同样先取最后 N 个命中，再在其中分页。
#[tokio::test]
async fn lines_filtered_last_paginates_inside_selected_matches() {
    let (app, _svc) = app(false, FakeService::populated(30));
    let resp = app
        .oneshot(get(
            "/sessions/s1/lines?re=hit&last=10&offset=2&limit=3",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    // 全部命中为非 3 倍数；最后 10 个是 16,17,19,20,22,23,25,26,28,29。
    assert_eq!(nos, vec![19, 20, 22]);
    assert_eq!(v["total"], serde_json::json!(10));
    assert_eq!(v["truncated"], serde_json::json!(true));
}

/// 单行选择也遵守统一分页：offset 越过选择集时页为空但 total 保持 1。
#[tokio::test]
async fn lines_no_offset_returns_empty_page_with_selection_total() {
    let (app, _svc) = app(false, FakeService::populated(100));
    let resp = app
        .oneshot(get(
            "/sessions/s1/lines?no=42&offset=1&limit=10",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["lines"], serde_json::json!([]));
    assert_eq!(v["total"], serde_json::json!(1));
    assert_eq!(v["truncated"], serde_json::json!(false));
}

/// offset 越过范围选择集只清空当前页，不改变 total。
#[tokio::test]
async fn lines_range_offset_past_end_preserves_total() {
    let (app, _svc) = app(false, FakeService::populated(100));
    let resp = app
        .oneshot(get(
            "/sessions/s1/lines?from=10&to=12&offset=9&limit=10",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    let v = body_json(resp).await;
    assert_eq!(v["lines"], serde_json::json!([]));
    assert_eq!(v["total"], serde_json::json!(3));
    assert_eq!(v["truncated"], serde_json::json!(false));
}

/// live ring 已淘汰头部时，around 窗口必须以实际 firstNo 为下界。
#[tokio::test]
async fn lines_around_clamps_to_evicted_ring_head() {
    let svc = FakeService {
        inner: Mutex::new(FakeState {
            present: true,
            lines: (1_000..=1_010)
                .map(|no| mk_line(no, Dir::Rx, &format!("line-{no}")))
                .collect(),
            ..FakeState::default()
        }),
    };
    let (app, _svc) = app(false, svc);
    let resp = app
        .oneshot(get(
            "/sessions/s1/lines?around=1005&span=10&limit=100",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    let v = body_json(resp).await;
    assert_eq!(v["lines"][0]["no"], serde_json::json!(1_000));
    assert_eq!(v["lines"][10]["no"], serde_json::json!(1_010));
    assert_eq!(v["total"], serde_json::json!(11));
}

/// 批注回填按行号单行读取（line_by_no），不再 snapshot。
#[tokio::test]
async fn annotations_backfill_uses_line_by_no() {
    let (app, svc) = app(false, FakeService::populated(100));
    let resp = app
        .oneshot(post_json(
            "/sessions/s1/annotations",
            r#"{"notes":[{"no":7,"note":"check"}]}"#,
            Some(TOKEN),
        ))
        .await
        .expect("oneshot");
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["added"], serde_json::json!(1));
    let note = &v["annotations"][0];
    assert_eq!(note["no"], serde_json::json!(7));
    // ts/text 由后端按行号回填
    assert_eq!(note["text"], serde_json::json!("hit-7"));
    assert_eq!(note["ts"], serde_json::json!("00:00:00.000"));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0, "批注回填不得 snapshot");
}

/// 复审 R-P1-1：选择集分页契约——「先选集、后分页」。
/// last=100&offset=20&limit=10 → 最新 100 行的第 21–30 行（9921..9930），
/// 而非全局末尾窗口。无过滤路径对照。
#[tokio::test]
async fn lines_last_offset_paginates_within_selection() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    let v = body_json(
        app.oneshot(get(
            "/sessions/s1/lines?last=100&offset=20&limit=10",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (9921..=9930).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(100));
    assert_eq!(v["truncated"], serde_json::json!(true));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0);
    // 有界：单次页读恰好 max=10
    assert_eq!(g.lines_after_calls, vec![(9920, 10)]);
}

/// 复审 R-P1-1：`no=X&offset>0` 空页但 total 保留；offset 越过选择集末尾
/// 同样空页保 total（不得重置为 0）。
#[tokio::test]
async fn lines_offset_beyond_selection_keeps_total() {
    let (app, _svc) = app(false, FakeService::populated(10_000));
    // no=42&offset=1 → 选择集 1 行，offset=1 页为空
    let v = body_json(
        app.clone()
            .oneshot(get("/sessions/s1/lines?no=42&offset=1", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    assert_eq!(v["lines"].as_array().unwrap().len(), 0);
    assert_eq!(v["total"], serde_json::json!(1));
    assert_eq!(v["truncated"], serde_json::json!(false));
    // sinceNo=9998&offset=5 → 选择集 2 行（9999、10000），页空
    let v = body_json(
        app.oneshot(get("/sessions/s1/lines?sinceNo=9998&offset=5", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    assert_eq!(v["lines"].as_array().unwrap().len(), 0);
    assert_eq!(v["total"], serde_json::json!(2));
    assert_eq!(v["truncated"], serde_json::json!(false));
}

/// 复审 R-P1-1：around 窗口下界钳到 first_no（ring 头部已淘汰时不高估 total）。
#[tokio::test]
async fn lines_around_window_clamps_to_first_no() {
    // nos 900..999（first_no=900）：around=905&span=10 → 窗口 [895,915] ∩
    // [900,999] = 900..=915，total=16（未钳制会错误地按 21 计）
    let (app, svc) = app(false, FakeService::populated_from(900, 100));
    let v = body_json(
        app.oneshot(get("/sessions/s1/lines?around=905&span=10", Some(TOKEN)))
            .await
            .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    assert_eq!(nos, (900..=915).collect::<Vec<_>>());
    assert_eq!(v["total"], serde_json::json!(16));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0);
}

/// 复审 R-P1-1：过滤路径同契约——re 命中子集的 last+offset 在选择集内分页。
#[tokio::test]
async fn lines_filtered_last_offset_paginates_within_selection() {
    let (app, svc) = app(false, FakeService::populated(10_000));
    let v = body_json(
        app.oneshot(get(
            "/sessions/s1/lines?re=hit&last=100&offset=20&limit=10",
            Some(TOKEN),
        ))
        .await
        .expect("oneshot"),
    )
    .await;
    let nos: Vec<u64> = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["no"].as_u64().unwrap())
        .collect();
    // 选择集 = 最新 100 个 hit（no 非 3 倍数），页 = 其第 21–30 个
    let hits: Vec<u64> = (1..=10_000).filter(|i| i % 3 != 0).collect();
    let sel_first = hits.len() - 100;
    let expected: Vec<u64> = hits[sel_first + 20..sel_first + 30].to_vec();
    assert_eq!(nos, expected);
    assert_eq!(v["total"], serde_json::json!(100));
    let g = svc.inner.lock().expect("fake mutex");
    assert_eq!(g.snapshot_calls, 0);
}
