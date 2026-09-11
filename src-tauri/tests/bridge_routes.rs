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
            }),
        }
    }

    /// 不存在的会话：全部访问面返回 None / 错误。
    fn missing() -> Self {
        Self {
            inner: Mutex::new(FakeState::default()),
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
        Err(ServiceError::NotFound)
    }

    fn snapshot(&self, _id: &str) -> Result<Vec<BridgeLine>, ServiceError> {
        let g = self.inner.lock().expect("fake mutex");
        if g.present {
            Ok(g.lines.clone())
        } else {
            Err(ServiceError::NotFound)
        }
    }

    fn lines_after(&self, _id: &str, no: u64, max: usize) -> Result<Vec<BridgeLine>, ServiceError> {
        let g = self.inner.lock().expect("fake mutex");
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
