use std::collections::{HashMap, VecDeque};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::port::{open_port, Dir, LogLine, PortConfig};
use crate::logfmt;
use crate::session::SessionLog;

/// 批量日志事件载荷（按会话路由，单事件名 `log`）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LogPayload {
    pub session_id: String,
    pub lines: Vec<LogLine>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub session_id: String,
    pub status: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub session_id: String,
    pub error: String,
}

/// 桥接环形缓冲容量（带原始字节的近期分析窗口）。
pub const RING_CAP: usize = 20000;

/// 绘图/解码配置（与前端 `PlotConfig` camelCase 对齐；供 REST 桥 `/decode` 复用文法）。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlotConfig {
    pub enabled: bool,
    pub source: String,
    pub frame_head: String,
    pub frame_tail: String,
    pub checksum: String,
    pub channels: u32,
    pub bytes_per_channel: u32,
    pub endian: String,
    pub signed: bool,
    pub max_points: u32,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: "binary".into(),
            frame_head: String::new(),
            frame_tail: String::new(),
            checksum: "none".into(),
            channels: 2,
            bytes_per_channel: 2,
            endian: "big".into(),
            signed: false,
            max_points: 2000,
        }
    }
}

/// REST 桥单行。`no` 为后端独立序号（与前端 `lineCounter` 无关，环形淘汰后继续递增）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLine {
    pub no: u64,
    pub ts: String,
    pub dir: Dir,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    pub epoch_millis: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#match: Option<MatchHit>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchHit {
    pub offset: u64,
    pub length: u64,
    pub field: String,
}

/// 会话列表项（REST `/sessions`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnap {
    pub id: String,
    pub config: PortConfig,
    pub status: String,
    pub line_count: usize,
    pub ring_cap: usize,
}

/// 会话统计（REST `/sessions/:id/stats`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStats {
    pub rx_lines: u64,
    pub tx_lines: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub first_no: u64,
    pub last_no: u64,
    pub first_ts: String,
    pub last_ts: String,
    pub first_epoch: u64,
    pub last_epoch: u64,
    pub ring_cap: usize,
    pub size: usize,
}

/// REST 桥书签条目（前端推送的只读镜像；`no` 为前端 UI 行号，与后端 `no` 体系无关，
/// 携带行文本/时间戳便于 AI 侧经 `?q=` 反查后端 `no`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeBookmark {
    pub no: u64,
    pub ts: String,
    pub text: String,
}

/// REST 桥告警历史条目（前端推送的只读镜像，环形 100 条、新的在前；`no` 为前端 UI 行号）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAlert {
    pub id: String,
    pub rule_id: String,
    pub pattern: String,
    pub level: String,
    pub no: u64,
    pub ts: String,
    pub text: String,
    pub at: u64,
}

/// AI 批注（REST 写入，事件推送到前端界面；`no` 为行号——
/// 桥与 UI 行号在会话内 1:1，除非用户清屏重计数）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAnnotation {
    pub id: String,
    pub no: u64,
    pub ts: String,
    pub text: String,
    pub note: String,
    pub at: u64,
}

/// 每会话环形缓冲 + 计数器（读线程写入，REST 桥读取）。
/// 不参与 emit/盘写/批；仅在既有 `batch.push(line)` 旁增量写入。
pub struct RingBuf {
    ring: Mutex<VecDeque<BridgeLine>>,
    seq: AtomicU64,
    rx_lines: AtomicU64,
    tx_lines: AtomicU64,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
}

