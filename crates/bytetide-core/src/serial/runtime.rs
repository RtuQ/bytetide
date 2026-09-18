//! 会话运行态：状态机事实源（`SessionStatus`/`SessionState`）、每会话共享单元
//! `SessionRuntime`（ring + 运行状态 + 前端推送规则配置五件套）与读线程主循环
//! （`session_thread`/`stream_loop`，Stage 2 Task 3 自 manager.rs 迁出）。
//! `serial::manager` 经再导出保持旧路径一个发布周期。
//!
//! 入库收敛（Task 3）：[`SessionRuntime::ingest`] 统一 ring/计数/告警评估；
//! 自动回复与捕获触发决策仅 `IngestOrigin::Transport` 产生——自动回复的写动作由
//! 传输循环执行（runtime 绝不持有/锁 io 句柄），TX 回显经 ingest(Transport) 入表，
//! 捕获的 arm/入档由持有 [`CaptureController`](super::capture::CaptureController) 的
//! 传输循环执行（档案行序需与自动回复回显交错，ingest 单独无法保证）；
//! `Replay` 不产生自动回复与捕获决策 → 结构上零自动回复零捕获副作用。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;

use super::capture::CaptureController;
use super::port::{Dir, LogLine, PortConfig};
use super::recording::RecordingController;
use super::ring::RingBuf;
use super::rules::{
    alert_eval, auto_reply_payload, capture_eval, AlertCfg, AlertWinState, AutoReplyCfg, CaptureCfg,
};
use super::transport::{describe_transport, is_net_transport, open_transport, Transport};
use super::{now_ms, BridgeAlert, PortCmd, SendMode, SendRequest};
use crate::errors::err_msg;
use crate::logfmt;
use crate::replay::ReplayState;
use crate::sink::EventSink;

/// 会话运行状态：REST 快照（bridge_list）与 sink 事件的共同事实源（serde 小写，
/// 与既有 REST status 字符串一致）。读线程写、REST 读，挂在 SessionRuntime.state。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// 已创建、读线程建链中（开串口/建链完成前）
    #[default]
    Connecting,
    /// 链路就绪，读循环运行中
    Connected,
    /// 正常收尾（用户停止/对端关闭/停止标志退出）
    Disconnected,
    /// 终止性错误（开串口失败/建链失败/读硬错误）
    Error,
    /// 离线加载的日志会话（无链路）
    Offline,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::Connecting => "connecting",
            SessionStatus::Connected => "connected",
            SessionStatus::Disconnected => "disconnected",
            SessionStatus::Error => "error",
            SessionStatus::Offline => "offline",
        }
    }
}

/// 每会话共享运行状态（读线程写，REST 读）。
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub status: SessionStatus,
    pub last_error: Option<String>,
}

impl SessionState {
    /// 置状态并紧接发 sink 事件（先写状态后 emit，保证事件观察者看到的状态已就位）。
    pub fn set_status(&mut self, sink: &dyn EventSink, session_id: &str, status: SessionStatus) {
        let event = status.as_str();
        self.status = status;
        sink.status(session_id, event);
    }

    /// 记录 last_error 并发 error 事件。`to_status` 显式声明是否随错误迁移状态：
    /// `Some(Error)`=错误且连接终止（开串口失败/建链失败/读硬错误）；
    /// `None`=错误但连接保持（写失败/分段失败等，仅记错、不发 status 事件）。
    pub fn set_error(
        &mut self,
        sink: &dyn EventSink,
        session_id: &str,
        msg: &str,
        to_status: Option<SessionStatus>,
    ) {
        self.last_error = Some(msg.to_string());
        sink.error(session_id, msg);
        if let Some(s) = to_status {
            self.set_status(sink, session_id, s);
        }
    }
}

/// 行入库来源：实时传输（串口/网络）或回放（离线载入/时序回放）。
/// 仅 Transport 产生自动回复与捕获触发决策；Replay 评估告警但零其他副作用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestOrigin {
    Transport,
    Replay,
}

/// 单行入库结果：ring 游标、待传输循环执行的自动回复、本行触发的告警（调用方
/// 攒批：mirror splice + 一次 alert_hits 事件，与既有批次语义一致）与捕获触发决策。
pub struct IngestOutcome {
    pub ring_no: u64,
    pub auto_reply: Option<SendRequest>,
    pub alerts: Vec<BridgeAlert>,
    pub capture_hit: Option<super::capture::CaptureHit>,
}

