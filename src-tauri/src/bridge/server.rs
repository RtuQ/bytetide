//! 桥控制器：配置持久化、监听生命周期（同步 bind）、运行态视图与 router 组装。
//!
//! - 启动/替换均**同步 bind**——命令返回前即可知道成败，绑定失败写 `runtime`
//!   （Error + last_error）供 UI 展示，不再仅 eprintln。
//! - `BridgeController` 是 managed state；`init` 在 setup 中调用一次。
//! - router 组装在此：会话访问面（`Arc<dyn BridgeService>`）+ 配置两参注入，
//!   pub 供集成测试（tests/bridge_routes.rs）脱离 Tauri 直测路由层。

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{async_runtime, Manager};
use tokio::net::TcpListener;

use super::auth::auth_mw;
use super::routes::BridgeCtx;
use super::service::BridgeService;

// =============================== 配置 ===============================

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// 桥配置层错误：随机源失败 / 非法绑定地址 / 非法端口。
/// Display 文本面向 UI 展示，**绝不包含 token 值**。
#[derive(Debug)]
pub enum BridgeConfigError {
    Randomness(String),
    InvalidBind(String),
    InvalidPort(u16),
}

impl std::fmt::Display for BridgeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Randomness(e) => write!(f, "secure randomness unavailable: {e}"),
            Self::InvalidBind(s) => write!(f, "invalid bind address: {s}"),
            Self::InvalidPort(p) => write!(f, "invalid port: {p} (must be 1-65535)"),
        }
    }
}

/// 桥服务运行态（UI 可见）。
/// `Starting` 为预留态：当前启动是命令内的同步 bind，返回前即已 Running/Error，
/// 异步启动感知留待后续；前端按四态渲染。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeState {
    Disabled,
    #[allow(dead_code)]
    Starting,
    Running,
    Error,
}

/// 运行态快照：`bound` = 实际监听地址（ip:port）；`last_error` 仅描述故障，不含令牌。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeRuntime {
    pub state: RuntimeState,
    pub bound: Option<String>,
    pub last_error: Option<String>,
}

impl Default for BridgeRuntime {
    fn default() -> Self {
        Self {
            state: RuntimeState::Disabled,
            bound: None,
            last_error: None,
        }
    }
}

/// bridge_* 命令完整响应：配置 + 运行态（UI 无需自行推断是否真的在监听）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeView {
    pub config: BridgeConfig,
    pub runtime: BridgeRuntime,
}