impl RingBuf {
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(0),
            rx_lines: AtomicU64::new(0),
            tx_lines: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
        }
    }

    /// 推入一行（分配单调 `no`、更新计数器、超容淘汰最旧）。不改 emit/盘写/批。
    pub fn push(&self, line: &LogLine) {
        let no = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let bl = BridgeLine {
            no,
            ts: line.ts.clone(),
            dir: line.dir,
            text: line.text.clone(),
            bytes: line.bytes.clone(),
            epoch_millis: line.epoch_millis,
            r#match: None,
        };
        let n = line
            .bytes
            .as_deref()
            .map(|b| b.len())
            .unwrap_or_else(|| line.text.len()) as u64;
        match line.dir {
            Dir::Rx => {
                self.rx_lines.fetch_add(1, Ordering::Relaxed);
                self.rx_bytes.fetch_add(n, Ordering::Relaxed);
            }
            Dir::Tx => {
                self.tx_lines.fetch_add(1, Ordering::Relaxed);
                self.tx_bytes.fetch_add(n, Ordering::Relaxed);
            }
        }
        let mut r = self.ring.lock();
        r.push_back(bl);
        while r.len() > RING_CAP {
            r.pop_front();
        }
    }

    /// 清屏：清空环形（不重置 `seq`，保持 `no` 单调，避免 REST 引用碰撞）。
    pub fn clear(&self) {
        self.ring.lock().clear();
    }

    pub fn snapshot(&self) -> Vec<BridgeLine> {
        self.ring.lock().iter().cloned().collect()
    }

    /// 仅返回 `no > since` 的行（长轮询 `/follow` 用，避免全 ring 拷贝）。
    pub fn lines_since(&self, since: u64) -> Vec<BridgeLine> {
        self.ring
            .lock()
            .iter()
            .filter(|l| l.no > since)
            .cloned()
            .collect()
    }

    /// 当前末行 `no`（空环返回 0）。
    pub fn last_no(&self) -> u64 {
        self.ring.lock().back().map(|l| l.no).unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.ring.lock().len()
    }

    /// 首/末行元信息（空环返回全 0）。
    pub fn bounds(&self) -> (u64, u64, String, String, u64, u64, usize) {
        let r = self.ring.lock();
        if r.is_empty() {
            return (0, 0, String::new(), String::new(), 0, 0, 0);
        }
        let f = r.front().expect("non-empty");
        let l = r.back().expect("non-empty");
        (
            f.no,
            l.no,
            f.ts.clone(),
            l.ts.clone(),
            f.epoch_millis,
            l.epoch_millis,
            r.len(),
        )
    }

    pub fn rx_lines(&self) -> u64 {
        self.rx_lines.load(Ordering::Relaxed)
    }
    pub fn tx_lines(&self) -> u64 {
        self.tx_lines.load(Ordering::Relaxed)
    }
    pub fn rx_bytes(&self) -> u64 {
        self.rx_bytes.load(Ordering::Relaxed)
    }
    pub fn tx_bytes(&self) -> u64 {
        self.tx_bytes.load(Ordering::Relaxed)
    }
}

pub enum SendMode {
    Ascii,
    Hex,
}

pub struct SendRequest {
    pub mode: SendMode,
    pub text: String,
}

/// 发往读线程的控制命令：发送数据 / 清屏（截断文件）。
pub enum PortCmd {
    Send(SendRequest),
    Clear,
}

/// 会话类型：实时串口 / 离线加载的日志文件。
enum SessionKind {
    Live,
    Offline,
}

struct SessionHandle {
    #[allow(dead_code)]
    config: PortConfig,
    kind: SessionKind,
    stop: Arc<AtomicBool>,
    write_tx: mpsc::Sender<PortCmd>,
    log_path: PathBuf,
    join: Option<thread::JoinHandle<()>>,
    buf: Arc<RingBuf>,
    plot: Arc<RwLock<PlotConfig>>,
    /// 前端推送的书签/告警历史镜像（REST 只读；后端不产生、不校验内容）。
    bookmarks: Arc<RwLock<Vec<BridgeBookmark>>>,
    alerts: Arc<RwLock<Vec<BridgeAlert>>>,
    /// AI 批注（REST 写入 + 前端同步的双向镜像）。
    annotations: Arc<RwLock<Vec<BridgeAnnotation>>>,
}

pub struct PortManager {
    sessions: RwLock<HashMap<String, SessionHandle>>,
    next_id: AtomicU64,
}