/// 每会话运行态共享单元：ring（拉模型数据真相）+ 运行状态 + 前端推送的规则配置。
/// SessionHandle 持 `Arc<SessionRuntime>`，读线程与 REST 桥经它共享同一批 Arc。
pub struct SessionRuntime {
    pub ring: Arc<RingBuf>,
    pub state: Arc<RwLock<SessionState>>,
    /// 自动回复规则（前端推送；读线程内评估并直接回写设备）
    pub auto_reply: Arc<RwLock<AutoReplyCfg>>,
    /// 告警规则（前端推送；读线程内评估，命中走 mirror + alert-hit 事件）
    pub alerts: Arc<RwLock<AlertCfg>>,
    /// 触发式现场捕获配置（前端推送；读线程内逐行评估触发）
    pub capture: Arc<RwLock<CaptureCfg>>,
    /// 细粒度回放状态（仅回放会话：replay runner 写、manager 查询面读；
    /// 非回放会话恒 None，见 crate::replay::ReplayState）
    pub replay_state: Arc<RwLock<Option<ReplayState>>>,
    /// 回放当前文件行号水位（最后已 ingest 的源文件行 no；seek 后未恢复=目标-1，
    /// loop 回卷=0）。仅回放会话由 runner 写、manager 查询面读（T7 控制面进度），
    /// 非回放会话恒 0。
    pub replay_cursor: Arc<RwLock<u64>>,
    /// 告警窗口/冷却状态（每会话独占；原 stream_loop 栈上状态迁入——随会话生灭）
    alert_states: Mutex<HashMap<String, AlertWinState>>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self {
            ring: Arc::new(RingBuf::new()),
            state: Arc::new(RwLock::new(SessionState::default())),
            auto_reply: Arc::new(RwLock::new(AutoReplyCfg::default())),
            alerts: Arc::new(RwLock::new(AlertCfg::default())),
            capture: Arc::new(RwLock::new(CaptureCfg::default())),
            replay_state: Arc::new(RwLock::new(None)),
            replay_cursor: Arc::new(RwLock::new(0)),
            alert_states: Mutex::new(HashMap::new()),
        }
    }

    /// 单行入库：分配单调 `no`、更新方向计数，并评估规则——
    /// 告警（窗口/冷却）两种 origin 都评估；自动回复与捕获触发决策仅 Transport。
    /// 不做落盘（调用方经 RecordingController 自理）、不执行自动回复写动作、不触碰捕获。
    /// `sink` 仅为 plan 指定签名保留：ingest 自身不发事件（告警随 outcome 返回，
    /// 由调用方攒批 mirror + alert_hits，与既有批次语义一致）。
    pub fn ingest(
        &self,
        line: &LogLine,
        origin: IngestOrigin,
        _sink: &dyn EventSink,
    ) -> IngestOutcome {
        let ring_no = self.ring.push(line);
        let mut out = IngestOutcome {
            ring_no,
            auto_reply: None,
            alerts: Vec::new(),
            capture_hit: None,
        };
        // 仅 RX 参与规则评估；TX 回显不受规则影响（armed 入档由调用方自理）
        if line.dir != Dir::Rx {
            return out;
        }
        // 告警：攒批结果随 outcome 返回（命中事件稀疏，不会形成 IPC 洪水）
        let now = now_ms();
        let alert_cfg = self.alerts.read().clone();
        let fired = alert_eval(&alert_cfg, &mut self.alert_states.lock(), &line.text, now);
        for rule in fired {
            out.alerts.push(BridgeAlert {
                id: format!("a{:x}-{:x}", now, ring_no),
                rule_id: rule.id.clone(),
                pattern: rule.pattern.clone(),
                level: rule.level.clone(),
                no: ring_no,
                ts: line.ts.clone(),
                text: line.text.clone(),
                at: now,
            });
        }
        if origin != IngestOrigin::Transport {
            return out;
        }
        // 自动回复：首条命中规则即回（payload/mode 交传输循环解码写出）
        let reply_cfg = self.auto_reply.read().clone();
        if let Some((payload, mode)) = auto_reply_payload(&reply_cfg, &line.text) {
            out.auto_reply = Some(SendRequest {
                mode: if mode == "hex" {
                    SendMode::Hex
                } else {
                    SendMode::Ascii
                },
                text: payload,
            });
        }
        // 捕获触发决策：关键词命中优先，其次本行告警联动；arm/入档由传输循环执行
        let capture_cfg = self.capture.read().clone();
        let hits = capture_eval(&capture_cfg, &line.text);
        out.capture_hit = hits
            .first()
            .map(|r| super::capture::CaptureHit {
                trigger: "keyword",
                rule: r.pattern.clone(),
            })
            .or_else(|| {
                out.alerts.last().map(|a| super::capture::CaptureHit {
                    trigger: "alert",
                    rule: a.pattern.clone(),
                })
            });
        out
    }

    /// 仅写共享状态，不发事件（事件薄壳见 SessionState::set_status/set_error，
    /// 「先写状态后 emit」的顺序由调用点保证）。
    pub fn set_status(&self, status: SessionStatus) {
        self.state.write().status = status;
    }

    /// 仅记录 last_error，不发事件、不迁移状态。
    pub fn set_error(&self, message: impl Into<String>) {
        self.state.write().last_error = Some(message.into());
    }
}

// ---------- 读线程主循环（原 manager 的 reader_loop/net_loop/stream_loop 合并迁移） ----------

