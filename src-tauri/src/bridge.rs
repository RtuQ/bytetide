//! REST 分析桥：进程内 axum server，供外部 AI CLI（经 skill + curl）读取/分析串口日志。
//!
//! - 无 MCP：纯 HTTP + Bearer token。
//! - 远程/虚拟机：绑定 `127.0.0.1`（默认）或 `0.0.0.0`（远程可达），URL+token 由用户复制到 AI 机。
//! - 数据源：`serial::manager::PortManager` 的每会话环形缓冲（带原始字节，近期 20000 行）。
//! - 安全：桥默认关；启用需 token；`/send`、`/exchange` 由 `allowSend` 独立门控。

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::{async_runtime, AppHandle, Emitter, Manager};
use tokio::net::TcpListener;

use bytetide_core::serial::manager::{
    BridgeAnnotation, BridgeLine, BridgeStats, MatchHit, PlotConfig, SendMode, SendRequest,
};
use bytetide_core::serial::port::{list_ports, Dir};
use crate::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_LIMIT: usize = 500;
const MAX_LIMIT: usize = 5000;

// =============================== 配置 ===============================

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConfig {
    pub enabled: bool,
    pub bind: String,
    pub port: u16,
    /// 空串 = 未设置令牌（启用时自动生成）。从不持久化为空以外的明文外的形式；LAN 明文可接受。
    pub token: String,
    pub allow_send: bool,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1".into(),
            port: 8765,
            token: String::new(),
            allow_send: false,
        }
    }
}

/// axum 共享状态：AppHandle（取 AppState/PortManager）+ 配置（令牌实时读）。
#[derive(Clone)]
struct BridgeCtx {
    app: AppHandle,
    cfg: Arc<RwLock<BridgeConfig>>,
}

// =============================== 控制器 ===============================

/// 桥生命周期管理（managed state）。`init` 在 setup 中调用一次。
pub struct BridgeController {
    app: AppHandle,
    cfg: Arc<RwLock<BridgeConfig>>,
    task: Mutex<Option<async_runtime::JoinHandle<()>>>,
}

impl BridgeController {
    pub fn init(app: AppHandle) -> Self {
        let cfg = load_config(&app);
        let ctrl = Self {
            app: app.clone(),
            cfg: Arc::new(RwLock::new(cfg)),
            task: Mutex::new(None),
        };
        if ctrl.cfg.read().enabled {
            ctrl.start_server();
        }
        ctrl
    }

    pub fn get_config(&self) -> BridgeConfig {
        self.cfg.read().clone()
    }

    /// 应用补丁；持久化；当 `enabled`/`bind`/`port` 变化时重启服务。返回新配置。
    pub fn set_config(&self, patch: &BridgeConfigPatch) -> BridgeConfig {
        let needs_restart;
        let new_cfg;
        {
            let mut c = self.cfg.write();
            let prev = c.clone();
            if let Some(v) = patch.enabled {
                c.enabled = v;
                if v && c.token.is_empty() {
                    c.token = new_token();
                }
            }
            if let Some(ref v) = patch.bind {
                c.bind = v.clone();
            }
            if let Some(v) = patch.port {
                c.port = v;
            }
            if let Some(v) = patch.allow_send {
                c.allow_send = v;
            }
            if let Some(ref v) = patch.token {
                c.token = v.clone();
            }
            save_config(&self.app, &c);
            new_cfg = c.clone();
            needs_restart = c.enabled != prev.enabled
                || (c.enabled && (c.bind != prev.bind || c.port != prev.port));
        }
        if needs_restart {
            self.restart();
        }
        new_cfg
    }

    /// 重置令牌（无需重启服务：令牌按请求实时读 `cfg`）。
    pub fn regen_token(&self) -> BridgeConfig {
        let t = new_token();
        {
            let mut c = self.cfg.write();
            c.token = t;
            save_config(&self.app, &c);
        }
        self.cfg.read().clone()
    }

    fn restart(&self) {
        self.stop();
        self.start_server();
    }

    fn stop(&self) {
        if let Some(h) = self.task.lock().take() {
            h.abort();
        }
    }

    fn start_server(&self) {
        let cfg = self.cfg.read().clone();
        if !cfg.enabled || cfg.token.is_empty() {
            return;
        }
        let ctx = BridgeCtx {
            app: self.app.clone(),
            cfg: self.cfg.clone(),
        };
        let bind = cfg.bind.clone();
        let port = cfg.port;
        let h = async_runtime::spawn(async move {
            let listener = match TcpListener::bind((bind.as_str(), port)).await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[bridge] bind {bind}:{port} failed: {e}");
                    return;
                }
            };
            if let Err(e) = axum::serve(listener, router(ctx)).await {
                eprintln!("[bridge] serve error: {e}");
            }
        });
        *self.task.lock() = Some(h);
    }
}

/// 前端 `bridge_set_config_cmd` 的补丁输入（全可选）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeConfigPatch {
    pub enabled: Option<bool>,
    pub bind: Option<String>,
    pub port: Option<u16>,
    pub token: Option<String>,
    pub allow_send: Option<bool>,
}

fn config_path(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("bridge.json")
}