impl PortManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// 创建会话并启动读线程；立即返回会话 ID。端口在读线程内打开，
    /// 打开失败通过 `session-error` 事件回报，状态从 connecting -> connected/error。
    pub fn connect(
        &self,
        config: PortConfig,
        log_config: logfmt::LogConfig,
        app: &AppHandle,
    ) -> anyhow::Result<String> {
        let id = format!("s{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let stop = Arc::new(AtomicBool::new(false));
        let buf = Arc::new(RingBuf::new());
        let plot = Arc::new(RwLock::new(PlotConfig::default()));
        let (write_tx, write_rx) = mpsc::channel::<PortCmd>();
        // 日志路径：模板非空则按当时时间+端口名解析，否则用默认 app_data 路径
        let (log_path, custom_path) = match log_config
            .log_path_template
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(tmpl) => (
                PathBuf::from(logfmt::format_tokens(
                    tmpl,
                    &chrono::Local::now(),
                    &config.name,
                )),
                true,
            ),
            None => (session_log_path(app, &id), false),
        };
        let ts_format = log_config.line_ts_format.filter(|s| !s.is_empty());

        let cfg = config.clone();
        let id2 = id.clone();
        let app2 = app.clone();
        let stop2 = stop.clone();
        let buf2 = buf.clone();
        let lp = log_path.clone();

        let handle = thread::Builder::new()
            .name(format!("reader-{}", id))
            .spawn(move || {
                // 串口与 TCP/UDP 源共用同一装配路径，按传输类型选择循环
                if is_net_transport(&cfg) {
                    net_loop(cfg, id2, app2, stop2, write_rx, lp, ts_format, custom_path, buf2)
                } else {
                    reader_loop(cfg, id2, app2, stop2, write_rx, lp, ts_format, custom_path, buf2)
                }
            })
            .map_err(|e| anyhow::anyhow!("spawn reader thread failed: {e}"))?;

        self.sessions.write().insert(
            id.clone(),
            SessionHandle {
                config,
                kind: SessionKind::Live,
                stop,
                write_tx,
                log_path,
                join: Some(handle),
                buf,
                plot,
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
            },
        );
        Ok(id)
    }

    /// 创建离线会话：无端口、无读线程，仅把已解析的日志行灌入 ring 供 REST 桥分析。
    /// `send` 对离线会话直接报错；`clear_log` 直接清 ring。id 用 `o{N}` 前缀，与 live 的 `s{N}` 区分。
    pub fn load_offline(
        &self,
        config: PortConfig,
        path: PathBuf,
        lines: Vec<LogLine>,
    ) -> String {
        let id = format!("o{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let buf = Arc::new(RingBuf::new());
        for line in &lines {
            buf.push(line);
        }
        let plot = Arc::new(RwLock::new(PlotConfig::default()));
        // rx 立即 drop -> 写通道天然断开；send() 对离线会话会先于此处早退报错。
        let (write_tx, _rx) = mpsc::channel::<PortCmd>();
        self.sessions.write().insert(
            id.clone(),
            SessionHandle {
                config,
                kind: SessionKind::Offline,
                stop: Arc::new(AtomicBool::new(false)),
                write_tx,
                log_path: path,
                join: None,
                buf,
                plot,
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
            },
        );
        id
    }

    pub fn disconnect(&self, id: &str) -> anyhow::Result<()> {
        let mut handle = self
            .sessions
            .write()
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        handle.stop.store(true, Ordering::Relaxed);
        if let Some(join) = handle.join.take() {
            let _ = join.join();
        }
        Ok(())
    }

    pub fn send(&self, id: &str, req: SendRequest) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            return Err(anyhow::anyhow!("离线会话不可发送"));
        }
        h.write_tx
            .send(PortCmd::Send(req))
            .map_err(|_| anyhow::anyhow!("发送通道已关闭"))?;
        Ok(())
    }

    pub fn clear_log(&self, id: &str) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            h.buf.clear();
            return Ok(());
        }
        h.write_tx
            .send(PortCmd::Clear)
            .map_err(|_| anyhow::anyhow!("通道已关闭"))?;
        Ok(())
    }

    /// 各 live 会话的Ring末行滞后快照（诊断心跳用）：(会话 id, 末行落后墙钟 ms, ring 长度, RX 行数)。