/// 读线程入口：串口与 TCP/UDP 源共用同一装配路径——开录制（connecting）→ 建链
/// （失败=错误且终止）→ connected → stream_loop。事件顺序与迁移前逐点一致。
#[allow(clippy::too_many_arguments)]
pub(crate) fn session_thread(
    config: PortConfig,
    session_id: String,
    sink: Arc<dyn EventSink>,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    log_path: PathBuf,
    log_shared: Arc<RwLock<PathBuf>>,
    ts_format: Option<String>,
    custom_path: bool,
    midnight_rotate: bool,
    rt: Arc<SessionRuntime>,
    captures_dir: PathBuf,
    alerts_mirror: Arc<RwLock<Vec<BridgeAlert>>>,
) {
    let net = is_net_transport(&config);
    let desc = describe_transport(&config);
    let (rec, ts_fmt) = open_recording(
        &rt.state,
        &*sink,
        &session_id,
        &log_path,
        custom_path,
        ts_format,
        log_shared,
        midnight_rotate,
    );

    let mut transport = match open_transport(&config) {
        Ok(t) => t,
        Err(e) => {
            // 错误且终止：状态置 Error 并保留（finish 不会被走到，也不得回落 disconnected）
            let msg = if net {
                err_msg("open_link_failed", format!("{desc}: {e}"))
            } else {
                err_msg("open_port_failed", format!("{}: {}", config.name, e))
            };
            rt.state
                .write()
                .set_error(&*sink, &session_id, &msg, Some(SessionStatus::Error));
            return;
        }
    };
    rt.state
        .write()
        .set_status(&*sink, &session_id, SessionStatus::Connected);

    let (eof_msg, write_err_msg) = if net {
        (err_msg("net_disconnected", ""), err_msg("net_write_failed", ""))
    } else {
        (
            err_msg("port_disconnected", ""),
            err_msg("port_write_failed", ""),
        )
    };
    stream_loop(
        transport.as_mut(),
        &eof_msg,
        &write_err_msg,
        &*sink,
        &session_id,
        &rt,
        stop,
        write_rx,
        &ts_fmt,
        rec,
        captures_dir,
        alerts_mirror,
    );
}

/// 打开录制并上报 connecting；打开失败仅在自定义路径时告警。
/// 返回 (RecordingController, ts_fmt)（原 open_session_log 语义：空路径 = 不落盘）。
#[allow(clippy::too_many_arguments)]
fn open_recording(
    state: &RwLock<SessionState>,
    sink: &dyn EventSink,
    session_id: &str,
    log_path: &std::path::Path,
    custom_path: bool,
    ts_format: Option<String>,
    log_shared: Arc<RwLock<PathBuf>>,
    midnight_rotate: bool,
) -> (RecordingController, String) {
    let ts_fmt = ts_format.unwrap_or_else(|| "%h:%m:%s.%t".to_string());
    let rec = if log_path.as_os_str().is_empty() {
        // 空路径 = 不落盘（CLI 缺省）：不建文件、不改错误状态，仅上报 connecting
        RecordingController::disabled(log_shared, midnight_rotate, chrono::Local::now())
    } else {
        match RecordingController::open(
            log_path.to_path_buf(),
            log_shared.clone(),
            true,
            midnight_rotate,
            chrono::Local::now(),
        ) {
            Ok(r) => r,
            Err(e) => {
                if custom_path {
                    // 日志文件打不开不影响连接：仅记 last_error，状态不变
                    state.write().set_error(
                        sink,
                        session_id,
                        &err_msg("log_path_unwritable", format!("{}: {}", log_path.display(), e)),
                        None,
                    );
                }
                RecordingController::disabled(log_shared, midnight_rotate, chrono::Local::now())
            }
        }
    };
    state
        .write()
        .set_status(sink, session_id, SessionStatus::Connecting);
    (rec, ts_fmt)
}