fn load_config(app: &AppHandle) -> BridgeConfig {
    let p = config_path(app);
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice::<BridgeConfig>(&b).unwrap_or_default(),
        Err(_) => BridgeConfig::default(),
    }
}

fn save_config(app: &AppHandle, cfg: &BridgeConfig) {
    let p = config_path(app);
    if let Ok(s) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(p, s);
    }
}

/// 生成 32 字符 hex 令牌（splitmix64，种子取纳秒+原子计数）。
fn new_token() -> String {
    static CTR: AtomicU64 = AtomicU64::new(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut s = nanos ^ CTR
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9E3779B97F4A7C15);
    let mut out = String::with_capacity(32);
    for _ in 0..16 {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.push_str(&format!("{:02x}", (s >> 56) as u8));
    }
    out
}

/// 恒定时间比较，避免计时侧信道。
fn ct_eq(a: &str, b: &str) -> bool {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.len() != bb.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in ab.iter().zip(bb.iter()) {
        d |= x ^ y;
    }
    d == 0
}

// =============================== 路由 ===============================

fn router(ctx: BridgeCtx) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/ports", get(ports))
        .route("/sessions", get(sessions))
        .route("/sessions/:id", get(session_detail))
        .route("/sessions/:id/stats", get(stats))
        .route("/sessions/:id/plot-config", get(plot_config).post(plot_config_set))
        .route("/sessions/:id/lines", get(lines))
        .route("/sessions/:id/follow", get(follow))
        .route("/sessions/:id/histogram", get(histogram))
        .route("/sessions/:id/timing", get(timing))
        .route("/sessions/:id/decode", get(decode))
        .route("/sessions/:id/value-hist", get(value_hist))
        .route("/sessions/:id/infer", get(infer))
        .route("/sessions/:id/bookmarks", get(bookmarks))
        .route("/sessions/:id/alerts", get(alerts))
        .route(
            "/sessions/:id/annotations",
            get(annotations_get)
                .post(annotations_post)
                .delete(annotations_delete),
        )
        .route("/sessions/:id/export", get(export_log))
        .route("/sessions/:id/send", post(send))
        .route("/sessions/:id/exchange", post(exchange))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth_mw))
        .with_state(ctx)
}

async fn auth_mw(State(ctx): State<BridgeCtx>, headers: HeaderMap, req: Request, next: Next) -> Response {
    // /health 公开（不回令牌）
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }
    let token = ctx.cfg.read().token.clone();
    if token.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            "bridge token not set (enable the bridge in the app first)".to_string(),
        )
            .into_response();
    }
    let ok = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| ct_eq(t, &token))
        .unwrap_or(false);
    if !ok {
        return (StatusCode::UNAUTHORIZED, "invalid or missing bearer token".to_string()).into_response();
    }
    next.run(req).await
}

// ----- 元信息 -----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    ok: bool,
    version: &'static str,
    ring_cap: usize,
    token_set: bool,
    allow_send: bool,
}

async fn health(State(ctx): State<BridgeCtx>) -> impl IntoResponse {
    let c = ctx.cfg.read();
    Json(Health {
        ok: true,
        version: VERSION,
        ring_cap: bytetide_core::serial::manager::RING_CAP,
        token_set: !c.token.is_empty(),
        allow_send: c.allow_send,
    })
}

async fn ports() -> impl IntoResponse {
    Json(list_ports())
}