/// 离线会话无读线程不参与；空 ring 返回 lag=0。
    pub fn perf_snapshot(&self) -> Vec<(String, u64, usize, u64)> {
        let now = now_ms();
        self.sessions
            .read()
            .iter()
            // 已停止的会话（用户点停止/断开但标签仍在）末行时间戳永远停在过去，
            // 计入只会产生随墙钟无限增长的假滞后噪音
            .filter(|(_, h)| {
                matches!(h.kind, SessionKind::Live) && !h.stop.load(Ordering::Relaxed)
            })
        .map(|(id, h)| {
            let (_, _, _, _, _, last_epoch, len) = h.buf.bounds();
            let lag = if last_epoch == 0 { 0 } else { now.saturating_sub(last_epoch) };
            (id.clone(), lag, len, h.buf.rx_lines())
        })
        .collect()
}

    /// 补拉：返回 ring 中 epochMillis > since_epoch 的行（前端自愈兜底用，
    /// 深度节流/事件丢失时按时间戳对齐补齐，与行号体系无关）。封顶 2000 行。
    pub fn ring_lines_since(&self, id: &str, since_epoch: u64, max: usize) -> anyhow::Result<Vec<BridgeLine>> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        let all = h.buf.snapshot();
        let from = all
            .iter()
            .position(|l| l.epoch_millis > since_epoch)
            .unwrap_or(all.len());
        let max = max.clamp(1, 2000);
        Ok(all[from..].iter().take(max).cloned().collect())
    }

    /// 会话日志文件完整路径（导出/打开日志位置用）。
    pub fn session_log_path(&self, id: &str) -> anyhow::Result<String> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        Ok(h.log_path.to_string_lossy().into_owned())
    }

    // ===== REST 桥访问器（同 crate 读取，不暴露 SessionHandle） =====

    pub fn bridge_list(&self) -> Vec<SessionSnap> {
        let s = self.sessions.read();
        s.iter()
            .map(|(id, h)| {
                let stop = h.stop.load(Ordering::Relaxed);
                let status = match h.kind {
                    SessionKind::Offline => "offline",
                    SessionKind::Live => {
                        if stop {
                            "disconnected"
                        } else {
                            "connected"
                        }
                    }
                };
                SessionSnap {
                    id: id.clone(),
                    config: h.config.clone(),
                    status: status.into(),
                    line_count: h.buf.len(),
                    ring_cap: RING_CAP,
                }
            })
            .collect()
    }

    pub fn bridge_snapshot(&self, id: &str) -> Option<Vec<BridgeLine>> {
        self.sessions.read().get(id).map(|h| h.buf.snapshot())
    }

    /// 长轮询：返回 `no > since` 的行 + 当前 `lastNo`（无会话返回 None）。
    pub fn bridge_follow(&self, id: &str, since: u64) -> Option<(Vec<BridgeLine>, u64)> {
        self.sessions
            .read()
            .get(id)
            .map(|h| (h.buf.lines_since(since), h.buf.last_no()))
    }

    pub fn bridge_stats(&self, id: &str) -> Option<BridgeStats> {
        let s = self.sessions.read();
        s.get(id).map(|h| {
            let (first_no, last_no, first_ts, last_ts, first_epoch, last_epoch, size) =
                h.buf.bounds();
            BridgeStats {
                rx_lines: h.buf.rx_lines(),
                tx_lines: h.buf.tx_lines(),
                rx_bytes: h.buf.rx_bytes(),
                tx_bytes: h.buf.tx_bytes(),
                first_no,
                last_no,
                first_ts,
                last_ts,
                first_epoch,
                last_epoch,
                ring_cap: RING_CAP,
                size,
            }
        })
    }

    pub fn bridge_plot(&self, id: &str) -> Option<PlotConfig> {
        self.sessions
            .read()
            .get(id)
            .map(|h| h.plot.read().clone())
    }

    pub fn bridge_set_plot(&self, id: &str, cfg: PlotConfig) -> bool {
        let s = self.sessions.read();
        if let Some(h) = s.get(id) {
            *h.plot.write() = cfg;
            true
        } else {
            false
        }
    }

    /// 书签镜像（前端推送同步，REST `/bookmarks` 只读）。
    pub fn bridge_set_bookmarks(&self, id: &str, v: Vec<BridgeBookmark>) -> bool {
        if let Some(h) = self.sessions.read().get(id) {
            *h.bookmarks.write() = v;
            true
        } else {
            false
        }
    }

    pub fn bridge_bookmarks(&self, id: &str) -> Option<Vec<BridgeBookmark>> {
        self.sessions
            .read()
            .get(id)
            .map(|h| h.bookmarks.read().clone())
    }

    /// 告警历史镜像（前端推送同步，REST `/alerts` 只读）。
    pub fn bridge_set_alerts(&self, id: &str, v: Vec<BridgeAlert>) -> bool {
        if let Some(h) = self.sessions.read().get(id) {
            *h.alerts.write() = v;
            true
        } else {
            false
        }
    }

    pub fn bridge_alerts(&self, id: &str) -> Option<Vec<BridgeAlert>> {
        self.sessions.read().get(id).map(|h| h.alerts.read().clone())
    }

    /// AI 批注镜像（REST 写入 / 前端同步双向；整包替换）。
    pub fn bridge_set_annotations(&self, id: &str, v: Vec<BridgeAnnotation>) -> bool {
        if let Some(h) = self.sessions.read().get(id) {
            *h.annotations.write() = v;
            true
        } else {
            false
        }
    }

    pub fn bridge_annotations(&self, id: &str) -> Option<Vec<BridgeAnnotation>> {
        self.sessions
            .read()
            .get(id)
            .map(|h| h.annotations.read().clone())
    }

    #[allow(dead_code)]
    pub fn disconnect_all(&self) {
        let handles: Vec<(String, SessionHandle)> = self.sessions.write().drain().collect();
        for (_, mut h) in handles {
            h.stop.store(true, Ordering::Relaxed);
            if let Some(join) = h.join.take() {
                let _ = join.join();
            }
        }
    }
}

