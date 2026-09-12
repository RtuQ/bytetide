//! 会话编排层：PortManager 建 runtime → 开链路（读线程内）→ spawn/join 线程 →
//! 路由命令 → 查询访问器。Stage 2 Task 3 后本文件不再含链路分支（transport/）、
//! 落盘录制与路径命名（recording.rs）、现场捕获（capture.rs）、读循环与行评估
//! （runtime.rs 的 session_thread/stream_loop/ingest）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use parking_lot::RwLock;

use super::port::{LogLine, PortConfig};
use super::recording::{default_log_path, next_segment_path};
use super::rules::{AlertCfg, AutoReplyCfg, CaptureCfg};
use super::runtime::{session_thread, IngestOrigin};
use super::transport::host_of;
use crate::logfmt;
use crate::sink::EventSink;
// ring/runtime/transport/共享 DTO 自本文件迁出（Stage 2 Task 2/3）：
// 旧公开路径经再导出保持一个发布周期
pub use super::ring::{BridgeLine, BridgeStats, MatchHit, RingBounds, RingBuf, RING_CAP};
pub use super::runtime::{SessionRuntime, SessionState, SessionStatus};
pub use super::transport::Pin;
pub use super::{
    BridgeAlert, BridgeAnnotation, BridgeBookmark, PlotConfig, SendMode, SendRequest, SessionSnap,
};

/// 经 core 再导出 `anyhow`：manager 的公开签名（send/session_log_path…）使用其类型，
/// 桌面端 crate（src-tauri）未直接依赖 anyhow，桥服务 trait 沿用同签名时经此路径引用。
pub use anyhow;

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
    write_tx: mpsc::Sender<super::PortCmd>,
    /// 当前日志文件路径（「分段」/读线程午夜轮转后随最新分段更新；「打开日志」指向当前文件）。
    /// 共享单元（短临界区只护路径本身）：writer 的切换全部在读线程内串行（「分段」命令
    /// 也经 PortCmd 进读线程），此处仅登记最新路径；严禁持它的锁再去锁 sessions（无死锁面）。
    log_path: Arc<RwLock<PathBuf>>,
    /// 连接时解析出的基准路径：分段命名始终基于它，避免 stem 越叠越长
    log_base: PathBuf,
    join: Option<thread::JoinHandle<()>>,
    /// 会话运行态五件套（ring/状态/自动回复/告警/捕获配置，见 SessionRuntime）：
    /// 读线程写、REST 桥读。锁序与 log_path 同类：仅 sessions → runtime 内部锁，
    /// 读线程只碰 runtime 各单元本身，无死锁面。
    runtime: Arc<SessionRuntime>,
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