/// 纯 hex 编码：字节 → 小写 hex（独立出来便于测格式）。
fn token_from(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 从注入的随机源生成令牌：失败向上传播（`Randomness`），**绝不回退时间/计数器**。
fn generate_token(
    rng: impl FnOnce(&mut [u8]) -> Result<(), String>,
) -> Result<String, BridgeConfigError> {
    let mut buf = [0u8; 32];
    rng(&mut buf).map_err(BridgeConfigError::Randomness)?;
    Ok(token_from(&buf))
}

/// OS CSPRNG（getrandom）32 字节 → 64 位小写 hex 令牌。
fn new_token() -> Result<String, BridgeConfigError> {
    generate_token(|buf| getrandom::fill(buf).map_err(|e| e.to_string()))
}

/// set_config 入口校验：端口 0 拒绝；bind 须为 IP 字面量或合法主机名字符集。
/// **不做 DNS 解析判定**——解析器对任意字符串都可能经搜索域返回 Ok，无法作为拒绝依据。
fn validate_bind_port(bind: &str, port: u16) -> Result<(), BridgeConfigError> {
    if port == 0 {
        return Err(BridgeConfigError::InvalidPort(port));
    }
    if bind.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    let hostname_ok = !bind.is_empty()
        && bind.len() <= 253
        && bind
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !bind.starts_with(['.', '-'])
        && !bind.ends_with(['.', '-']);
    if hostname_ok {
        Ok(())
    } else {
        Err(BridgeConfigError::InvalidBind(format!(
            "{bind}: not an IP address or hostname"
        )))
    }
}

// =============================== 控制器 ===============================

/// 桥生命周期管理（managed state）。
pub struct BridgeController {
    cfg: Arc<RwLock<BridgeConfig>>,
    runtime: Arc<RwLock<BridgeRuntime>>,
    task: Mutex<Option<async_runtime::JoinHandle<()>>>,
    /// 会话访问面：生产 = `ManagerBridgeService`；测试注入 no-op——控制器可脱离 AppHandle 直测。
    service: Arc<dyn BridgeService>,
    /// 持久化目录（生产 = app_data_dir；测试 = 临时目录）。
    config_dir: PathBuf,
    /// 令牌生成器（生产 = OS CSPRNG `new_token`；测试注入固定值/失败）。
    token_gen: Box<dyn Fn() -> Result<String, BridgeConfigError> + Send + Sync>,
}

impl BridgeController {
    pub fn init(app: tauri::AppHandle) -> Self {
        let dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        let ctrl = Self {
            cfg: Arc::new(RwLock::new(load_config(&dir))),
            runtime: Arc::new(RwLock::new(BridgeRuntime::default())),
            task: Mutex::new(None),
            service: Arc::new(super::service::ManagerBridgeService::new(app)),
            config_dir: dir,
            token_gen: Box::new(new_token),
        };
        ctrl.bootstrap();
        ctrl
    }

    /// `#[cfg(test)]` 构造：注入持久化目录、令牌生成器与 no-op 服务，跳过 AppHandle。
    /// 与 init 一样执行自启 bootstrap（测试「自启绑失败 → Error」语义用）。
    #[cfg(test)]
    fn for_tests(
        dir: PathBuf,
        token_gen: Box<dyn Fn() -> Result<String, BridgeConfigError> + Send + Sync>,
    ) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let ctrl = Self {
            cfg: Arc::new(RwLock::new(load_config(&dir))),
            runtime: Arc::new(RwLock::new(BridgeRuntime::default())),
            task: Mutex::new(None),
            service: Arc::new(tests::NoopService),
            config_dir: dir,
            token_gen,
        };
        ctrl.bootstrap();
        ctrl
    }

    /// init 自启：enabled 且已有 token 才尝试；绑失败 → Error 态（配置保留，UI 可见可改）。
    fn bootstrap(&self) {
        let autostart = {
            let c = self.cfg.read();
            c.enabled && !c.token.is_empty()
        };
        if autostart {
            self.start_attempt();
        }
    }

    /// 测试辅助：透传持久化目录（`bridge_runtime_first_enable_bind_failure_*` 校验落盘用）。
    #[cfg(test)]
    fn test_config_dir(&self) -> PathBuf {
        self.config_dir.clone()
    }

    /// 完整视图：配置 + 运行态（`bridge_get_config_cmd` 响应）。
    pub fn get_view(&self) -> BridgeView {
        BridgeView {
            config: self.cfg.read().clone(),
            runtime: self.runtime.read().clone(),
        }
    }

    /// 应用补丁并按需重启。语义：
    /// - 校验先行：端口 0 / 非法 bind → Err，配置原样。
    /// - 启用且无 token 时生成令牌：随机源失败 → Err，旧配置与旧 token 原样。
    /// - **替换运行中监听**（bind/port 变化）：先绑新地址成功才 abort 旧任务；
    ///   绑失败 → Err，旧配置原样、旧监听继续跑、runtime 仍 Running（bound=旧地址）。
    /// - **启动路径绑失败**（首次启用/Error 后重试）：配置变更保留（enabled=true），
    ///   runtime=Error + last_error——UI 可见，用户可改地址或关闭。
    pub fn set_config(&self, patch: &BridgeConfigPatch) -> Result<BridgeView, String> {
        // 1. 校验先行（不动任何状态）
        {
            let c = self.cfg.read();
            let bind = patch.bind.as_deref().unwrap_or(&c.bind);
            let port = patch.port.unwrap_or(c.port);
            validate_bind_port(bind, port).map_err(|e| e.to_string())?;
            // 远程绑定显式确认（一次性，仅约束本次 bind 切换）：把桥暴露到非环回
            // 地址属于对外可见动作，未经确认一律拒绝，错误文本固定（前端可判别）
            if let Some(remote) = patch.bind.as_deref() {
                if bind_parses_remote(remote) && patch.confirm_remote != Some(true) {
                    return Err("remote bind requires explicit confirmation".into());
                }
            }
        }
        // 2. 在克隆上合成新配置：令牌生成失败 → Err 且原配置/原 token 原样
        let mut new_cfg = self.cfg.read().clone();
        if let Some(v) = patch.enabled {
            new_cfg.enabled = v;
        }
        if let Some(ref v) = patch.bind {
            new_cfg.bind = v.clone();
        }
        if let Some(v) = patch.port {
            new_cfg.port = v;
        }
        if let Some(v) = patch.allow_send {
            new_cfg.allow_send = v;
        }
        if let Some(ref v) = patch.token {
            new_cfg.token = v.clone();
        }
        if new_cfg.enabled && new_cfg.token.is_empty() {
            new_cfg.token = (self.token_gen)().map_err(|e| e.to_string())?;
        }
        let will_run = new_cfg.enabled && !new_cfg.token.is_empty();
        let running_before = self.runtime.read().state == RuntimeState::Running;
        let addr_changed = {
            let c = self.cfg.read();
            new_cfg.bind != c.bind || new_cfg.port != c.port
        };

        if !will_run {
            // 停止路径：abort 旧任务 + 保存 + Disabled
            self.stop();
            self.commit(&new_cfg);
            *self.runtime.write() = BridgeRuntime::default();
            return Ok(self.get_view());
        }
        if running_before {
            if !addr_changed {
                // 仅 token/allowSend 等微调：无需重启（令牌按请求实时读 cfg）
                self.commit(&new_cfg);
                return Ok(self.get_view());
            }
            // 替换：先绑新地址，成功才 abort 旧任务；绑失败全部原样 + Err
            let (listener, bound) = bind_listener(&new_cfg.bind, new_cfg.port)
                .map_err(|e| format!("bind {}:{} failed: {e}", new_cfg.bind, new_cfg.port))?;
            self.stop();
            self.commit(&new_cfg);
            self.spawn_serve(listener);
            *self.runtime.write() = BridgeRuntime {
                state: RuntimeState::Running,
                bound: Some(bound),
                last_error: None,
            };
            return Ok(self.get_view());
        }
        // 启动路径：成功 Running；失败 Error（配置保留，用户可改地址或关闭）
        self.commit(&new_cfg);
        self.start_attempt();
        Ok(self.get_view())
    }

    /// 重置令牌（无需重启服务：令牌按请求实时读 `cfg`）。
    /// 随机源失败 → Err，旧 token 原样。
    pub fn regen_token(&self) -> Result<BridgeView, String> {
        let t = (self.token_gen)().map_err(|e| e.to_string())?;
        {
            let mut c = self.cfg.write();
            c.token = t;
            save_config(&self.config_dir, &c);
        }
        Ok(self.get_view())
    }

    /// 启动路径（init 自启 / 停止后启用 / Error 后重试）：同步 bind。
    /// 成功 → Running{bound}；失败 → Error{last_error}。配置不回滚。
    fn start_attempt(&self) {
        let (bind, port) = {
            let c = self.cfg.read();
            (c.bind.clone(), c.port)
        };
        match bind_listener(&bind, port) {
            Ok((listener, bound)) => {
                self.spawn_serve(listener);
                *self.runtime.write() = BridgeRuntime {
                    state: RuntimeState::Running,
                    bound: Some(bound),
                    last_error: None,
                };
            }
            Err(e) => {
                *self.runtime.write() = BridgeRuntime {
                    state: RuntimeState::Error,
                    bound: None,
                    last_error: Some(format!("bind {bind}:{port} failed: {e}")),
                };
            }
        }
    }

    /// 把已绑定的 std listener 交给 spawn 的任务 serve
    /// （`TcpListener::from_std` 须在 tokio runtime 上下文，任务内调用成立）。
    fn spawn_serve(&self, std_listener: std::net::TcpListener) {
        let service = self.service.clone();
        let cfg = self.cfg.clone();
        let h = async_runtime::spawn(async move {
            match TcpListener::from_std(std_listener) {
                Ok(listener) => {
                    if let Err(e) = axum::serve(listener, router(service, cfg)).await {
                        eprintln!("[bridge] serve error: {e}");
                    }
                }
                Err(e) => eprintln!("[bridge] listener convert failed: {e}"),
            }
        });
        *self.task.lock() = Some(h);
    }

    /// 原子写回配置 + 落盘（仅在所有可能失败的步骤之后调用）。
    fn commit(&self, cfg: &BridgeConfig) {
        *self.cfg.write() = cfg.clone();
        save_config(&self.config_dir, cfg);
    }

    fn stop(&self) {
        if let Some(h) = self.task.lock().take() {
            h.abort();
        }
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
    /// 远程绑定的一次性显式确认（仅对本次 bind 切换有效，不写入 bridge.json）。
    pub confirm_remote: Option<bool>,
}

fn config_path(dir: &std::path::Path) -> PathBuf {
    dir.join("bridge.json")
}

fn load_config(dir: &std::path::Path) -> BridgeConfig {
    let p = config_path(dir);
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice::<BridgeConfig>(&b).unwrap_or_default(),
        Err(_) => BridgeConfig::default(),
    }
}