fn session_log_path(app: &AppHandle, id: &str) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("logs"));
    dir.join("sessions").join(format!("{}.log", id))
}

#[allow(clippy::too_many_arguments)]
fn reader_loop(
    config: PortConfig,
    session_id: String,
    app: AppHandle,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    log_path: PathBuf,
    ts_format: Option<String>,
    custom_path: bool,
    ring: Arc<RingBuf>,
) {
    let (session_log, ts_fmt) =
        open_session_log(&app, &session_id, &log_path, custom_path, ts_format);

    let mut port = match open_port(&config) {
        Ok(p) => p,
        Err(e) => {
            emit_error(
                &app,
                &session_id,
                &format!("打开串口 {} 失败: {}", config.name, e),
            );
            emit_status(&app, &session_id, "error");
            return;
        }
    };
    emit_status(&app, &session_id, "connected");

    stream_loop(
        port.as_mut(),
        "串口连接已断开",
        "写入串口失败",
        &app,
        &session_id,
        stop,
        write_rx,
        &ts_fmt,
        session_log,
        ring,
    );
}

fn emit_batch(app: &AppHandle, session_id: &str, batch: &mut Vec<LogLine>) {
    if batch.is_empty() {
        return;
    }
    let payload = LogPayload {
        session_id: session_id.to_string(),
        lines: std::mem::take(batch),
    };
    let _ = app.emit("log", payload);
}

fn emit_status(app: &AppHandle, session_id: &str, status: &str) {
    let _ = app.emit(
        "session-status",
        StatusPayload {
            session_id: session_id.to_string(),
            status: status.to_string(),
        },
    );
}

fn emit_error(app: &AppHandle, session_id: &str, error: &str) {
    let _ = app.emit(
        "session-error",
        ErrorPayload {
            session_id: session_id.to_string(),
            error: error.to_string(),
        },
    );
}