fn decode_hex(s: &str) -> Vec<u8> {
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

fn make_tx_line(text: &str, ts_fmt: &str) -> LogLine {
    LogLine {
        ts: logfmt::format_ts(ts_fmt),
        dir: Dir::Tx,
        text: text.to_string(),
        bytes: None,
        epoch_millis: now_ms(),
    }
}

/// 逐 RX 行的传输侧处理：落盘 → ingest（ring/告警/自动回复/捕获决策）→
/// 自动回复经传输写回 + TX 回显入库 → 捕获 arm/入档（顺序与迁移前 apply_rx_rules 一致）。
#[allow(clippy::too_many_arguments)]
fn handle_rx_line(
    line: &LogLine,
    io: &mut dyn Transport,
    rec: &mut RecordingController,
    cap: &mut CaptureController,
    rt: &SessionRuntime,
    sink: &dyn EventSink,
    session_id: &str,
    ts_fmt: &str,
    write_err_msg: &str,
    fired_alerts: &mut Vec<BridgeAlert>,
) {
    let _ = rec.write(line);
    let outcome = rt.ingest(line, IngestOrigin::Transport, sink);
    fired_alerts.extend(outcome.alerts);
    // 自动回复：读线程内直接回写设备（不依赖宿主存活），TX 回显入库
    if let Some(reply) = outcome.auto_reply {
        let bytes = match reply.mode {
            SendMode::Ascii => reply.text.clone().into_bytes(),
            SendMode::Hex => decode_hex(&reply.text),
        };
        if !bytes.is_empty() {
            match io.write_all(&bytes) {
                Ok(()) => {
                    let tx = make_tx_line(&reply.text, ts_fmt);
                    let _ = rec.write(&tx);
                    rt.ingest(&tx, IngestOrigin::Transport, sink);
                    // armed 捕获在后续窗口内：自动回复的 TX 也入档
                    cap.on_line(&tx, sink, session_id);
                }
                // 回写失败但连接保持：仅记 last_error
                Err(_) => rt
                    .state
                    .write()
                    .set_error(sink, session_id, write_err_msg, None),
            }
        }
    }
    // 现场捕获：关键词命中或本行告警联动触发；armed 时本行继续入档（快照已含触发行）
    let capture_cfg = rt.capture.read().clone();
    cap.commit(
        outcome.capture_hit,
        &capture_cfg,
        &rt.ring,
        line,
        sink,
        session_id,
        &rt.state,
    );
}

/// 串口与 TCP/UDP 共用的读循环：行切分、TX 回显、空闲半行刷出。
/// 数据不经事件推送（IPC 洪水会把消费者调度饿死）：行进 ring（消费方
/// 按 `no` 游标拉取）与落盘文件；只有状态/错误/低频事件走 sink。
#[allow(clippy::too_many_arguments)]
fn stream_loop(
    io: &mut dyn Transport,
    eof_msg: &str,
    write_err_msg: &str,
    sink: &dyn EventSink,
    session_id: &str,
    rt: &SessionRuntime,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    ts_fmt: &str,
    mut rec: RecordingController,
    captures_dir: PathBuf,
    alerts_mirror: Arc<RwLock<Vec<BridgeAlert>>>,
) {
    let mut buf = vec![0u8; 65536];
    let mut line_buf: Vec<u8> = Vec::new();
    let mut last_idle_flush = Instant::now();
    // 待上报命中（攒批：写 mirror + 稀疏事件）
    let mut fired_alerts: Vec<BridgeAlert> = Vec::new();
    // 触发式现场捕获：armed = 已触发、尚在写后续窗口
    let mut cap = CaptureController::new(captures_dir);
    // 午夜自动分段：1 秒节流的检查时钟（日期基准在 RecordingController 内）
    let mut last_date_check = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = write_rx.try_recv() {
            match cmd {
                PortCmd::Send(req) => {
                    let bytes: Vec<u8> = match req.mode {
                        SendMode::Ascii => req.text.clone().into_bytes(),
                        SendMode::Hex => decode_hex(&req.text),
                    };
                    match io.write_all(&bytes) {
                        Ok(()) => {
                            let tx = make_tx_line(&req.text, ts_fmt);
                            let _ = rec.write(&tx);
                            rt.ingest(&tx, IngestOrigin::Transport, sink);
                            // armed 捕获在后续窗口内：TX 回显一并入档
                            cap.on_line(&tx, sink, session_id);
                        }
                        // 写失败但连接保持：仅记 last_error，状态不变
                        Err(_) => rt
                            .state
                            .write()
                            .set_error(sink, session_id, write_err_msg, None),
                    }
                }
                PortCmd::Clear => {
                    let _ = rec.clear();
                    rt.ring.clear();
                }
                PortCmd::RecOn(path) => {
                    // 分段/恢复录制：flush+关闭旧文件后另起新文件；创建失败则报错停写
                    //（ring/视图不受影响），下一条 RecOn 可再试
                    if let Err(e) = rec.rotate(path.clone()) {
                        rt.state.write().set_error(
                            sink,
                            session_id,
                            &err_msg("log_rotate_failed", format!("{}: {}", path.display(), e)),
                            None,
                        );
                    }
                }
                PortCmd::RecOff => {
                    let _ = rec.pause();
                }
                PortCmd::Signal { pin, level } => {
                    if let Err(e) = io.set_signal(pin, level) {
                        // 置位失败连接不受影响：仅记 last_error
                        rt.state
                            .write()
                            .set_error(sink, session_id, &err_msg("set_signal_failed", &e), None);
                    }
                }
            }
        }

        // 午夜自动分段（1 秒节流）：与 RecOn/RecOff 同在读线程内串行切 writer；
        // log_shared 只写路径单元，不碰 sessions 锁
        if last_date_check.elapsed() >= Duration::from_secs(1) {
            last_date_check = Instant::now();
            if let Err(fail) = rec.tick_midnight(chrono::Local::now()) {
                rt.state.write().set_error(
                    sink,
                    session_id,
                    &err_msg(
                        "midnight_rotate_failed",
                        format!("{}: {}", fail.path.display(), fail.err),
                    ),
                    None,
                );
            }
        }

        match io.read(&mut buf) {
            Ok(0) => {
                // EOF：对端正常关闭。记 last_error 供 REST 观测，状态走正常断开
                //（finish_loop 置 Disconnected，不进 Error）
                rt.state.write().set_error(sink, session_id, eof_msg, None);
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
                        let line = make_rx_line(&raw, ts_fmt);
                        handle_rx_line(
                            &line,
                            io,
                            &mut rec,
                            &mut cap,
                            rt,
                            sink,
                            session_id,
                            ts_fmt,
                            write_err_msg,
                            &mut fired_alerts,
                        );
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
                    let line = make_rx_line(&raw, ts_fmt);
                    handle_rx_line(
                        &line,
                        io,
                        &mut rec,
                        &mut cap,
                        rt,
                        sink,
                        session_id,
                        ts_fmt,
                        write_err_msg,
                        &mut fired_alerts,
                    );
                    last_idle_flush = Instant::now();
                }
            }
            Err(e) => {
                // 读硬错误且终止：状态置 Error，finish_loop 保留不覆盖
                rt.state
                    .write()
                    .set_error(sink, session_id, &err_msg("read_failed", &e), Some(SessionStatus::Error));
                break;
            }
        }

        // 命中上报：写 mirror（REST /alerts 只读）+ 稀疏事件通知宿主（通知/提示音在 UI 侧）
        if !fired_alerts.is_empty() {
            {
                let mut m = alerts_mirror.write();
                m.splice(0..0, fired_alerts.iter().cloned());
                let n = m.len();
                if n > 100 {
                    m.drain(..n - 100);
                }
            }
            sink.alert_hits(session_id, std::mem::take(&mut fired_alerts));
        }

        // 捕获后续窗口到期（窗口内无新行也要收尾，否则档案悬着不发事件）
        cap.expire_if_due(sink, session_id);
    }

    // 断连现场：会话结束前把最后 pre_ms 窗口抓成档案（设备重启/掉线现场最珍贵）
    {
        let cfg = rt.capture.read().clone();
        cap.trigger_on_disconnect(&cfg, &rt.ring, sink, session_id, &rt.state);
    }
    cap.finalize(sink, session_id);
    finish_loop(&mut rec, &rt.state, sink, session_id);
}