async fn sessions(State(ctx): State<BridgeCtx>) -> impl IntoResponse {
    Json(ctx.app.state::<AppState>().manager.bridge_list())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDetail {
    id: String,
    config: serde_json::Value,
    status: String,
    stats: Option<BridgeStats>,
    /// 会话落盘日志完整路径（`/export` 即流式返回该文件；离线会话为来源文件）。
    log_path: Option<String>,
}

async fn session_detail(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    let snap = ctx
        .app
        .state::<AppState>()
        .manager
        .bridge_list()
        .into_iter()
        .find(|s| s.id == id);
    match snap {
        Some(s) => {
            let stats = ctx.app.state::<AppState>().manager.bridge_stats(&id);
            let log_path = ctx
                .app
                .state::<AppState>()
                .manager
                .session_log_path(&id)
                .ok();
            Json(SessionDetail {
                id: s.id,
                config: serde_json::to_value(&s.config).unwrap_or_default(),
                status: s.status,
                stats,
                log_path,
            })
            .into_response()
        }
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

async fn stats(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.app.state::<AppState>().manager.bridge_stats(&id) {
        Some(s) => Json(s).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

async fn plot_config(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.app.state::<AppState>().manager.bridge_plot(&id) {
        Some(c) => Json(c).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

/// 桥写回绘图文法后通知前端实时采纳的事件载荷。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlotUpdatedPayload {
    session_id: String,
    config: PlotConfig,
}

/// 写回绘图文法（整包替换——先 GET 当前值再改字段）。后端留存供 `/decode` 复用，
/// 同时发 `bridge-plot-updated` 事件让应用界面即时采纳。不受 allowSend 门控（不触串口）。
async fn plot_config_set(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(mut cfg): Json<PlotConfig>,
) -> Response {
    if let Err(e) = sanitize_plot_config(&mut cfg) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    if !ctx
        .app
        .state::<AppState>()
        .manager
        .bridge_set_plot(&id, cfg.clone())
    {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    }
    let _ = ctx.app.emit(
        "bridge-plot-updated",
        PlotUpdatedPayload {
            session_id: id,
            config: cfg.clone(),
        },
    );
    Json(cfg).into_response()
}

/// 用户在应用里标记的书签（前端推送的只读镜像；`no` 为 UI 行号）。
async fn bookmarks(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.app.state::<AppState>().manager.bridge_bookmarks(&id) {
        Some(b) => Json(b).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

/// 告警历史（前端推送的只读镜像，环形 100 条、新的在前；`no` 为 UI 行号）。
async fn alerts(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.app.state::<AppState>().manager.bridge_alerts(&id) {
        Some(a) => Json(a).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

// ----- AI 批注（REST 写入 → 界面实时可见，与书签的"人标给 AI"互为对称） -----

/// 每会话批注容量上限（超出丢最旧）。
const ANNOTATION_CAP: usize = 200;
/// 批注携带的行文本摘录长度。
const ANNOTATION_CLIP: usize = 200;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn clip_str(s: &str) -> String {
    s.chars().take(ANNOTATION_CLIP).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationInput {
    no: u64,
    note: String,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationsBody {
    notes: Vec<AnnotationInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationsDeleteParams {
    /// 缺省时清空全部。
    id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationsPage {
    added: usize,
    annotations: Vec<BridgeAnnotation>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnnotationsPayload {
    session_id: String,
    annotations: Vec<BridgeAnnotation>,
}

/// 批注变化后推送到前端界面（日志行标记 + 侧栏面板实时刷新）。
fn emit_annotations(ctx: &BridgeCtx, id: &str, annotations: Vec<BridgeAnnotation>) {
    let _ = ctx.app.emit(
        "bridge-annotations-updated",
        AnnotationsPayload {
            session_id: id.to_string(),
            annotations,
        },
    );
}

/// 合并 AI 批注：按 (no, note) 去重（重复提交幂等），超出容量丢最旧。
/// 返回 (合并后列表, 实际新增条数)。
fn merge_annotations(
    mut existing: Vec<BridgeAnnotation>,
    incoming: Vec<BridgeAnnotation>,
    cap: usize,
) -> (Vec<BridgeAnnotation>, usize) {
    let mut added = 0usize;
    for c in incoming {
        if existing.iter().any(|e| e.no == c.no && e.note == c.note) {
            continue;
        }
        existing.push(c);
        added += 1;
    }
    if existing.len() > cap {
        let overflow = existing.len() - cap;
        existing.drain(..overflow);
    }
    (existing, added)
}

/// 列出当前批注。
async fn annotations_get(State(ctx): State<BridgeCtx>, Path(id): Path<String>) -> Response {
    match ctx.app.state::<AppState>().manager.bridge_annotations(&id) {
        Some(a) => Json(a).into_response(),
        None => (StatusCode::NOT_FOUND, "session not found").into_response(),
    }
}

/// 新增批注：`no` 为桥行号；`ts`/`text` 缺省时后端从 ring 中按 `no` 回填。
/// 按 (no, note) 幂等，返回完整列表与新增数；任何变化实时推送到界面。
async fn annotations_post(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(body): Json<AnnotationsBody>,
) -> Response {
    if body.notes.is_empty() {
        return (StatusCode::BAD_REQUEST, "notes must not be empty").into_response();
    }
    let manager = &ctx.app.state::<AppState>().manager;
    // 行仍在缓冲中时回填 ts/text，AI 只需给 no + note
    let snap = manager.bridge_snapshot(&id);
    let Some(snap) = snap else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    let now = now_ms();
    let mut candidates: Vec<BridgeAnnotation> = Vec::with_capacity(body.notes.len());
    for (i, n) in body.notes.into_iter().enumerate() {
        let note = n.note.trim().to_string();
        if note.is_empty() {
            return (StatusCode::BAD_REQUEST, "note must not be empty").into_response();
        }
        if n.no == 0 {
            return (StatusCode::BAD_REQUEST, "no must be >= 1").into_response();
        }
        let (ts, text) = match snap.iter().find(|l| l.no == n.no) {
            Some(l) => (l.ts.clone(), clip_str(&l.text)),
            None => (n.ts, clip_str(&n.text)),
        };
        candidates.push(BridgeAnnotation {
            id: format!("an{now:x}-{i}"),
            no: n.no,
            ts,
            text,
            note,
            at: now,
        });
    }
    let existing = manager.bridge_annotations(&id).unwrap_or_default();
    let (all, added) = merge_annotations(existing, candidates, ANNOTATION_CAP);
    if added > 0 {
        manager.bridge_set_annotations(&id, all.clone());
        emit_annotations(&ctx, &id, all.clone());
    }
    Json(AnnotationsPage {
        added,
        annotations: all,
    })
    .into_response()
}

/// 删除批注：带 `?id=` 删单条，不带则清空全部；返回剩余列表。
async fn annotations_delete(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<AnnotationsDeleteParams>,
) -> Response {
    let manager = &ctx.app.state::<AppState>().manager;
    let Some(existing) = manager.bridge_annotations(&id) else {
        return (StatusCode::NOT_FOUND, "session not found").into_response();
    };
    let remaining: Vec<BridgeAnnotation> = match &p.id {
        Some(rid) => existing.into_iter().filter(|a| &a.id != rid).collect(),
        None => vec![],
    };
    manager.bridge_set_annotations(&id, remaining.clone());
    emit_annotations(&ctx, &id, remaining.clone());
    Json(remaining).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportParams {
    /// 只返回 `{ path, sizeBytes, missing }` 元信息而不拉流；宽松取值 1/true/yes/on。
    info: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportInfo {
    path: String,
    size_bytes: u64,
    missing: bool,
}

/// 全量历史导出：流式返回会话落盘日志（追加写 TSV：`ts<TAB>dir<TAB>text`，
/// 含 ring 已淘汰的行、无原始字节；`clearLog` 会截断该文件）。离线会话返回其来源文件。
async fn export_log(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<ExportParams>,
) -> Response {
    let path = match ctx.app.state::<AppState>().manager.session_log_path(&id) {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
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
    let stream = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024);
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
    if cleaned.len() % 2 != 0 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{name} must be hex pairs, got {:?}", field));
    }
    *field = cleaned.to_ascii_uppercase();
    Ok(())
}

// =============================== 过滤 ===============================

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FilterFields {
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

struct FilterSpec {
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
    fn is_noop(&self) -> bool {
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

fn build_filter(f: &FilterFields) -> Result<FilterSpec, (StatusCode, String)> {
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
        Some(p) if !p.is_empty() => Some(Regex::new(p).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("invalid regex: {e}"),
            )
        })?),
        _ => None,
    };
    let qs = f
        .q
        .as_deref()
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
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

fn parse_hex(s: &str) -> Vec<u8> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..cleaned.len())
        .step_by(2)
        .filter_map(|i| cleaned.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

/// `AA55????` -> `vec![0xAA,0x55, Any,Any,Any,Any]`。
fn parse_mask(s: &str) -> Vec<Option<u8>> {
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
fn line_bytes(l: &BridgeLine) -> Cow<'_, [u8]> {
    match &l.bytes {
        Some(b) => Cow::Borrowed(b.as_slice()),
        None => Cow::Owned(l.text.as_bytes().to_vec()),
    }
}

/// 返回 `Some(Some(hit))`=通过且带命中；`Some(None)`=通过无命中；`None`=拒绝。
fn apply_filter(l: &BridgeLine, f: &FilterSpec) -> Option<Option<MatchHit>> {
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
        match str_find(&l.text, q, f.ci) {
            Some(off) => {
                if hit.is_none() {
                    hit = Some(MatchHit {
                        offset: off as u64,
                        length: q.len() as u64,
                        field: "text".into(),
                    });
                }
            }
            None => return None,
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
        match bytes_find(&b, &f.hex) {
            Some(off) => {
                if hit.is_none() {
                    hit = Some(MatchHit {
                        offset: off as u64,
                        length: f.hex.len() as u64,
                        field: "bytes".into(),
                    });
                }
            }
            None => return None,
        }
    }
    if !f.mask.is_empty() {
        let b = line_bytes(l);
        match mask_find(&b, &f.mask) {
            Some(off) => {
                if hit.is_none() {
                    hit = Some(MatchHit {
                        offset: off as u64,
                        length: f.mask.len() as u64,
                        field: "bytes".into(),
                    });
                }
            }
            None => return None,
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

/// 过滤快照，返回带（可选）命中信息的 owned 行（保留原序）。
fn filtered(ctx: &BridgeCtx, id: &str, f: &FilterSpec) -> Option<Vec<BridgeLine>> {
    let snap = ctx.app.state::<AppState>().manager.bridge_snapshot(id)?;
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

// =============================== /lines ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinesParams {
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
struct LinesPage {
    lines: Vec<BridgeLine>,
    total: usize,
    first_no: u64,
    last_no: u64,
    size: usize,
    truncated: bool,
}

async fn lines(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<LinesParams>,
) -> Response {
    let f = match build_filter(&p.filter) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    let snap = match ctx.app.state::<AppState>().manager.bridge_snapshot(&id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
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
    let page: Vec<BridgeLine> = selected.into_iter().skip(offset).take(limit).cloned().collect();

    let fmt = p.format.as_deref();
    if fmt == Some("csv") || fmt == Some("tsv") {
        let sep = if fmt == Some("csv") { ',' } else { '\t' };
        let mut s = format!("no{sep}ts{sep}dir{sep}text{sep}bytes{sep}epochMillis\n");
        for l in &page {
            let bytes = l
                .bytes
                .as_ref()
                .map(|b| b.iter().map(|x| format!("{:02X}", x)).collect::<Vec<_>>().join(" "))
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
struct FollowParams {
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
struct FollowPage {
    lines: Vec<BridgeLine>,
    last_no: u64,
    timed_out: bool,
    truncated: bool,
}

async fn follow(
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
    let mut scanned = since;
    loop {
        if let Some((lines, high)) = ctx.app.state::<AppState>().manager.bridge_follow(&id, scanned) {
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
        } else {
            return (StatusCode::NOT_FOUND, "session not found").into_response();
        }
        if tokio::time::Instant::now() >= deadline {
            let last_no = ctx.app.state::<AppState>().manager.bridge_follow(&id, scanned).map(|(_, n)| n).unwrap_or(0);
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
struct HistParams {
    #[serde(flatten)]
    filter: FilterFields,
    bucket: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistBucket {
    bucket_start: u64,
    count: u64,
}

async fn histogram(
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
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    let bucket = p.bucket.unwrap_or(1000).max(1);
    let mut map: BTreeMap<u64, u64> = BTreeMap::new();
    for l in &filt {
        let b = (l.epoch_millis / bucket) * bucket;
        *map.entry(b).or_insert(0) += 1;
    }
    let out: Vec<HistBucket> = map
        .into_iter()
        .map(|(bucket_start, count)| HistBucket { bucket_start, count })
        .collect();
    Json(out).into_response()
}

// =============================== /timing ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimingParams {
    #[serde(flatten)]
    filter: FilterFields,
    gap_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Gap {
    from_no: u64,
    to_no: u64,
    duration_ms: u64,
    from_ts: String,
    to_ts: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimingPage {
    count: usize,
    min_gap: u64,
    max_gap: u64,
    avg_gap: u64,
    p95_gap: u64,
    gaps: Vec<Gap>,
}

async fn timing(
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
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
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
struct DecodeParams {
    #[serde(flatten)]
    filter: FilterFields,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodeFrame {
    no: u64,
    idx: u32,
    values: Vec<f64>,
    raw_hex: String,
    ts: String,
    epoch_millis: u64,
    valid: bool,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DecodePage {
    frames: Vec<DecodeFrame>,
    frame_count: u32,
    last_error: String,
    scanned: usize,
}

async fn decode(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<DecodeParams>,
) -> Response {
    // 基础文法取已存 PlotConfig，参数覆盖
    let mut base = ctx.app.state::<AppState>().manager.bridge_plot(&id).unwrap_or_default();
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
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    // from/to 在 no 上裁剪
    let filt: Vec<BridgeLine> = filt
        .into_iter()
        .filter(|l| {
            p.from.map_or(true, |x| l.no >= x) && p.to.map_or(true, |x| l.no <= x)
        })
        .collect();

    let limit = p.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let page = parse_frames(&base, &filt, limit);
    Json(page).into_response()
}

fn to_hex(bytes: &[u8]) -> String {
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

fn parse_value(bytes: &[u8], off: usize, len: usize, endian: &str, signed: bool) -> f64 {
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
fn parse_frames(cfg: &PlotConfig, lines: &[BridgeLine], limit: usize) -> DecodePage {
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
                &mut frames, idx, &bytes, &ranges, head_len, channels, bpc, cfg, i, frame_end,
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
                &mut frames, idx, &bytes, &ranges, 0, channels, bpc, cfg, frame_start, frame_end,
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

// =============================== /value-hist ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValueHistParams {
    #[serde(flatten)]
    filter: FilterFields,
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
struct ValueDist {
    value: f64,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ValueHistPage {
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
    dist.sort_by(|a, b| b.1.cmp(&a.1));
    ValueStats {
        samples,
        distinct: counts.len(),
        min: if samples > 0 { min } else { 0.0 },
        max: if samples > 0 { max } else { 0.0 },
        mean: if samples > 0 { sum / samples as f64 } else { 0.0 },
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

async fn value_hist(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Query(p): Query<ValueHistParams>,
) -> Response {
    let mut base = ctx.app.state::<AppState>().manager.bridge_plot(&id).unwrap_or_default();
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
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
    };
    let filt: Vec<BridgeLine> = filt
        .into_iter()
        .filter(|l| {
            p.from.map_or(true, |x| l.no >= x) && p.to.map_or(true, |x| l.no <= x)
        })
        .collect();

    let page = parse_frames(&base, &filt, MAX_LIMIT);
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
struct InferParams {
    #[serde(flatten)]
    filter: FilterFields,
    min_repeat: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HexCount {
    hex: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChecksumCount {
    kind: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InferPage {
    heads: Vec<HexCount>,
    tails: Vec<HexCount>,
    checksums: Vec<ChecksumCount>,
    suggested_frame_len: Option<usize>,
}

async fn infer(
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
        None => return (StatusCode::NOT_FOUND, "session not found").into_response(),
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
        let b = line_bytes(l);
        if b.is_empty() {
            continue;
        }
        total += 1;
        *heads1.entry(b[0]).or_insert(0) += 1;
        if b.len() >= 2 {
            *heads2.entry(((b[0] as u16) << 8) | b[1] as u16).or_insert(0) += 1;
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
    heads.sort_by(|a, b| b.count.cmp(&a.count));
    heads.truncate(8);

    let mut tails: Vec<HexCount> = tails1
        .iter()
        .filter(|(_, c)| **c >= min_rep)
        .map(|(t, c)| HexCount {
            hex: format!("{:02X}", t),
            count: *c,
        })
        .collect();
    tails.sort_by(|a, b| b.count.cmp(&a.count));
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

    let suggested_frame_len = lens
        .iter()
        .max_by_key(|(_, c)| **c)
        .and_then(|(len, c)| {
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

// =============================== /send & /exchange ===============================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendBody {
    mode: Option<String>,
    text: String,
}

async fn send(
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
    match ctx
        .app
        .state::<AppState>()
        .manager
        .send(&id, SendRequest { mode, text: body.text })
    {
        Ok(()) => Json(serde_json::json!({})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeMatch {
    re: Option<String>,
    hex: Option<String>,
    dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeBody {
    send: SendBody,
    wait_ms: Option<u64>,
    r#match: Option<ExchangeMatch>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExchangePage {
    sent: bool,
    response: Option<BridgeLine>,
    waited_ms: u64,
}

async fn exchange(
    State(ctx): State<BridgeCtx>,
    Path(id): Path<String>,
    Json(body): Json<ExchangeBody>,
) -> Response {
    if !ctx.cfg.read().allow_send {
        return (
            StatusCode::FORBIDDEN,
            "allowSend is disabled; enable it in the bridge panel".to_string(),
        )
            .into_response();
    }
    // 发送
    let mode = match body.send.mode.as_deref() {
        Some("hex") => SendMode::Hex,
        _ => SendMode::Ascii,
    };
    if let Err(e) = ctx
        .app
        .state::<AppState>()
        .manager
        .send(&id, SendRequest { mode, text: body.send.text })
    {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }
    // 捕获匹配
    let re = body.r#match.as_ref().and_then(|m| {
        m.re.as_deref().and_then(|p| {
            if p.is_empty() {
                None
            } else {
                Regex::new(p).ok()
            }
        })
    });
    let hex = body
        .r#match
        .as_ref()
        .map(|m| parse_hex(m.hex.as_deref().unwrap_or("")))
        .unwrap_or_default();
    let dir = body
        .r#match
        .as_ref()
        .and_then(|m| match m.dir.as_deref() {
            Some("tx") => Some(Dir::Tx),
            _ => Some(Dir::Rx),
        })
        .unwrap_or(Dir::Rx);

    let wait = std::time::Duration::from_millis(body.wait_ms.unwrap_or(2000).min(30_000));
    let deadline = tokio::time::Instant::now() + wait;
    let mut baseline = ctx.app.state::<AppState>().manager.bridge_follow(&id, 0).map(|(_, n)| n).unwrap_or(0);
    let start = std::time::Instant::now();
    loop {
        if let Some((lines, last_no)) = ctx.app.state::<AppState>().manager.bridge_follow(&id, baseline) {
            for l in &lines {
                if l.dir != dir {
                    continue;
                }
                let hit_re = re.as_ref().map(|r| r.is_match(&l.text)).unwrap_or(true);
                let hit_hex = if hex.is_empty() {
                    true
                } else {
                    bytes_find(&line_bytes(l), &hex).is_some()
                };
                if hit_re && hit_hex {
                    return Json(ExchangePage {
                        sent: true,
                        response: Some(l.clone()),
                        waited_ms: start.elapsed().as_millis() as u64,
                    })
                    .into_response();
                }
            }
            baseline = last_no;
        }
        if tokio::time::Instant::now() >= deadline {
            return Json(ExchangePage {
                sent: true,
                response: None,
                waited_ms: start.elapsed().as_millis() as u64,
            })
            .into_response();
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

#[cfg(test)]
mod tests {
    //! 纯函数单测：解析/过滤/帧解码的黄金用例与边界。
    //! 仅测私有纯函数（同模块可见），不触网/不经 axum/不建 manager。
    use super::*;

    fn mk_line(no: u64, dir: Dir, text: &str, bytes: Option<Vec<u8>>, epoch: u64) -> BridgeLine {
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

    fn mk_plot() -> PlotConfig {
        PlotConfig::default()
    }

    // ---------------- parse_frames：三个 golden case + 边界 ----------------

    #[test]
    fn parse_frames_binary_head_two_channels_big_unsigned() {
        // AA 55 | 01 00 | 02 00   -> ch1=0x0100=256, ch2=0x0200=512
        let mut cfg = mk_plot();
        cfg.source = "binary".into();
        cfg.frame_head = "AA55".into();
        cfg.checksum = "none".into();
        cfg.channels = 2;
        cfg.bytes_per_channel = 2;
        cfg.endian = "big".into();
        cfg.signed = false;
        let line = mk_line(1, Dir::Rx, "", Some(vec![0xAA, 0x55, 0x01, 0x00, 0x02, 0x00]), 1000);
        let page = parse_frames(&cfg, &[line], 500);
        assert_eq!(page.frame_count, 1);
        assert_eq!(page.frames.len(), 1);
        assert_eq!(page.frames[0].values, vec![256.0, 512.0]);
        assert_eq!(page.frames[0].raw_hex, "AA 55 01 00 02 00");
        assert!(page.last_error.is_empty());
    }

    #[test]
    fn parse_frames_binary_head_signed_sum_checksum() {
        // FF | 80 | 80   head=FF, data=0x80(signed -> -128), cs=sum(0x80)=0x80
        let mut cfg = mk_plot();
        cfg.source = "binary".into();
        cfg.frame_head = "FF".into();
        cfg.checksum = "sum".into();
        cfg.channels = 1;
        cfg.bytes_per_channel = 1;
        cfg.endian = "big".into();
        cfg.signed = true;
        let line = mk_line(1, Dir::Rx, "", Some(vec![0xFF, 0x80, 0x80]), 1000);
        let page = parse_frames(&cfg, &[line], 500);
        assert_eq!(page.frame_count, 1);
        assert_eq!(page.frames[0].values, vec![-128.0]);
        assert_eq!(page.frames[0].raw_hex, "FF 80 80");
        assert!(page.last_error.is_empty());

        // 校验错 -> 拒收，0 帧，last_error 记录失配
        let bad = mk_line(2, Dir::Rx, "", Some(vec![0xFF, 0x80, 0x00]), 2000);
        let p2 = parse_frames(&cfg, &[bad], 500);
        assert_eq!(p2.frame_count, 0);
        assert!(p2.frames.is_empty());
        assert!(p2.last_error.contains("checksum mismatch"));
    }

    #[test]
    fn parse_frames_ascii_hex_tail_little() {
        // 文本 "AA550DBB660D" 经 ascii-hex -> [AA,55,0D,BB,66,0D]
        // tail=0D，2ch×1B 小端无校验 -> 帧1[170,85] 帧2[187,102]
        let mut cfg = mk_plot();
        cfg.source = "ascii-hex".into();
        cfg.frame_tail = "0D".into();
        cfg.checksum = "none".into();
        cfg.channels = 2;
        cfg.bytes_per_channel = 1;
        cfg.endian = "little".into();
        cfg.signed = false;
        let line = mk_line(7, Dir::Rx, "AA550DBB660D", None, 3000);
        let page = parse_frames(&cfg, &[line], 500);
        assert_eq!(page.frame_count, 2);
        assert_eq!(page.frames[0].values, vec![170.0, 85.0]);
        assert_eq!(page.frames[0].raw_hex, "AA 55 0D");
        assert_eq!(page.frames[1].values, vec![187.0, 102.0]);
        assert_eq!(page.frames[1].raw_hex, "BB 66 0D");
    }

    #[test]
    fn parse_frames_head_mismatch_advances_one_byte() {
        // 前导垃圾 00，随后真帧 AA 55 01 00 02 00 -> 扫描失配按 1 字节前进
        let mut cfg = mk_plot();
        cfg.frame_head = "AA55".into();
        cfg.channels = 2;
        cfg.bytes_per_channel = 2;
        cfg.endian = "big".into();
        let line = mk_line(1, Dir::Rx, "", Some(vec![0x00, 0xAA, 0x55, 0x01, 0x00, 0x02, 0x00]), 1000);
        let page = parse_frames(&cfg, &[line], 500);
        assert_eq!(page.frame_count, 1);
        assert_eq!(page.frames[0].raw_hex, "AA 55 01 00 02 00");
    }

    #[test]
    fn parse_frames_limit_truncates() {
        let mut cfg = mk_plot();
        cfg.frame_head = "AA".into();
        cfg.channels = 1;
        cfg.bytes_per_channel = 1;
        cfg.endian = "big".into();
        // AA 01 AA 02 AA 03 -> 3 帧（head=AA,data=1B）
        let mk = || mk_line(1, Dir::Rx, "", Some(vec![0xAA, 0x01, 0xAA, 0x02, 0xAA, 0x03]), 0);
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
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "x", None, 0), &f), None));
        assert!(matches!(apply_filter(&mk_line(2, Dir::Tx, "x", None, 0), &f), Some(None)));
    }

    #[test]
    fn apply_filter_q_multiple_all_must_match() {
        let f = spec(None, vec!["OK", "ERR"], vec![], vec![], None, None, None);
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "OK ERR foo", None, 0), &f), Some(Some(_))));
        assert!(matches!(apply_filter(&mk_line(2, Dir::Rx, "OK foo", None, 0), &f), None));
    }

    #[test]
    fn apply_filter_hex_substring() {
        let f = spec(None, vec![], vec![0xAA, 0x55], vec![], None, None, None);
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "", Some(vec![0x00, 0xAA, 0x55, 0x99]), 0), &f), Some(Some(_))));
        assert!(matches!(apply_filter(&mk_line(2, Dir::Rx, "", Some(vec![0xAA, 0x99]), 0), &f), None));
    }

    #[test]
    fn apply_filter_mask_wildcard() {
        let f = spec(None, vec![], vec![], vec![Some(0xAA), None], None, None, None); // AA??
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "", Some(vec![0xAA, 0x12]), 0), &f), Some(Some(_))));
        assert!(matches!(apply_filter(&mk_line(2, Dir::Rx, "", Some(vec![0x55, 0x12]), 0), &f), None));
    }

    #[test]
    fn apply_filter_exclude_negative() {
        let f = spec(None, vec!["DATA"], vec![], vec![], Some("NOISE"), None, None);
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "DATA NOISE", None, 0), &f), None));
        assert!(matches!(apply_filter(&mk_line(2, Dir::Rx, "DATA here", None, 0), &f), Some(Some(_))));
    }

    #[test]
    fn apply_filter_time_window() {
        let f = spec(None, vec![], vec![], vec![], None, Some(100), Some(200));
        assert!(matches!(apply_filter(&mk_line(1, Dir::Rx, "x", None, 50), &f), None)); // before
        assert!(matches!(apply_filter(&mk_line(2, Dir::Rx, "x", None, 150), &f), Some(None))); // inside
        assert!(matches!(apply_filter(&mk_line(3, Dir::Rx, "x", None, 250), &f), None)); // after
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

    // ---------------- ct_eq ----------------

    #[test]
    fn ct_eq_equal_and_differ() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd")); // 前缀同末位异
        assert!(!ct_eq("abc", "ab")); // 长度不等早退
        assert!(!ct_eq("", "a"));
        assert!(ct_eq("", ""));
    }

    // ---------------- line_bytes ----------------

    #[test]
    fn line_bytes_borrows_raw_or_falls_back_to_text() {
        let with_bytes = mk_line(1, Dir::Rx, "x", Some(vec![0xAA, 0x55]), 0);
        assert_eq!(&*line_bytes(&with_bytes), &[0xAA, 0x55]);
        let no_bytes = mk_line(2, Dir::Rx, "abc", None, 0);
        assert_eq!(&*line_bytes(&no_bytes), b"abc");
    }

    // ---------------- parse_value ----------------

    #[test]
    fn parse_value_endian_and_signed_boundary() {
        assert_eq!(parse_value(&[0x01, 0x00], 0, 2, "big", false), 256.0);
        assert_eq!(parse_value(&[0x01, 0x00], 0, 2, "little", false), 1.0);
        assert_eq!(parse_value(&[0x80], 0, 1, "big", true), -128.0);
        assert_eq!(parse_value(&[0x7F], 0, 1, "big", true), 127.0);
        assert_eq!(parse_value(&[0x80, 0x00], 0, 2, "big", true), -32768.0);
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

    // ---------------- sanitize_plot_config ----------------

    #[test]
    fn sanitize_plot_config_normalizes_and_clamps() {
        let mut cfg = PlotConfig::default();
        cfg.source = "ASCII-HEX".into();
        cfg.checksum = " Xor ".into();
        cfg.endian = "Little".into();
        cfg.frame_head = "aa 55".into();
        cfg.channels = 99;
        cfg.max_points = 0;
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
        let mut bad = PlotConfig::default();
        bad.source = "raw".into();
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig::default();
        bad.checksum = "crc8".into();
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig::default();
        bad.endian = "middle".into();
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig::default();
        bad.frame_tail = "AA5".into(); // 奇数长度
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig::default();
        bad.frame_head = "ZZ".into(); // 非 hex
        assert!(sanitize_plot_config(&mut bad).is_err());

        let mut bad = PlotConfig::default();
        bad.bytes_per_channel = 3;
        assert!(sanitize_plot_config(&mut bad).is_err());

        // 空 head/tail 合法（无帧头模式）
        let mut ok = PlotConfig::default();
        ok.frame_head.clear();
        ok.frame_tail.clear();
        assert!(sanitize_plot_config(&mut ok).is_ok());
    }

    // ---------------- merge_annotations ----------------

    fn mk_note(id: &str, no: u64, note: &str) -> BridgeAnnotation {
        BridgeAnnotation {
            id: id.into(),
            no,
            ts: String::new(),
            text: String::new(),
            note: note.into(),
            at: 0,
        }
    }

    #[test]
    fn merge_annotations_dedupes_by_no_and_note() {
        let existing = vec![mk_note("a", 5, "first")];
        // 同 (no, note) 重复提交幂等；同 no 不同 note 保留
        let (all, added) = merge_annotations(
            existing,
            vec![mk_note("b", 5, "first"), mk_note("c", 5, "second")],
            200,
        );
        assert_eq!(added, 1);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "a"); // 旧的在前、不被替换
        assert_eq!(all[1].note, "second");
    }

    #[test]
    fn merge_annotations_caps_and_drops_oldest() {
        let existing: Vec<BridgeAnnotation> = (0..3).map(|i| mk_note(&format!("o{i}"), i, "n")).collect();
        let incoming: Vec<BridgeAnnotation> = (10..14).map(|i| mk_note(&format!("n{i}"), i, "n")).collect();
        let (all, added) = merge_annotations(existing, incoming, 5);
        assert_eq!(added, 4);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].id, "o2"); // 7 条裁到 5：最旧的 o0、o1 被丢弃
        assert_eq!(all.last().unwrap().id, "n13");
    }

    // ---------------- /follow 服务端过滤 ----------------

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
        let spec = build_filter(&mk_ff(Some("error"), None, None)).ok().unwrap();
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
        let spec = build_filter(&mk_ff(Some("(?i)error"), None, None)).ok().unwrap();
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
        let err = build_filter(&mk_ff(Some("(unclosed"), None, None)).err().unwrap();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("invalid regex"));
    }
}