fn save_config(dir: &std::path::Path, cfg: &BridgeConfig) {
    let p = config_path(dir);
    if let Ok(s) = serde_json::to_vec_pretty(cfg) {
        let _ = std::fs::write(p, s);
    }
}

/// bind 串是否解析为非环回 IP 字面量（0.0.0.0/:: 等未指定地址也算远程可达）。
/// 主机名不做 DNS 判定（与 validate_bind_port 同一理由），不纳入确认门禁。
fn bind_parses_remote(bind: &str) -> bool {
    match bind.parse::<std::net::IpAddr>() {
        Ok(ip) => !ip.is_loopback(),
        Err(_) => false,
    }
}

/// 同步绑定指定地址，返回 (std listener, 实际监听地址串)。
/// listener 置为非阻塞——`tokio::net::TcpListener::from_std` 要求非阻塞模式。
/// 同步绑定使启动结果在命令返回前即可观测（旧实现 spawn 内异步 bind，失败仅 eprintln）。
fn bind_listener(bind: &str, port: u16) -> std::io::Result<(std::net::TcpListener, String)> {
    let listener = std::net::TcpListener::bind((bind, port))?;
    listener.set_nonblocking(true)?;
    let bound = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| format!("{bind}:{port}"));
    Ok((listener, bound))
}