fn decode_hex(s: &str) -> Vec<u8> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..cleaned.len())
        .step_by(2)
        .filter_map(|i| cleaned.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

fn now_ms() -> u64 {
    chrono::Local::now().timestamp_millis() as u64
}

fn make_rx_line(raw: &[u8], ts_fmt: &str) -> LogLine {
    let text = String::from_utf8_lossy(raw).into_owned();
    // 仅当原始字节非合法 UTF-8 时携带，前端才能恢复 0x80+ 的孤立字节
    let bytes = if std::str::from_utf8(raw).is_err() {
        Some(raw.to_vec())
    } else {
        None
    };
    LogLine {
        ts: logfmt::format_ts(ts_fmt),
        dir: Dir::Rx,
        text,
        bytes,
        epoch_millis: now_ms(),
    }
}

fn sink_line(
    line: LogLine,
    session_log: &mut Option<SessionLog>,
    ring: &RingBuf,
    batch: &mut Vec<LogLine>,
) {
    if let Some(w) = session_log.as_mut() {
        w.append(&line);
    }
    ring.push(&line);
    batch.push(line);
}

fn finish_loop(
    session_log: Option<SessionLog>,
    batch: &mut Vec<LogLine>,
    app: &AppHandle,
    session_id: &str,
) {
    emit_batch(app, session_id, batch);
    if let Some(mut w) = session_log {
        let _ = w.flush();
    }
    emit_status(app, session_id, "disconnected");
}

/// 打开会话日志文件并上报 connecting；打开失败仅在自定义路径时告警。
/// 返回 (session_log, ts_fmt)，供串口/网络两类循环共用。
fn open_session_log(
    app: &AppHandle,
    session_id: &str,
    log_path: &std::path::Path,
    custom_path: bool,
    ts_format: Option<String>,
) -> (Option<SessionLog>, String) {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let session_log = match SessionLog::create(log_path) {
        Ok(w) => Some(w),
        Err(e) => {
            if custom_path {
                emit_error(
                    app,
                    session_id,
                    &format!("日志路径无效/不可写: {}: {}", log_path.display(), e),
                );
            }
            None
        }
    };
    let ts_fmt = ts_format.unwrap_or_else(|| "%h:%m:%s.%t".to_string());
    emit_status(app, session_id, "connecting");
    (session_log, ts_fmt)
}

/// 串口与 TCP/UDP 共用的读循环：行切分、TX 回显、空闲半行刷出、40ms 批量回推。
/// 序列化语义与旧 reader_loop 完全一致（读取超时由 IO 源自行设定为 200ms）。
#[allow(clippy::too_many_arguments)]
fn stream_loop<T: std::io::Read + std::io::Write + ?Sized>(
    io: &mut T,
    eof_msg: &str,
    write_err_msg: &str,
    app: &AppHandle,
    session_id: &str,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    ts_fmt: &str,
    mut session_log: Option<SessionLog>,
    ring: Arc<RingBuf>,
) {
    let mut buf = vec![0u8; 65536];
    let mut line_buf: Vec<u8> = Vec::new();
    let mut batch: Vec<LogLine> = Vec::new();
    let mut last_flush = Instant::now();
    let mut last_idle_flush = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = write_rx.try_recv() {
            match cmd {
                PortCmd::Send(req) => {
                    let bytes: Vec<u8> = match req.mode {
                        SendMode::Ascii => req.text.clone().into_bytes(),
                        SendMode::Hex => decode_hex(&req.text),
                    };
                    match io.write_all(&bytes) {
                        Ok(()) => sink_line(
                            LogLine {
                                ts: logfmt::format_ts(ts_fmt),
                                dir: Dir::Tx,
                                text: req.text.clone(),
                                bytes: None,
                                epoch_millis: now_ms(),
                            },
                            &mut session_log,
                            &ring,
                            &mut batch,
                        ),
                        Err(_) => emit_error(app, session_id, write_err_msg),
                    }
                }
                PortCmd::Clear => {
                    if let Some(w) = session_log.as_mut() {
                        let _ = w.clear();
                    }
                    ring.clear();
                }
            }
        }

        match io.read(&mut buf) {
            Ok(0) => {
                emit_error(app, session_id, eof_msg);
                break;
            }
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\n' {
                        let mut raw = line_buf.clone();
                        if raw.last() == Some(&b'\r') {
                            raw.pop();
                        }
                        line_buf.clear();
                        sink_line(make_rx_line(&raw, ts_fmt), &mut session_log, &ring, &mut batch);
                    } else {
                        line_buf.push(b);
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                // 空闲时把未结束的半行也刷出，保证流式数据可见
                if !line_buf.is_empty() && last_idle_flush.elapsed() > Duration::from_millis(150) {
                    let mut raw = line_buf.clone();
                    if raw.last() == Some(&b'\r') {
                        raw.pop();
                    }
                    line_buf.clear();
                    sink_line(make_rx_line(&raw, ts_fmt), &mut session_log, &ring, &mut batch);
                    last_idle_flush = Instant::now();
                }
            }
            Err(e) => {
                emit_error(app, session_id, &format!("读取错误: {e}"));
                break;
            }
        }

        // 批量回推，防止 IPC 风暴
        let now = Instant::now();
        if (!batch.is_empty() && now.duration_since(last_flush) > Duration::from_millis(40))
            || batch.len() >= 64
        {
            emit_batch(app, session_id, &mut batch);
            last_flush = now;
        }
    }

    finish_loop(session_log, &mut batch, app, session_id);
}

// ---------- 网络数据源（TCP client/server / UDP 监听）----------

const NET_READ_TIMEOUT: Duration = Duration::from_millis(200);

/// 已建立的网络链路；读写行为与串口 Box<dyn SerialPort> 同构。
enum NetLink {
    Tcp(std::net::TcpStream),
    Udp(std::net::UdpSocket),
}

impl std::io::Read for NetLink {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            NetLink::Tcp(s) => s.read(buf),
            // UDP 无 Read 实现（仅有共享引用的旧实现差异），直接用固有 recv
            NetLink::Udp(s) => s.recv(buf),
        }
    }
}
impl std::io::Write for NetLink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            NetLink::Tcp(s) => s.write(buf),
            // 未 connect 的监听 socket 上 send 会报 NotConnected，符合“UDP 纯接收”设计
            NetLink::Udp(s) => s.send(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bind_host_of(config: &PortConfig) -> String {
    match config.tcp_host.as_deref() {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => "0.0.0.0".to_string(),
    }
}

/// 按 transport 建立链路。tcp-server 只接受首个连入连接；
/// udp 为纯接收监听（未实现对端发送，TX 将在 stream_loop 中报写入失败）。
fn establish_link(config: &PortConfig) -> std::io::Result<NetLink> {
    match config.transport.as_deref() {
        Some("tcp-client") => {
            let host = config.tcp_host.clone().unwrap_or_else(|| "127.0.0.1".into());
            let port = config.tcp_port.unwrap_or(23);
            let stream = std::net::TcpStream::connect((host.as_str(), port))?;
            stream.set_read_timeout(Some(NET_READ_TIMEOUT))?;
            stream.set_nodelay(true).ok();
            Ok(NetLink::Tcp(stream))
        }
        Some("tcp-server") => {
            let listener = std::net::TcpListener::bind((bind_host_of(config).as_str(), config.tcp_port.unwrap_or(9000)))?;
            // 只服务首个接入连接；该连接断开即会话结束（多并发列为后续增强）
            let (stream, _peer) = listener.accept()?;
            stream.set_read_timeout(Some(NET_READ_TIMEOUT))?;
            stream.set_nodelay(true).ok();
            Ok(NetLink::Tcp(stream))
        }
        Some("udp") => {
            let sock = std::net::UdpSocket::bind((
                bind_host_of(config).as_str(),
                config.udp_local_port.unwrap_or(0),
            ))?;
            sock.set_read_timeout(Some(NET_READ_TIMEOUT))?;
            Ok(NetLink::Udp(sock))
        }
        other => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("未知传输类型: {other:?}"),
        )),
    }
}