fn finish_loop(
    rec: &mut RecordingController,
    state: &RwLock<SessionState>,
    sink: &dyn EventSink,
    session_id: &str,
) {
    let _ = rec.flush();
    // 正常收尾：仅当会话仍在活动态才置 Disconnected。Error 保留（读失败不该被
    // 覆盖成 disconnected）；Offline / 已 Disconnected 不动、也不重复发事件。
    let mut st = state.write();
    if matches!(
        st.status,
        SessionStatus::Connecting | SessionStatus::Connected
    ) {
        st.set_status(sink, session_id, SessionStatus::Disconnected);
    }
}

#[cfg(test)]
mod tests {
    //! 状态机 + sink 薄壳（自 manager 迁移）、ingest 双 origin 契约（Replay 隔离）、
    //! open_recording 事件序。读循环端到端行为由 manager 的 loopback TCP 集成测试覆盖。
    use std::path::Path;

    use super::super::port::Dir;
    use super::super::rules::{AlertRuleCfg, AutoReplyRuleCfg, CaptureRuleCfg};
    use super::*;
    use crate::sink::{NullSink, VecSink};

    fn mk_log(dir: Dir, text: &str, epoch: u64) -> LogLine {
        LogLine {
            ts: "t".into(),
            dir,
            text: text.into(),
            bytes: None,
            epoch_millis: epoch,
        }
    }

    fn rx(text: &str) -> LogLine {
        mk_log(Dir::Rx, text, 1000)
    }

    fn auto_reply_cfg() -> AutoReplyCfg {
        AutoReplyCfg {
            enabled: true,
            rules: vec![AutoReplyRuleCfg {
                id: "r1".into(),
                trigger: "ERROR".into(),
                reply: "RESET".into(),
                use_regex: false,
                case_sensitive: false,
                whole_word: false,
                append_newline: false,
                reply_mode: "ascii".into(),
                enabled: true,
            }],
        }
    }

    fn alert_cfg() -> AlertCfg {
        AlertCfg {
            enabled: true,
            rules: vec![AlertRuleCfg {
                id: "a1".into(),
                pattern: "ERROR".into(),
                use_regex: false,
                case_sensitive: false,
                whole_word: false,
                min_count: 1,
                window_sec: 0,
                cooldown_sec: 0,
                level: "warn".into(),
                enabled: true,
            }],
        }
    }

    fn capture_cfg() -> CaptureCfg {
        CaptureCfg {
            enabled: true,
            on_disconnect: false,
            pre_ms: 60_000,
            post_ms: 60_000,
            rules: vec![CaptureRuleCfg {
                id: "c1".into(),
                pattern: "ERROR".into(),
                use_regex: false,
                case_sensitive: false,
                whole_word: false,
                enabled: true,
            }],
        }
    }