// =============================== 路由组装 ===============================

/// 构造桥 router：会话访问面 + 配置（令牌/allowSend 按请求实时读）两参注入，
/// 内部自建 BridgeCtx。pub 供集成测试（tests/bridge_routes.rs）脱离 Tauri 直测路由层。
pub fn router(service: Arc<dyn BridgeService>, cfg: Arc<RwLock<BridgeConfig>>) -> Router {
    let ctx = BridgeCtx { service, cfg };
    Router::new()
        .route("/health", get(super::routes::metadata::health))
        .route("/ports", get(super::routes::metadata::ports))
        .route("/sessions", get(super::routes::metadata::sessions))
        .route(
            "/sessions/:id",
            get(super::routes::metadata::session_detail),
        )
        .route("/sessions/:id/stats", get(super::routes::metadata::stats))
        .route(
            "/sessions/:id/plot-config",
            get(super::routes::analysis::plot_config)
                .post(super::routes::analysis::plot_config_set),
        )
        .route("/sessions/:id/lines", get(super::routes::lines::lines))
        .route("/sessions/:id/follow", get(super::routes::lines::follow))
        .route(
            "/sessions/:id/histogram",
            get(super::routes::lines::histogram),
        )
        .route("/sessions/:id/timing", get(super::routes::analysis::timing))
        .route("/sessions/:id/decode", get(super::routes::analysis::decode))
        .route(
            "/sessions/:id/value-hist",
            get(super::routes::analysis::value_hist),
        )
        .route("/sessions/:id/infer", get(super::routes::analysis::infer))
        .route(
            "/sessions/:id/bookmarks",
            get(super::routes::lines::bookmarks),
        )
        .route("/sessions/:id/alerts", get(super::routes::lines::alerts))
        .route(
            "/sessions/:id/annotations",
            get(super::routes::annotations::annotations_get)
                .post(super::routes::annotations::annotations_post)
                .delete(super::routes::annotations::annotations_delete),
        )
        .route(
            "/sessions/:id/export",
            get(super::routes::lines::export_log),
        )
        .route("/sessions/:id/send", post(super::routes::exchange::send))
        .route(
            "/sessions/:id/exchange",
            post(super::routes::exchange::exchange),
        )
        .layer(middleware::from_fn_with_state(ctx.clone(), auth_mw))
        .with_state(ctx)
}