fn describe_transport(config: &PortConfig) -> String {
    match config.transport.as_deref() {
        Some("tcp-client") => format!(
            "TCP 连接 {}:{}",
            config.tcp_host.clone().unwrap_or_default(),
            config.tcp_port.map(|p| p.to_string()).unwrap_or_default()
        ),
        Some("tcp-server") => format!(
            "TCP 服务 {}:{}", bind_host_of(config), 
            config.tcp_port.map(|p| p.to_string()).unwrap_or_default()
        ),
        Some("udp") => format!(
            "UDP 监听 {}:{}",
            bind_host_of(config),
            config.udp_local_port.map(|p| p.to_string()).unwrap_or_default()
        ),
        _ => "串口".to_string(),
    }
}

fn is_net_transport(config: &PortConfig) -> bool {
    matches!(config.transport.as_deref(), Some(t) if t != "serial")
}

/// 网络源会话循环：建链 -> 复用 stream_loop；失败路径与串口一致（error 状态 + 错误事件）。
#[allow(clippy::too_many_arguments)]
fn net_loop(
    config: PortConfig,
    session_id: String,
    app: AppHandle,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    log_path: PathBuf,
    ts_format: Option<String>,
    custom_path: bool,
    ring: Arc<RingBuf>,
) {
    let desc = describe_transport(&config);
    let (session_log, ts_fmt) = open_session_log(&app, &session_id, &log_path, custom_path, ts_format);

    let mut link = match establish_link(&config) {
        Ok(l) => l,
        Err(e) => {
            emit_error(&app, &session_id, &format!("建立 {desc} 失败: {e}"));
            emit_status(&app, &session_id, "error");
            return;
        }
    };
    emit_status(&app, &session_id, "connected");

    stream_loop(
        &mut link,
        "网络连接已断开",
        "网络写入失败",
        &app,
        &session_id,
        stop,
        write_rx,
        &ts_fmt,
        session_log,
        ring,
    );
}

#[cfg(test)]
mod tests {
    //! RingBuf 纯逻辑单测：单调 no、快照顺序、游标、边界、驱逐、计数器。
    use super::*;

    fn mk_log(ts: &str, dir: Dir, text: &str, bytes: Option<Vec<u8>>, epoch: u64) -> LogLine {
        LogLine {
            ts: ts.into(),
            dir,
            text: text.into(),
            bytes,
            epoch_millis: epoch,
        }
    }