    // ---------- ingest：双 origin 契约 ----------

    #[test]
    fn ingest_wraps_ring_push_with_monotonic_no_and_counters() {
        let rt = SessionRuntime::new();
        assert_eq!(rt.state.read().status, SessionStatus::Connecting);
        let no1 = rt
            .ingest(&mk_log(Dir::Rx, "x", 1), IngestOrigin::Transport, &NullSink)
            .ring_no;
        let no2 = rt
            .ingest(&mk_log(Dir::Tx, "y", 2), IngestOrigin::Transport, &NullSink)
            .ring_no;
        assert_eq!((no1, no2), (1, 2));
        assert_eq!(rt.ring.len(), 2);
        assert_eq!(rt.ring.rx_lines(), 1);
        assert_eq!(rt.ring.tx_lines(), 1);
        assert_eq!(rt.ring.last_no(), 2);
    }

    #[test]
    fn transport_origin_yields_auto_reply_capture_hit_and_alerts() {
        let rt = SessionRuntime::new();
        *rt.auto_reply.write() = auto_reply_cfg();
        *rt.alerts.write() = alert_cfg();
        *rt.capture.write() = capture_cfg();
        let out = rt.ingest(&rx("x ERROR occurred"), IngestOrigin::Transport, &NullSink);
        assert_eq!(out.ring_no, 1);
        // 自动回复交传输循环执行（payload 原文 + ascii 模式）
        let reply = out.auto_reply.expect("Transport 应返回自动回复");
        assert!(matches!(reply.mode, SendMode::Ascii));
        assert_eq!(reply.text, "RESET");
        // 告警同批评估（min_count=1 即触发）
        assert_eq!(out.alerts.len(), 1);
        assert_eq!(out.alerts[0].pattern, "ERROR");
        assert_eq!(out.alerts[0].no, 1);
        // 捕获决策：关键词命中优先于告警联动
        let hit = out.capture_hit.expect("Transport 应返回捕获决策");
        assert_eq!(hit.trigger, "keyword");
        assert_eq!(hit.rule, "ERROR");
        // runtime 侧无捕获副作用（arm/入档由传输循环执行）
        assert_eq!(rt.ring.len(), 1);
    }

    #[test]
    fn replay_origin_has_zero_auto_reply_and_zero_capture_side_effects() {
        let rt = SessionRuntime::new();
        *rt.auto_reply.write() = auto_reply_cfg();
        *rt.alerts.write() = alert_cfg();
        *rt.capture.write() = capture_cfg();
        let sink = VecSink::default();
        let out = rt.ingest(&rx("x ERROR occurred"), IngestOrigin::Replay, &sink);
        // 零自动回复、零捕获决策（结构上：无 CaptureController 可被 Replay 触达）
        assert!(out.auto_reply.is_none(), "Replay 不得产生自动回复");
        assert!(out.capture_hit.is_none(), "Replay 不得产生捕获触发");
        // 告警两种 origin 都评估
        assert_eq!(out.alerts.len(), 1);
        // ring/计数照常；无任何 sink 事件
        assert_eq!(out.ring_no, 1);
        assert_eq!(rt.ring.len(), 1);
        assert_eq!(rt.ring.rx_lines(), 1);
        assert!(sink.0.lock().is_empty(), "ingest 自身不发事件");
    }

    #[test]
    fn tx_line_updates_counters_but_skips_rules() {
        let rt = SessionRuntime::new();
        *rt.auto_reply.write() = auto_reply_cfg();
        *rt.alerts.write() = alert_cfg();
        *rt.capture.write() = capture_cfg();
        let out = rt.ingest(
            &mk_log(Dir::Tx, "ERROR echo", 1),
            IngestOrigin::Transport,
            &NullSink,
        );
        assert!(out.auto_reply.is_none());
        assert!(out.capture_hit.is_none());
        assert!(out.alerts.is_empty());
        assert_eq!(out.ring_no, 1);
        assert_eq!(rt.ring.tx_lines(), 1);
    }

    #[test]
    fn alert_window_state_persists_across_ingest_calls() {
        // 窗口计数跨行累计（状态随 runtime 存续，原 stream_loop 栈上状态语义等价）
        let rt = SessionRuntime::new();
        *rt.alerts.write() = AlertCfg {
            enabled: true,
            rules: vec![AlertRuleCfg {
                pattern: "fail".into(),
                min_count: 2,
                window_sec: 10,
                enabled: true,
                ..AlertRuleCfg::default()
            }],
        };
        let out1 = rt.ingest(&rx("fail"), IngestOrigin::Transport, &NullSink);
        assert!(out1.alerts.is_empty(), "首行不足阈值");
        let out2 = rt.ingest(&rx("fail"), IngestOrigin::Transport, &NullSink);
        assert_eq!(out2.alerts.len(), 1, "同窗口第二行触发");
    }

    // ---------- SessionState：REST 快照与 sink 事件的共同事实源（自 manager 迁移） ----------