impl Default for PortManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PortManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// 创建会话并启动读线程；立即返回会话 ID。链路在读线程内打开（open_transport），
    /// 打开失败通过 sink.error 回报，状态从 connecting -> connected/error。
    /// `sessions_dir`：无自定义路径模板时默认日志文件的落盘目录；传空 PathBuf 表示不落盘（CLI 缺省）。
    pub fn connect(
        &self,
        config: PortConfig,
        log_config: logfmt::LogConfig,
        sink: Arc<dyn EventSink>,
        sessions_dir: PathBuf,
    ) -> anyhow::Result<String> {
        let id = format!("s{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let stop = Arc::new(AtomicBool::new(false));
        // 会话运行态五件套（ring/状态/自动回复/告警/捕获）收拢在 SessionRuntime：
        // 运行状态起点 Connecting（读线程 open_recording 时同步上报 connecting 事件）
        let runtime = Arc::new(SessionRuntime::new());
        let plot = Arc::new(RwLock::new(PlotConfig::default()));
        let (write_tx, write_rx) = mpsc::channel::<super::PortCmd>();
        // 日志路径：模板非空则按当时时间+端口名+主机地址解析（token 替换值做文件名清洗），
        // 否则用默认 sessions_dir 路径
        let host = host_of(&config);
        let (log_path, custom_path) = match log_config
            .log_path_template
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            Some(tmpl) => (
                PathBuf::from(logfmt::format_path(
                    tmpl,
                    &chrono::Local::now(),
                    &config.name,
                    &host,
                )),
                true,
            ),
            // 空 sessions_dir（CLI 缺省不落盘）：空路径交给 RecordingController 跳过录制
            None if sessions_dir.as_os_str().is_empty() => (PathBuf::new(), false),
            None => (default_log_path(&sessions_dir, &id), false),
        };
        let ts_format = log_config.line_ts_format.filter(|s| !s.is_empty());
        let midnight_rotate = log_config.midnight_rotate.unwrap_or(false);

        let cfg = config.clone();
        let id2 = id.clone();
        let stop2 = stop.clone();
        let rt2 = runtime.clone();
        // 现场档案目录：不落盘（CLI）时为空路径，读线程内据此跳过捕获
        let captures_dir = if sessions_dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            sessions_dir.join("captures")
        };
        let alerts_mirror = Arc::new(RwLock::new(Vec::new()));
        let cdir2 = captures_dir.clone();
        let am2 = alerts_mirror.clone();
        let lp = log_path.clone();
        // ls = 与 SessionHandle 共享的当前路径单元（RecordingController 同持）
        let log_shared = Arc::new(RwLock::new(log_path.clone()));
        let ls = log_shared.clone();

        let handle = thread::Builder::new()
            .name(format!("reader-{}", id))
            .spawn(move || {
                session_thread(
                    cfg,
                    id2,
                    sink,
                    stop2,
                    write_rx,
                    lp,
                    ls,
                    ts_format,
                    custom_path,
                    midnight_rotate,
                    rt2,
                    cdir2,
                    am2,
                )
            })
            .map_err(|e| anyhow::anyhow!("spawn reader thread failed: {e}"))?;

        self.sessions.write().insert(
            id.clone(),
            SessionHandle {
                config,
                kind: SessionKind::Live,
                stop,
                write_tx,
                log_path: log_shared,
                log_base: log_path,
                join: Some(handle),
                runtime,
                plot,
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: alerts_mirror.clone(),
                annotations: Arc::new(RwLock::new(Vec::new())),
            },
        );
        Ok(id)
    }

    /// 创建离线会话：无端口、无读线程，仅把已解析的日志行灌入 ring 供 REST 桥分析。
    /// `send` 对离线会话直接报错；`clear_log` 直接清 ring。id 用 `o{N}` 前缀，与 live 的 `s{N}` 区分。
    /// 行经 ingest(Replay) 入库：更新 ring/计数并评估告警，但**零自动回复零捕获副作用**
    /// （Replay 不产生该决策；告警命中无 mirror/事件通道，静默丢弃与既有行为一致）。
    pub fn load_offline(&self, config: PortConfig, path: PathBuf, lines: Vec<LogLine>) -> String {
        let id = format!("o{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let runtime = Arc::new(SessionRuntime::new());
        // 离线会话无链路：恒 Offline，无错误
        {
            let mut st = runtime.state.write();
            st.status = SessionStatus::Offline;
            st.last_error = None;
        }
        for line in &lines {
            runtime.ingest(line, IngestOrigin::Replay, &crate::sink::NullSink);
        }
        let plot = Arc::new(RwLock::new(PlotConfig::default()));
        // rx 立即 drop -> 写通道天然断开；send() 对离线会话会先于此处早退报错。
        let (write_tx, _rx) = mpsc::channel::<super::PortCmd>();
        self.sessions.write().insert(
            id.clone(),
            SessionHandle {
                config,
                kind: SessionKind::Offline,
                stop: Arc::new(AtomicBool::new(false)),
                write_tx,
                log_path: Arc::new(RwLock::new(path.clone())),
                log_base: path,
                join: None,
                runtime,
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
            .send(super::PortCmd::Send(req))
            .map_err(|_| anyhow::anyhow!("发送通道已关闭"))?;
        Ok(())
    }

    /// 信号线控制（DTR/RTS）：照 send 模式投递到读线程串行执行；
    /// 网络源在读线程内报“无信号线”，离线会话直接拒绝。
    pub fn set_signal(&self, id: &str, pin: Pin, level: bool) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            return Err(anyhow::anyhow!("离线会话不可控制信号线"));
        }
        h.write_tx
            .send(super::PortCmd::Signal { pin, level })
            .map_err(|_| anyhow::anyhow!("通道已关闭"))?;
        Ok(())
    }

    pub fn clear_log(&self, id: &str) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            h.runtime.ring.clear();
            return Ok(());
        }
        h.write_tx
            .send(super::PortCmd::Clear)
            .map_err(|_| anyhow::anyhow!("通道已关闭"))?;
        Ok(())
    }

    /// 各 live 会话的Ring末行滞后快照（诊断心跳用）：(会话 id, 末行落后墙钟 ms, ring 长度, RX 行数)。
    /// 离线会话无读线程不参与；空 ring 返回 lag=0。
    pub fn perf_snapshot(&self) -> Vec<(String, u64, usize, u64)> {
        let now = super::now_ms();
        self.sessions
            .read()
            .iter()
            // 已停止的会话（用户点停止/断开但标签仍在）末行时间戳永远停在过去，
            // 计入只会产生随墙钟无限增长的假滞后噪音
            .filter(|(_, h)| matches!(h.kind, SessionKind::Live) && !h.stop.load(Ordering::Relaxed))
            .map(|(id, h)| {
                let (_, _, _, _, _, last_epoch, len) = h.runtime.ring.bounds();
                let lag = if last_epoch == 0 {
                    0
                } else {
                    now.saturating_sub(last_epoch)
                };
                (id.clone(), lag, len, h.runtime.ring.rx_lines())
            })
            .collect()
    }

    /// 游标补拉：返回 ring 中 `no > since_no` 的行（前端视图拉模型的数据通道）。
    /// `no` 单调递增且 clear 不回退——游标语义下不重不漏；二分定位 O(log n)。
    pub fn ring_lines_after_no(
        &self,
        id: &str,
        since_no: u64,
        max: usize,
    ) -> anyhow::Result<Vec<BridgeLine>> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        let max = max.clamp(1, RING_CAP);
        Ok(h.runtime.ring.lines_after_no(since_no, max))
    }

    /// 往前翻页补拉：返回 ring 中 `no < before_no` 的最新 max 行（升序）。
    /// 与 `ring_lines_after_no` 同一套 clamp 与会话守卫；前端视图缓冲裁掉旧行后回补用。
    pub fn ring_lines_before_no(
        &self,
        id: &str,
        before_no: u64,
        max: usize,
    ) -> anyhow::Result<Vec<BridgeLine>> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        let max = max.clamp(1, RING_CAP);
        Ok(h.runtime.ring.lines_before_no(before_no, max))
    }

    /// ring 现存行号边界（空环全 0）：前端判断「上滑还有没有旧行可回补」。
    pub fn ring_bounds(&self, id: &str) -> anyhow::Result<RingBounds> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        let (first_no, last_no, _, _, _, _, size) = h.runtime.ring.bounds();
        Ok(RingBounds {
            first_no,
            last_no,
            size,
            ring_cap: RING_CAP,
        })
    }

    /// 前端推送实时规则（自动回复/告警/捕获）：拉模型下评估在后端读线程，
    /// 规则变更与连接建立时由前端整体覆盖推送。
    pub fn set_live_rules(
        &self,
        id: &str,
        auto_reply: AutoReplyCfg,
        alerts: AlertCfg,
        capture: CaptureCfg,
    ) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        *h.runtime.auto_reply.write() = auto_reply;
        *h.runtime.alerts.write() = alerts;
        *h.runtime.capture.write() = capture;
        Ok(())
    }

    /// 会话日志文件完整路径（导出/打开日志位置用）。
    pub fn session_log_path(&self, id: &str) -> anyhow::Result<String> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        // 先落局部变量再构造 Ok：路径读守卫的临时值不能活到块尾（晚于 sessions 释放）
        let p = h.log_path.read().clone().to_string_lossy().into_owned();
        Ok(p)
    }

    /// 日志分段（「分段」按钮）：关闭当前日志文件，从当前时刻另起带时间戳的
    /// 新文件继续落盘，旧文件保留；录制暂停中调用会顺带恢复录制。
    /// 返回新文件完整路径。
    pub fn rotate_log(&self, id: &str) -> anyhow::Result<String> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            return Err(anyhow::anyhow!("离线会话不落盘"));
        }
        if h.log_base.as_os_str().is_empty() {
            return Err(anyhow::anyhow!("该会话未启用日志落盘"));
        }
        let np = next_segment_path(&h.log_base, &chrono::Local::now(), |p| p.exists());
        h.write_tx
            .send(super::PortCmd::RecOn(np.clone()))
            .map_err(|_| anyhow::anyhow!("通道已关闭（会话未连接）"))?;
        // 「打开日志」与下次分段都应基于新文件：只锁路径单元、不取 sessions 写锁
        //（读线程午夜轮转同样只写该单元，锁序恒为 sessions -> log_path，无死锁面）
        *h.log_path.write() = np.clone();
        Ok(np.to_string_lossy().into_owned())
    }

    /// 落盘录制开关（「录制」按钮）：关=暂停写文件（数据仍进 ring/视图）；
    /// 开=另起新分段文件继续录制。
    pub fn set_recording(&self, id: &str, on: bool) -> anyhow::Result<()> {
        if on {
            return self.rotate_log(id).map(|_| ());
        }
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if matches!(h.kind, SessionKind::Offline) {
            return Err(anyhow::anyhow!("离线会话不落盘"));
        }
        h.write_tx
            .send(super::PortCmd::RecOff)
            .map_err(|_| anyhow::anyhow!("通道已关闭（会话未连接）"))
    }

    // ===== REST 桥访问器（同 crate 读取，不暴露 SessionHandle） =====

    pub fn bridge_list(&self) -> Vec<SessionSnap> {
        let s = self.sessions.read();
        s.iter()
            .map(|(id, h)| {
                // 共享运行状态是唯一事实源：connecting/error 中间态与 last_error 来自
                // 读线程，不再从 stop 标志推断（推不出中间态，错误期还会假报 connected）
                let st = h.runtime.state.read().clone();
                SessionSnap {
                    id: id.clone(),
                    config: h.config.clone(),
                    status: st.status.as_str().into(),
                    last_error: st.last_error,
                    line_count: h.runtime.ring.len(),
                    ring_cap: RING_CAP,
                }
            })
            .collect()
    }

    pub fn bridge_snapshot(&self, id: &str) -> Option<Vec<BridgeLine>> {
        self.sessions
            .read()
            .get(id)
            .map(|h| h.runtime.ring.snapshot())
    }

    /// 长轮询：返回 `no > since` 的行 + 当前 `lastNo`（无会话返回 None）。
    pub fn bridge_follow(&self, id: &str, since: u64) -> Option<(Vec<BridgeLine>, u64)> {
        self.sessions
            .read()
            .get(id)
            .map(|h| (h.runtime.ring.lines_since(since), h.runtime.ring.last_no()))
    }

    /// 交换基线：只取当前 lastNo（不做全量行分配）。
    pub fn bridge_last_no(&self, id: &str) -> Option<u64> {
        self.sessions
            .read()
            .get(id)
            .map(|h| h.runtime.ring.last_no())
    }

    pub fn bridge_stats(&self, id: &str) -> Option<BridgeStats> {
        let s = self.sessions.read();
        s.get(id).map(|h| {
            let (first_no, last_no, first_ts, last_ts, first_epoch, last_epoch, size) =
                h.runtime.ring.bounds();
            BridgeStats {
                rx_lines: h.runtime.ring.rx_lines(),
                tx_lines: h.runtime.ring.tx_lines(),
                rx_bytes: h.runtime.ring.rx_bytes(),
                tx_bytes: h.runtime.ring.tx_bytes(),
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
        self.sessions.read().get(id).map(|h| h.plot.read().clone())
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
        self.sessions
            .read()
            .get(id)
            .map(|h| h.alerts.read().clone())
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

#[cfg(test)]
mod tests {
    //! manager 侧单测：编排访问器（perf/桥镜像/游标）与假传输（loopback TCP）集成。
    //! （状态机/ingest 单测在 serial/runtime.rs，录制/捕获/传输契约测试在各自模块。）
    use std::time::{Duration, Instant};

    use super::*;

    fn mk_log(
        ts: &str,
        dir: super::super::port::Dir,
        text: &str,
        bytes: Option<Vec<u8>>,
        epoch: u64,
    ) -> LogLine {
        LogLine {
            ts: ts.into(),
            dir,
            text: text.into(),
            bytes,
            epoch_millis: epoch,
        }
    }

    #[test]
    fn perf_snapshot_skips_stopped_and_offline() {
        let m = PortManager::new();
        let mk_handle = |stopped: bool| {
            let runtime = Arc::new(SessionRuntime::new());
            runtime.ingest(
                &mk_log("t", super::super::port::Dir::Rx, "x", None, 1000),
                IngestOrigin::Transport,
                &crate::sink::NullSink,
            );
            let stop = Arc::new(AtomicBool::new(stopped));
            let (tx, _rx) = mpsc::channel();
            SessionHandle {
                config: PortConfig::default(),
                kind: SessionKind::Live,
                stop,
                write_tx: tx,
                log_path: Arc::new(RwLock::new(PathBuf::from("x.log"))),
                log_base: PathBuf::from("x.log"),
                join: None,
                runtime,
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

    #[test]
    fn bridge_last_no_reads_ring_cursor_without_full_snapshot() {
        let m = PortManager::new();
        // 未知会话 None
        assert_eq!(m.bridge_last_no("nope"), None);
        let id = m.load_offline(
            PortConfig::default(),
            PathBuf::from("x.log"),
            vec![
                mk_log("01:00:00.000", super::super::port::Dir::Rx, "a", None, 1000),
                mk_log("02:00:00.000", super::super::port::Dir::Tx, "b", None, 2000),
            ],
        );
        // /exchange 基线：仅游标值（与 buf.last_no 一致），不分配快照
        assert_eq!(m.bridge_last_no(&id), Some(2));
    }

    // ============ 集成：假传输（本机 loopback TCP，不开硬件、不固定 sleep 轮询）============

    use crate::sink::VecSink;

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
            thread::sleep(Duration::from_millis(10));
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
        assert!(err.contains("建立"), "错误消息应说明建链失败: {err}");
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
        assert_eq!(snap.last_error.as_deref(), Some("网络连接已断开"));
        let events = sink.0.lock().clone();
        assert!(
            events
                .iter()
                .any(|e| *e == format!("error {id} 网络连接已断开")),
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

    #[test]
    fn session_state_offline_remains_offline() {
        let m = PortManager::new();
        let id = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
        let snap = m
            .bridge_list()
            .into_iter()
            .find(|s| s.id == id)
            .expect("离线会话在列表");
        assert_eq!(snap.status, "offline");
        assert_eq!(snap.last_error, None);
    }
}