    #[test]
    fn push_assigns_monotonic_no_and_snapshot_order() {
        let buf = RingBuf::new();
        for i in 0..3 {
            buf.push(&mk_log("00:00:00.001", Dir::Rx, &format!("l{i}"), None, 1000 + i));
        }
        let nos: Vec<u64> = buf.snapshot().iter().map(|l| l.no).collect();
        assert_eq!(nos, vec![1, 2, 3]);
        assert_eq!(buf.last_no(), 3);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn lines_since_cursor() {
        let buf = RingBuf::new();
        for i in 0..5 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let s1: Vec<u64> = buf.lines_since(1).iter().map(|l| l.no).collect();
        assert_eq!(s1, vec![2, 3, 4, 5]);
        let s0: Vec<u64> = buf.lines_since(0).iter().map(|l| l.no).collect();
        assert_eq!(s0, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn bounds_and_clear_keeps_seq_monotonic() {
        let buf = RingBuf::new();
        buf.push(&mk_log("01:00:00.000", Dir::Rx, "a", None, 3600000));
        buf.push(&mk_log("02:00:00.000", Dir::Tx, "b", None, 7200000));
        let (fno, lno, fts, lts, fep, lep, len) = buf.bounds();
        assert_eq!((fno, lno), (1, 2));
        assert_eq!(fts, "01:00:00.000");
        assert_eq!(lts, "02:00:00.000");
        assert_eq!((fep, lep), (3600000, 7200000));
        assert_eq!(len, 2);
        // 清屏不重置 seq，避免 no 引用碰撞
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.last_no(), 0);
        buf.push(&mk_log("t", Dir::Rx, "c", None, 0));
        assert_eq!(buf.snapshot()[0].no, 3);
    }

    #[test]
    fn evicts_oldest_beyond_cap() {
        let buf = RingBuf::new();
        let n = (RING_CAP + 5) as u64;
        for i in 0..n {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        assert_eq!(buf.len(), RING_CAP);
        let snap = buf.snapshot();
        assert_eq!(snap.first().unwrap().no, 6); // 先 5 行被驱逐
        assert_eq!(snap.last().unwrap().no, n);
        assert_eq!(buf.last_no(), n);
    }

    #[test]
    fn perf_snapshot_skips_stopped_and_offline() {
        let m = PortManager::new();
        let mk_handle = |stopped: bool| {
            let ring = Arc::new(RingBuf::new());
            ring.push(&mk_log("t", Dir::Rx, "x", None, 1000));
            let stop = Arc::new(AtomicBool::new(stopped));
            let (tx, _rx) = mpsc::channel();
            SessionHandle {
                config: PortConfig::default(),
                kind: SessionKind::Live,
                stop,
                write_tx: tx,
                log_path: PathBuf::from("x.log"),
                join: None,
                buf: ring,
                plot: Arc::new(RwLock::new(PlotConfig::default())),
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
            }
        };
        m.sessions.write().insert("s1".into(), mk_handle(false));
        m.sessions.write().insert("s2".into(), mk_handle(true)); // 已停止
        let ids: Vec<String> = m.perf_snapshot().into_iter().map(|(id, ..)| id).collect();
        assert_eq!(ids, vec!["s1".to_string()]);
    }

    #[test]
    fn counters_by_dir_and_bytes() {
        let buf = RingBuf::new();
        // rx 携带字节(2B) + rx 纯文本(len 3) + tx 纯文本(len 1)
        buf.push(&mk_log("t", Dir::Rx, "x", Some(vec![0xAA, 0x55]), 0));
        buf.push(&mk_log("t", Dir::Rx, "abc", None, 0));
        buf.push(&mk_log("t", Dir::Tx, "y", None, 0));
        assert_eq!(buf.rx_lines(), 2);
        assert_eq!(buf.tx_lines(), 1);
        assert_eq!(buf.rx_bytes(), 5); // 2 + 3
        assert_eq!(buf.tx_bytes(), 1);
    }

    #[test]
    fn bridge_bookmarks_alerts_roundtrip_and_unknown_session() {
        let m = PortManager::new();
        let id = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
        assert!(m.bridge_bookmarks(&id).unwrap().is_empty());
        assert!(m.bridge_alerts(&id).unwrap().is_empty());

        let bms = vec![BridgeBookmark {
            no: 3,
            ts: "00:00:01.000".into(),
            text: "ERR line".into(),
        }];
        assert!(m.bridge_set_bookmarks(&id, bms.clone()));
        assert_eq!(m.bridge_bookmarks(&id).unwrap(), bms);

        let alerts = vec![BridgeAlert {
            id: "a1".into(),
            rule_id: "r1".into(),
            pattern: "ERR".into(),
            level: "err".into(),
            no: 3,
            ts: "00:00:01.000".into(),
            text: "ERR line".into(),
            at: 12345,
        }];
        assert!(m.bridge_set_alerts(&id, alerts.clone()));
        assert_eq!(m.bridge_alerts(&id).unwrap(), alerts);

        // 未知会话：写入 false、读取 None
        assert!(!m.bridge_set_bookmarks("nope", vec![]));
        assert!(m.bridge_bookmarks("nope").is_none());
        assert!(!m.bridge_set_alerts("nope", vec![]));
        assert!(m.bridge_alerts("nope").is_none());

        let notes = vec![BridgeAnnotation {
            id: "an1".into(),
            no: 9,
            ts: "00:00:09.000".into(),
            text: "ERR line".into(),
            note: "从这里开始校验失败".into(),
            at: 999,
        }];
        assert!(m.bridge_set_annotations(&id, notes.clone()));
        assert_eq!(m.bridge_annotations(&id).unwrap(), notes);
        assert!(!m.bridge_set_annotations("nope", vec![]));
        assert!(m.bridge_annotations("nope").is_none());
    }
}