    #[test]
    fn session_state_transitions_connecting_connected_disconnected() {
        let sink = VecSink::default();
        // live 会话默认起点：Connecting（connect() 初始化）
        let mut st = SessionState::default();
        assert_eq!(st.status, SessionStatus::Connecting);
        assert_eq!(st.last_error, None);
        st.set_status(&sink, "s1", SessionStatus::Connected);
        assert_eq!(st.status, SessionStatus::Connected);
        st.set_status(&sink, "s1", SessionStatus::Disconnected);
        assert_eq!(st.status, SessionStatus::Disconnected);
        // 每次置位同步上报，字符串与 REST 序列化一致
        assert_eq!(
            sink.0.lock().clone(),
            vec![
                "status s1 connected".to_string(),
                "status s1 disconnected".to_string(),
            ]
        );
    }

    #[test]
    fn session_state_set_error_without_transition_keeps_status() {
        // 错误但连接保持（写失败/分段失败等）：只记 last_error，不发 status 事件
        let sink = VecSink::default();
        let mut st = SessionState::default();
        st.set_status(&sink, "s1", SessionStatus::Connected);
        st.set_error(&sink, "s1", "port_write_failed|", None);
        assert_eq!(st.status, SessionStatus::Connected);
        assert_eq!(st.last_error.as_deref(), Some("port_write_failed|"));
        assert_eq!(
            sink.0.lock().clone(),
            vec![
                "status s1 connected".to_string(),
                "error s1 port_write_failed|".to_string(),
            ]
        );
    }

    #[test]
    fn session_state_error_then_finish_preserves_error() {
        // 错误且终止：status=Error；正常收尾不得把 Error 覆盖成 disconnected
        let sink = VecSink::default();
        let mut st = SessionState::default();
        st.set_status(&sink, "s1", SessionStatus::Connected);
        st.set_error(&sink, "s1", "read_failed|boom", Some(SessionStatus::Error));
        assert_eq!(st.status, SessionStatus::Error);
        assert_eq!(st.last_error.as_deref(), Some("read_failed|boom"));
        let shared = RwLock::new(st);
        let mut rec = disabled_rec();
        finish_loop(&mut rec, &shared, &sink, "s1");
        assert_eq!(shared.read().status, SessionStatus::Error);
        // 事件序：error -> status error；finish 未追加 disconnected
        assert_eq!(
            sink.0.lock().clone(),
            vec![
                "status s1 connected".to_string(),
                "error s1 read_failed|boom".to_string(),
                "status s1 error".to_string(),
            ]
        );
    }

    #[test]
    fn session_state_finish_marks_disconnected_and_offline_stays_offline() {
        let sink = VecSink::default();
        // 活动态正常收尾：Connected -> Disconnected
        let connected = RwLock::new(SessionState {
            status: SessionStatus::Connected,
            last_error: None,
        });
        let mut rec = disabled_rec();
        finish_loop(&mut rec, &connected, &sink, "s1");
        assert_eq!(connected.read().status, SessionStatus::Disconnected);
        // Offline 恒 Offline：finish 不动非活动态、不重复发事件
        let offline = RwLock::new(SessionState {
            status: SessionStatus::Offline,
            last_error: None,
        });
        let mut rec2 = disabled_rec();
        finish_loop(&mut rec2, &offline, &sink, "s2");
        assert_eq!(offline.read().status, SessionStatus::Offline);
        assert_eq!(
            sink.0.lock().clone(),
            vec!["status s1 disconnected".to_string()]
        );
    }

    fn disabled_rec() -> RecordingController {
        RecordingController::disabled(
            Arc::new(RwLock::new(PathBuf::new())),
            false,
            chrono::Local::now(),
        )
    }

    // ---------- open_recording：空路径不落盘（自 manager 迁移） ----------

    #[test]
    fn open_recording_empty_path_skips_recording() {
        // CLI 缺省不落盘：空路径 -> 不建文件（不 active），仍上报 connecting、无错误事件
        let sink = VecSink::default();
        let state = Arc::new(RwLock::new(SessionState::default()));
        let (rec, ts_fmt) = open_recording(
            &state,
            &sink,
            "s1",
            Path::new(""),
            false,
            None,
            Arc::new(RwLock::new(PathBuf::new())),
            false,
        );
        assert!(!rec.is_active());
        assert_eq!(ts_fmt, "%h:%m:%s.%t");
        assert_eq!(state.read().status, SessionStatus::Connecting);
        assert_eq!(
            sink.0.lock().clone(),
            vec!["status s1 connecting".to_string()]
        );
    }

    // ===== 集成：假传输（本机 loopback TCP，不开硬件、不固定 sleep 轮询）端到端跑 =====
    // session_thread/stream_loop 读循环（自 manager.rs 测试迁入：被测主循环在本文件，
    // manager 只提供编排；manager.rs 行数受架构检查器 1000 行限额约束）。
    // 事件序/状态串断言与迁移前逐点一致。

    use std::time::{Duration, Instant};