#[cfg(test)]
mod tests {
    //! token_* / bridge_runtime_*：CSPRNG 令牌与监听生命周期。
    //! 控制器测试注入 no-op 服务与临时目录，不触 AppHandle。

    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    use super::super::error::ServiceError;
    use bytetide_core::serial::manager::{
        BridgeAlert, BridgeAnnotation, BridgeBookmark, BridgeLine, BridgeStats, PlotConfig,
        SendRequest, SessionSnap,
    };

    /// 控制器测试替身：桥 serve 的会话访问面 no-op（生命周期测试只关心绑定与运行态）。
    pub(super) struct NoopService;

    impl BridgeService for NoopService {
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
            Err(ServiceError::NotFound)
        }
        fn lines_after(
            &self,
            _id: &str,
            _no: u64,
            _max: usize,
        ) -> Result<Vec<BridgeLine>, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn line_by_no(&self, _id: &str, _no: u64) -> Result<Option<BridgeLine>, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn last_no(&self, _id: &str) -> Result<u64, ServiceError> {
            Err(ServiceError::NotFound)
        }
        fn log_path(&self, _id: &str) -> Result<std::path::PathBuf, ServiceError> {
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
            Ok(())
        }
        fn notify_annotations_changed(&self, _session_id: &str, _annotations: &[BridgeAnnotation]) {
        }
        fn notify_plot_updated(&self, _session_id: &str, _config: &PlotConfig) {}
    }

    /// 测试持久化目录：std::env::temp_dir + tag+pid 唯一子目录（不引新依赖）。
    fn bridge_tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "bytetide-bridge-test-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("mk temp dir");
        d
    }

    /// 令牌生成器测试替身：前 `fail_after` 次成功返回固定 64 hex，其后注入随机源故障。
    fn fixed_tok_gen(
        fail_after: u32,
    ) -> Box<dyn Fn() -> Result<String, BridgeConfigError> + Send + Sync> {
        let n = AtomicU32::new(0);
        Box::new(move || {
            if n.fetch_add(1, AtomicOrdering::Relaxed) < fail_after {
                Ok("ab".repeat(32))
            } else {
                Err(BridgeConfigError::Randomness("injected rng failure".into()))
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn cfg_patch(
        enabled: Option<bool>,
        bind: Option<&str>,
        port: Option<u16>,
        confirm_remote: Option<bool>,
    ) -> BridgeConfigPatch {
        BridgeConfigPatch {
            enabled,
            bind: bind.map(Into::into),
            port,
            token: None,
            allow_send: None,
            confirm_remote,
        }
    }

    /// 探测一个当前空闲的 loopback 端口（bind 0 拿手后立刻释放；存在极小竞态，测试可接受）。
    fn free_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("probe bind")
            .local_addr()
            .expect("probe addr")
            .port()
    }

    // ----- token_*：格式、唯一性、随机源失败传播 -----

    #[test]
    fn token_from_encodes_64_lowercase_hex() {
        assert_eq!(token_from(&[0u8; 32]), "0".repeat(64));
        assert_eq!(token_from(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(token_from(&[0xAA]), "aa"); // 小写输出
    }

    #[test]
    fn token_generate_formats_64_lowercase_hex_and_is_unique() {
        let is_lower_hex = |s: &str| {
            s.len() == 64
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        };
        let t1 = generate_token(|b| {
            b.fill(0x5a);
            Ok(())
        })
        .expect("ok");
        assert!(is_lower_hex(&t1));
        let t2 = generate_token(|b| {
            b.fill(0xa5);
            Ok(())
        })
        .expect("ok");
        assert!(is_lower_hex(&t2));
        assert_ne!(t1, t2, "不同随机输入必产生不同令牌");
    }

    #[test]
    fn token_generate_propagates_rng_failure_never_falls_back() {
        let err = generate_token(|_| Err("boom".into())).expect_err("rng failed");
        assert!(matches!(err, BridgeConfigError::Randomness(ref m) if m.contains("boom")));
    }

    #[test]
    fn token_new_token_uses_os_csprng_format_and_unique() {
        let is_lower_hex = |s: &str| {
            s.len() == 64
                && s.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        };
        let a = new_token().expect("os randomness available");
        let b = new_token().expect("os randomness available");
        assert!(is_lower_hex(&a), "64 位小写 hex，实得 {a:?}");
        assert_ne!(a, b, "OS CSPRNG 两次生成必不相同");
    }

    // ----- bridge_runtime_*：校验拒绝、随机源失败保旧、绑失败语义 -----

    #[test]
    fn bridge_runtime_set_config_rejects_port_zero_and_invalid_bind() {
        let ctrl = BridgeController::for_tests(bridge_tmp_dir("rejects"), fixed_tok_gen(u32::MAX));
        // 端口 0 → Err，旧配置原样
        let err = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), Some(0), None))
            .expect_err("port 0 must be rejected");
        assert!(err.contains("port"), "错误应提及端口：{err}");
        let v = ctrl.get_view();
        assert_eq!(v.config.port, 8765, "旧配置原样");
        assert_eq!(v.runtime.state, RuntimeState::Disabled);
        // 非法 bind → Err，旧配置原样
        let err = ctrl
            .set_config(&cfg_patch(Some(true), Some("not an ip"), None, None))
            .expect_err("invalid bind must be rejected");
        assert!(err.contains("not an ip"));
        assert_eq!(ctrl.get_view().config.bind, "127.0.0.1");
    }

    #[test]
    fn remote_bind_requires_explicit_confirmation() {
        let ctrl =
            BridgeController::for_tests(bridge_tmp_dir("remote-gate"), fixed_tok_gen(u32::MAX));
        // 未确认的远程 bind → 固定错误文本，配置原样
        let err = ctrl
            .set_config(&cfg_patch(Some(true), Some("0.0.0.0"), None, None))
            .expect_err("remote bind without confirmation must be rejected");
        assert_eq!(err, "remote bind requires explicit confirmation");
        assert_eq!(ctrl.get_view().config.bind, "127.0.0.1", "旧配置原样");
        // 环回地址不受门禁约束
        let free = free_port();
        let view = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), Some(free), None))
            .expect("loopback bind needs no confirmation");
        assert_eq!(view.runtime.state, RuntimeState::Running);
    }

    #[test]
    fn remote_bind_confirmation_is_one_shot_per_switch() {
        let ctrl =
            BridgeController::for_tests(bridge_tmp_dir("remote-oneshot"), fixed_tok_gen(u32::MAX));
        // 确认后切换成功；确认是一次性的，不写入持久化配置
        let free = free_port();
        let view = ctrl
            .set_config(&cfg_patch(
                Some(true),
                Some("0.0.0.0"),
                Some(free),
                Some(true),
            ))
            .expect("confirmed remote bind succeeds");
        assert_eq!(view.config.bind, "0.0.0.0");
        assert_eq!(view.runtime.state, RuntimeState::Running);
        // 再次远程 bind 切换（换端口）未带确认 → 仍拒绝
        let free2 = free_port();
        let err = ctrl
            .set_config(&cfg_patch(None, Some("0.0.0.0"), Some(free2), None))
            .expect_err("confirmation is one-shot, next remote switch needs it again");
        assert_eq!(err, "remote bind requires explicit confirmation");
        // 关闭桥不涉及 bind 切换 → 无需确认
        let view = ctrl
            .set_config(&cfg_patch(Some(false), None, None, None))
            .expect("disable needs no confirmation");
        assert_eq!(view.runtime.state, RuntimeState::Disabled);
        assert_eq!(view.config.bind, "0.0.0.0");
    }

    #[test]
    fn bridge_runtime_rng_failure_keeps_old_token_and_config() {
        // 第 1 次成功（首次启用），其后恒失败
        let ctrl = BridgeController::for_tests(bridge_tmp_dir("rng-keep"), fixed_tok_gen(1));
        let view = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), None, None))
            .expect("first enable succeeds");
        let old_token = view.config.token.clone();
        assert_eq!(old_token.len(), 64);
        // regen_token：随机源失败 → Err 且旧 token 原样
        let err = ctrl.regen_token().expect_err("rng failed");
        assert!(err.contains("injected rng failure"));
        assert_eq!(ctrl.get_view().config.token, old_token, "旧 token 不动");

        // 恒失败生成器 + 全新控制器：启用整体 Err，配置/token 完全保持旧值
        let ctrl2 = BridgeController::for_tests(bridge_tmp_dir("rng-keep2"), fixed_tok_gen(0));
        let err = ctrl2
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), None, None))
            .expect_err("enable requires token");
        assert!(err.contains("injected rng failure"));
        let v = ctrl2.get_view();
        assert!(!v.config.enabled, "生成失败时配置整体保持旧值");
        assert!(v.config.token.is_empty());
        assert_eq!(v.runtime.state, RuntimeState::Disabled);
    }

    #[test]
    fn bridge_runtime_first_enable_bind_failure_keeps_config_and_sets_error() {
        let ctrl =
            BridgeController::for_tests(bridge_tmp_dir("bind-fail"), fixed_tok_gen(u32::MAX));
        // 占住一个真实 loopback 端口，让 controller 绑同一端口必败
        let occupier = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy");
        let port = occupier.local_addr().expect("addr").port();
        let view = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), Some(port), None))
            .expect("首次启用绑失败不返回 Err：配置保留 + Error 态可见");
        assert!(
            view.config.enabled,
            "配置变更保留（enabled=true，用户可改地址或关闭）"
        );
        assert_eq!(view.config.port, port);
        assert_eq!(view.runtime.state, RuntimeState::Error);
        let last_err = view
            .runtime
            .last_error
            .as_deref()
            .expect("last_error visible");
        assert!(last_err.contains("bind"), "错误描述绑定失败：{last_err}");
        assert!(
            !last_err.contains(&view.config.token),
            "错误信息绝不包含 token 值"
        );
        // get_view（= bridge_get_config_cmd 路径）同样可见 Error
        let v2 = ctrl.get_view();
        assert_eq!(v2.runtime.state, RuntimeState::Error);
        assert!(v2.runtime.last_error.is_some());
        // 持久化目录里 enabled=true 已落盘（重启后由 init 自启路径复现同一语义）
        let persisted_path = dir_of(&ctrl).join("bridge.json");
        let persisted: BridgeConfig =
            serde_json::from_slice(&std::fs::read(persisted_path).expect("bridge.json exists"))
                .expect("persisted");
        assert!(persisted.enabled);
    }

    #[test]
    fn bridge_runtime_replace_bind_failure_keeps_old_listener_and_config() {
        let ctrl =
            BridgeController::for_tests(bridge_tmp_dir("replace-fail"), fixed_tok_gen(u32::MAX));
        // 先在空闲端口 A 启动成功
        let free = free_port();
        let view = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), Some(free), None))
            .expect("start on free port");
        assert_eq!(view.runtime.state, RuntimeState::Running);
        assert_eq!(
            view.runtime.bound.as_deref(),
            Some(format!("127.0.0.1:{free}").as_str())
        );
        // 占住端口 B，替换到 B → Err
        let occupier = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy");
        let busy = occupier.local_addr().expect("addr").port();
        let err = ctrl
            .set_config(&cfg_patch(None, None, Some(busy), None))
            .expect_err("replace bind failure must return Err");
        assert!(err.contains("bind"), "错误描述绑定失败：{err}");
        // 旧配置原样、旧监听继续跑、runtime 仍 Running（bound=旧地址）
        let v = ctrl.get_view();
        assert_eq!(v.config.port, free, "旧配置原样保留");
        assert_eq!(v.runtime.state, RuntimeState::Running);
        assert_eq!(
            v.runtime.bound.as_deref(),
            Some(format!("127.0.0.1:{free}").as_str())
        );
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", free)).is_ok(),
            "旧监听继续服务（TCP 可握手）"
        );
    }

    #[test]
    fn bridge_runtime_running_then_disable_roundtrip() {
        let ctrl =
            BridgeController::for_tests(bridge_tmp_dir("roundtrip"), fixed_tok_gen(u32::MAX));
        let free = free_port();
        let view = ctrl
            .set_config(&cfg_patch(Some(true), Some("127.0.0.1"), Some(free), None))
            .expect("start");
        assert_eq!(view.runtime.state, RuntimeState::Running);
        assert_eq!(
            view.runtime.bound.as_deref(),
            Some(format!("127.0.0.1:{free}").as_str())
        );
        assert!(view.runtime.last_error.is_none());
        // 关闭 → Disabled、bound 清空
        let view = ctrl
            .set_config(&cfg_patch(Some(false), None, None, None))
            .expect("disable");
        assert_eq!(view.runtime.state, RuntimeState::Disabled);
        assert_eq!(view.runtime.bound, None);
        assert_eq!(view.runtime.last_error, None);
    }

    #[test]
    fn bridge_runtime_init_autostart_success_and_failure_paths() {
        let token = "ab".repeat(32);
        // 自启成功：预置 enabled + token + 空闲端口 → Running + bound
        let dir_ok = bridge_tmp_dir("init-ok");
        save_config(
            &dir_ok,
            &BridgeConfig {
                enabled: true,
                bind: "127.0.0.1".into(),
                port: free_port(),
                token: token.clone(),
                allow_send: false,
            },
        );
        let ctrl = BridgeController::for_tests(dir_ok, fixed_tok_gen(u32::MAX));
        assert_eq!(ctrl.get_view().runtime.state, RuntimeState::Running);
        assert!(ctrl.get_view().runtime.bound.is_some());

        // 自启失败：端口被占用 → init 后即 Error（配置保留），UI 可见
        let dir_bad = bridge_tmp_dir("init-bad");
        let occupier = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("occupy");
        let port = occupier.local_addr().expect("addr").port();
        save_config(
            &dir_bad,
            &BridgeConfig {
                enabled: true,
                bind: "127.0.0.1".into(),
                port,
                token,
                allow_send: false,
            },
        );
        let ctrl = BridgeController::for_tests(dir_bad, fixed_tok_gen(u32::MAX));
        let v = ctrl.get_view();
        assert_eq!(v.runtime.state, RuntimeState::Error);
        assert!(v
            .runtime
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("bind"));
        assert!(v.config.enabled, "配置保留（用户可改地址或关闭）");
    }

    /// 测试辅助：读控制器持久化目录（仅测试可见的透传）。
    fn dir_of(ctrl: &BridgeController) -> std::path::PathBuf {
        ctrl.test_config_dir()
    }
}