    use super::super::{PortManager, SessionSnap};

    /// tcp-client 配置指向本机端口。
    fn tcp_client_cfg(port: u16) -> PortConfig {
        PortConfig {
            name: format!("tcp-{port}"),
            transport: Some("tcp-client".into()),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(port),
            ..PortConfig::default()
        }
    }

    /// 轮询直到 bridge_list 状态到达期望值且 VecSink 已收到对应事件
    ///（实现保证先写状态后发事件，两者都到齐才算稳定）；带总超时上限。
    fn wait_for_status(m: &PortManager, sink: &VecSink, id: &str, want: &str) -> SessionSnap {
        let deadline = Instant::now() + Duration::from_secs(5);
        let want_ev = format!("status {id} {want}");
        loop {
            let snap = m
                .bridge_list()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap_or_else(|| panic!("会话 {id} 不在 bridge_list"));
            let has_event = sink.0.lock().contains(&want_ev);
            if snap.status == want && has_event {
                return snap;
            }
            assert!(
                Instant::now() < deadline,
                "等待状态 {want} 超时：status={:?} 事件={:?}",
                snap.status,
                sink.0.lock().clone()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 一致性断言（plan 明确要求）：bridge_list 的 status/lastError 与 sink 最近一次事件一致。
    fn assert_snap_matches_sink(snap: &SessionSnap, sink: &VecSink, id: &str) {
        let events = sink.0.lock().clone();
        let status_prefix = format!("status {id} ");
        let error_prefix = format!("error {id} ");
        let last_status = events.iter().rev().find(|e| e.starts_with(&status_prefix));
        assert_eq!(
            last_status.map(|e| &e[status_prefix.len()..]),
            Some(snap.status.as_str()),
            "REST status 应与 sink 最近一次 status 事件一致"
        );
        let last_error = events.iter().rev().find(|e| e.starts_with(&error_prefix));
        assert_eq!(
            last_error.map(|e| e[error_prefix.len()..].to_string()),
            snap.last_error,
            "REST lastError 应与 sink 最近一次 error 事件一致"
        );
    }

    #[test]
    fn session_state_tcp_client_connecting_then_connected() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let sink_handle: Arc<dyn EventSink> = sink.clone();
        let id = m
            .connect(
                tcp_client_cfg(port),
                logfmt::LogConfig::default(),
                sink_handle,
                PathBuf::new(),
            )
            .expect("connect");
        let snap = wait_for_status(&m, &sink, &id, "connected");
        assert_eq!(snap.last_error, None);
        // sink 事件序：connecting 先于 connected
        let events = sink.0.lock().clone();
        let ci = events
            .iter()
            .position(|e| *e == format!("status {id} connecting"))
            .expect("connecting 事件");
        let cn = events
            .iter()
            .position(|e| *e == format!("status {id} connected"))
            .expect("connected 事件");
        assert!(ci < cn);
        assert_snap_matches_sink(&snap, &sink, &id);
        let _ = m.disconnect(&id);
    }

    #[test]
    fn session_state_tcp_client_connect_failure_sets_error() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        drop(listener); // 无人监听：建链必失败
        let sink_handle: Arc<dyn EventSink> = sink.clone();
        let id = m
            .connect(
                tcp_client_cfg(port),
                logfmt::LogConfig::default(),
                sink_handle,
                PathBuf::new(),
            )
            .expect("connect");
        let snap = wait_for_status(&m, &sink, &id, "error");
        let err = snap.last_error.as_deref().expect("应记录 last_error");
        assert!(
            err.starts_with("open_link_failed|"),
            "错误消息应为 code|detail 建链失败形状: {err}"
        );
        assert_snap_matches_sink(&snap, &sink, &id);
        let _ = m.disconnect(&id);
    }

    #[test]
    fn session_state_tcp_client_eof_goes_disconnected() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let sink_handle: Arc<dyn EventSink> = sink.clone();
        let id = m
            .connect(
                tcp_client_cfg(port),
                logfmt::LogConfig::default(),
                sink_handle,
                PathBuf::new(),
            )
            .expect("connect");
        wait_for_status(&m, &sink, &id, "connected");
        // 服务端接受连接后主动关闭：客户端 read 得 Ok(0)（EOF）→ 正常断开路径
        let (accepted, _) = listener.accept().expect("客户端已连入");
        drop(accepted);
        let snap = wait_for_status(&m, &sink, &id, "disconnected");
        // EOF 记 last_error（供 REST 观测）但状态走正常断开而非 error
        assert_eq!(snap.last_error.as_deref(), Some("net_disconnected|"));
        let events = sink.0.lock().clone();
        assert!(
            events
                .iter()
                .any(|e| *e == format!("error {id} net_disconnected|")),
            "EOF 应有 error 事件: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| *e == format!("status {id} disconnected")),
            "EOF 应有 disconnected 事件: {events:?}"
        );
        assert_snap_matches_sink(&snap, &sink, &id);
        let _ = m.disconnect(&id);
    }
}
