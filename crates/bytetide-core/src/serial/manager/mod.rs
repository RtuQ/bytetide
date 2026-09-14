//! 会话编排层：PortManager 建 runtime → 开链路（读线程内）→ spawn/join 线程 →
//! 路由命令 → 查询访问器。Stage 2 Task 3 后本文件不再含链路分支（transport/）、
//! 落盘录制与路径命名（recording.rs）、现场捕获（capture.rs）、读循环与行评估
//! （runtime.rs 的 session_thread/stream_loop/ingest）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use parking_lot::{Mutex, RwLock};

use super::port::{LogLine, PortConfig};
use super::recording::{default_log_path, next_segment_path};
use super::rules::{AlertCfg, AutoReplyCfg, CaptureCfg};
use super::runtime::{session_thread, IngestOrigin};
use super::transport::host_of;
use crate::logfmt;
use crate::offline::{open_offline, OfflineIndex, OfflineReader};
use crate::replay::{spawn_replay, ReplayCmd, ReplayConfig, ReplayState};
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

/// 会话类型：实时串口 / 离线加载的日志文件 / 时序回放（离线日志按原时间差重放）。
enum SessionKind {
    Live,
    Offline,
    /// 时序回放：ring 实时灌入（ingest Replay origin），无链路、无落盘、
    /// 断开语义对齐 Offline（send/信号线/落盘拒绝，见各方法守卫）。
    Replay,
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
    /// 离线分页读取器（`load_offline_indexed` 会话独有）：查询按页直读源文件
    /// （虚拟 ring，ring 恒空）。live 会话与旧全量 `load_offline` 会话为 None。
    offline: Option<Arc<Mutex<OfflineReader>>>,
    /// 回放控制通道（仅 Replay 会话；disconnect 显式发 Stop——调用方仍持
    /// start_replay 返回的 sender 克隆，仅 drop 不保证关通道）。
    replay_tx: Option<mpsc::Sender<ReplayCmd>>,
}

pub struct PortManager {
    sessions: RwLock<HashMap<String, SessionHandle>>,
    /// 停止墓碑（两阶段关闭的第 1 阶段）：`disconnect` 移除会话句柄后，ring 的
    /// 只读副本留在此处，供前端「最终补拉」取走停止前最后一批尚未拉取的行
    /// （正常拉取周期 200ms + 渲染进程被节流时更久——没有墓碑时这批数据在
    /// 句柄删除瞬间即不可达，违反「停止仅断开连接，保留标签页与日志」的约定）。
    /// 前端拉空后调 [`Self::release_dead`] 显式释放（第 2 阶段）；未释放的按
    /// FIFO 容量上限淘汰兜底。仅 [`Self::ring_lines_after_no`] 路由到此。
    dead_rings: Mutex<VecDeque<(String, Arc<SessionRuntime>)>>,
    next_id: AtomicU64,
}

/// 墓碑容量：覆盖「连停数个会话」的补拉窗口即可，防无释放时内存常驻
/// （每份 ring ≤ RING_CAP ≈ 17MB）。
const DEAD_RING_CAP: usize = 4;

impl Default for PortManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PortManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            dead_rings: Mutex::new(VecDeque::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// 释放停止墓碑（两阶段关闭第 2 阶段；未知 id 静默——FIFO 淘汰/重复释放幂等）。
    pub fn release_dead(&self, id: &str) {
        self.dead_rings.lock().retain(|(did, _)| did != id);
    }

    fn park_dead(&self, id: &str, runtime: &Arc<SessionRuntime>) {
        let mut dead = self.dead_rings.lock();
        dead.push_back((id.to_string(), runtime.clone()));
        while dead.len() > DEAD_RING_CAP {
            dead.pop_front();
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
                offline: None,
                replay_tx: None,
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
                offline: None,
                replay_tx: None,
            },
        );
        id
    }

    /// 创建离线分页会话（Stage 2 Task 8）：core 建稀疏索引（每 4096 数据行记字节
    /// 偏移）后 ring 保持为空——`ring_lines_after_no`/`ring_lines_before_no`/
    /// `bridge_snapshot`/`bridge_follow`/`bridge_last_no`/`bridge_stats`/
    /// `ring_bounds` 对该会话改走 `OfflineReader` 按页直读源文件（虚拟 ring），
    /// 不把全文件灌进 ring。行 no 语义与现离线会话一致：数据行按序连续分配、
    /// 首行 no=1（同 `RingBuf::push`）。`log_path` 指向源文件（「打开日志」/导出）。
    /// 前端不再全量解析推后端；旧 `load_offline`（前端已解析行整包灌 ring）保留。
    pub fn load_offline_indexed(
        &self,
        config: PortConfig,
        path: PathBuf,
    ) -> anyhow::Result<(String, OfflineIndex)> {
        let (index, reader) = open_offline(&path)?;
        let id = format!("o{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let runtime = Arc::new(SessionRuntime::new());
        // 离线会话无链路：恒 Offline，无错误
        {
            let mut st = runtime.state.write();
            st.status = SessionStatus::Offline;
            st.last_error = None;
        }
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
                plot: Arc::new(RwLock::new(PlotConfig::default())),
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
                offline: Some(Arc::new(Mutex::new(reader))),
                replay_tx: None,
            },
        );
        Ok((id, index))
    }

    /// 创建回放会话（id 前缀 `r`）：open_offline 建稀疏索引 → spawn_replay 按相邻行
    /// 原始时间差重放进 ring（IngestOrigin::Replay：告警评估、零自动回复零捕获）。
    /// 无链路无落盘：write_tx=断开通道占位（断开语义对齐 Offline），`sessions_dir`
    /// 预留；log_path/log_base=源文件。速度非法在执行前报错。守卫面：send/信号线/
    /// 落盘报「回放会话不支持…」，set_live_rules/clear_log 允许（见各方法）。
    pub fn start_replay(
        &self,
        path: &Path,
        replay_config: ReplayConfig,
        sink: Arc<dyn EventSink>,
        _sessions_dir: PathBuf,
    ) -> anyhow::Result<(String, mpsc::Sender<ReplayCmd>)> {
        self.start_replay_indexed(path, replay_config, sink, _sessions_dir)
            .map(|(id, tx, _)| (id, tx))
    }

    /// [`Self::start_replay`] 的完整版：同时返回离线索引摘要（行数与首末行 epoch
    /// 毫秒——前端回放工具条的总行数/时长来源，T7 命令层入口）。
    pub fn start_replay_indexed(
        &self,
        path: &Path,
        replay_config: ReplayConfig,
        sink: Arc<dyn EventSink>,
        _sessions_dir: PathBuf,
    ) -> anyhow::Result<(String, mpsc::Sender<ReplayCmd>, OfflineIndex)> {
        // 非法速度执行前失败（不建会话、不 spawn 线程）
        replay_config
            .validate()
            .map_err(|e| anyhow::anyhow!("回放配置非法: {e}"))?;
        let (index, reader) = open_offline(path)?;
        let id = format!("r{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let runtime = Arc::new(SessionRuntime::new());
        // 回放线程起跑即置 Running/connected；Ready 只覆盖 spawn 前的瞬态
        *runtime.replay_state.write() = Some(ReplayState::Ready);
        let (cmd_tx, cmd_rx) = mpsc::channel::<ReplayCmd>();
        // rx 立即 drop -> 写通道天然断开（断开语义对齐 Offline：send 早退守卫在前）
        let (write_tx, _rx) = mpsc::channel::<super::PortCmd>();
        // spawn 失败转 anyhow（与 connect 的 reader 线程同错误策略，不 panic）；
        // 失败时未登记会话，runtime/reader 随局部变量丢弃
        let join = spawn_replay(
            reader,
            runtime.clone(),
            replay_config,
            cmd_rx,
            sink,
            id.clone(),
        )
        .map_err(|e| anyhow::anyhow!("spawn replay thread failed: {e}"))?;
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.sessions.write().insert(
            id.clone(),
            SessionHandle {
                config: PortConfig {
                    name,
                    ..PortConfig::default()
                },
                kind: SessionKind::Replay,
                stop: Arc::new(AtomicBool::new(false)),
                write_tx,
                log_path: Arc::new(RwLock::new(path.to_path_buf())),
                log_base: path.to_path_buf(),
                join: Some(join),
                runtime,
                plot: Arc::new(RwLock::new(PlotConfig::default())),
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
                offline: None,
                replay_tx: Some(cmd_tx.clone()),
            },
        );
        Ok((id, cmd_tx, index))
    }

    /// 回放控制命令投递（仅 Replay 会话；非回放/不存在报稳定错误）。命令异步生效
    /// （runner ≤50ms 排水），调用方经 [`Self::replay_view`] 轮询到位。
    /// T7 命令层入口：SessionHandle 已持有控制通道（T6），此处只补公开路由面。
    pub fn replay_control(&self, id: &str, cmd: ReplayCmd) -> anyhow::Result<()> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if !matches!(h.kind, SessionKind::Replay) {
            return Err(anyhow::anyhow!("非回放会话"));
        }
        let tx = h
            .replay_tx
            .clone()
            .ok_or_else(|| anyhow::anyhow!("回放控制通道不可用"))?;
        drop(sessions);
        tx.send(cmd)
            .map_err(|_| anyhow::anyhow!("回放控制通道已关闭"))?;
        Ok(())
    }

    /// 回放控制面视图：细粒度状态 + 当前文件行号水位（最后已 ingest 的源文件行；
    /// seek 后未恢复=目标-1，loop 回卷=0）。非回放会话/不存在返回 None
    /// （T7 命令层据此报「会话不存在或非回放会话」）。
    pub fn replay_view(&self, id: &str) -> Option<(ReplayState, u64)> {
        let sessions = self.sessions.read();
        let h = sessions.get(id)?;
        if !matches!(h.kind, SessionKind::Replay) {
            return None;
        }
        let state = (*h.runtime.replay_state.read())?;
        let line = *h.runtime.replay_cursor.read();
        Some((state, line))
    }

    pub fn disconnect(&self, id: &str) -> anyhow::Result<()> {
        let mut handle = self
            .sessions
            .write()
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        handle.stop.store(true, Ordering::Relaxed);
        // 回放会话：显式 Stop（start_replay 返回的 sender 克隆仍在调用方手里，
        // 仅 drop 不保证关通道）；线程已退出时发送失败静默忽略
        if let Some(tx) = handle.replay_tx.take() {
            let _ = tx.send(ReplayCmd::Stop);
        }
        if let Some(join) = handle.join.take() {
            let _ = join.join();
        }
        // 两阶段关闭第 1 阶段：线程已停、ring 定格，留墓碑供最终补拉
        self.park_dead(id, &handle.runtime);
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
        if matches!(h.kind, SessionKind::Replay) {
            return Err(anyhow::anyhow!("回放会话不支持发送"));
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
        if matches!(h.kind, SessionKind::Replay) {
            return Err(anyhow::anyhow!("回放会话不支持信号线"));
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
        // 离线（镜像遗忘 / ring 清屏）与回放（ring 清屏、seq 不回退、源文件与
        // 回放游标不动）：都不经 PortCmd——写通道是断开占位
        if matches!(h.kind, SessionKind::Offline | SessionKind::Replay) {
            match &h.offline {
                // 分页离线会话：虚拟 ring 清屏=后端镜像遗忘（后续查询全空，
                // no 游标语义/计数器与 RingBuf::clear 一致；源文件不动）
                Some(r) => r.lock().clear(),
                // 离线全量 / 回放会话：ring 清屏（seq 不回退，回放游标不动）
                None => h.runtime.ring.clear(),
            }
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

    /// 会话模式（场景启动守卫用，Stage 3 Task 3）：live 可跑场景；offline/replay
    /// 与不存在分别返回对应值——`None`=会话不存在。
    pub fn session_mode(&self, id: &str) -> Option<&'static str> {
        self.sessions.read().get(id).map(|h| match h.kind {
            SessionKind::Live => "live",
            SessionKind::Offline => "offline",
            SessionKind::Replay => "replay",
        })
    }

    /// 游标补拉：返回 ring 中 `no > since_no` 的行（前端视图拉模型的数据通道）。
    /// `no` 单调递增且 clear 不回退——游标语义下不重不漏；二分定位 O(log n)。
    /// 离线分页会话改走 `OfflineReader` 页读（no 连续：第 i 行 no=since_no+1+i）。
    /// 已停止会话路由到停止墓碑（两阶段关闭的最终补拉窗口，见
    /// [`Self::release_dead`]）；墓碑亦未命中才报「会话不存在」。
    pub fn ring_lines_after_no(
        &self,
        id: &str,
        since_no: u64,
        max: usize,
    ) -> anyhow::Result<Vec<BridgeLine>> {
        let max = max.clamp(1, RING_CAP);
        {
            let sessions = self.sessions.read();
            if let Some(h) = sessions.get(id) {
                return Ok(match &h.offline {
                    Some(r) => r.lock().lines_after(since_no, max)?,
                    None => h.runtime.ring.lines_after_no(since_no, max),
                });
            }
        }
        let dead = self.dead_rings.lock();
        let runtime = dead
            .iter()
            .find(|(did, _)| did == id)
            .map(|(_, rt)| rt)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        Ok(runtime.ring.lines_after_no(since_no, max))
    }

    /// 按行号精确读单行（会话缺失 Err；行不存在 `Ok(None)`；`no=0` 恒 None）。
    /// ring/离线文件的 no 都按序连续 → `lines_after(no-1, 1)` 一步定位，供
    /// REST `/lines?no=` 与批注回填做有界读取（评审 P1-1：不物化全量快照）。
    pub fn bridge_line_by_no(&self, id: &str, no: u64) -> anyhow::Result<Option<BridgeLine>> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if no == 0 {
            return Ok(None);
        }
        let line = match &h.offline {
            Some(r) => r.lock().lines_after(no - 1, 1)?.into_iter().next(),
            None => h.runtime.ring.lines_after_no(no - 1, 1).into_iter().next(),
        };
        Ok(line.filter(|l| l.no == no))
    }

    /// 往前翻页补拉：返回 ring 中 `no < before_no` 的最新 max 行（升序）。
    /// 与 `ring_lines_after_no` 同一套 clamp 与会话守卫；前端视图缓冲裁掉旧行后回补用。
    /// 离线分页会话改走 `OfflineReader` 页读（虚拟 ring 的 no<before 最新窗口）。
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
        Ok(match &h.offline {
            Some(r) => r.lock().lines_before(before_no, max)?,
            None => h.runtime.ring.lines_before_no(before_no, max),
        })
    }

    /// ring 现存行号边界（空环全 0）：前端判断「上滑还有没有旧行可回补」。
    /// 离线分页会话返回虚拟 ring 边界（首行 no=1、末行 no=line_count）。
    pub fn ring_bounds(&self, id: &str) -> anyhow::Result<RingBounds> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        Ok(match &h.offline {
            Some(r) => {
                let (first_no, last_no, .., size) = r.lock().bounds();
                RingBounds {
                    first_no,
                    last_no,
                    size,
                    ring_cap: RING_CAP,
                }
            }
            None => {
                let (first_no, last_no, _, _, _, _, size) = h.runtime.ring.bounds();
                RingBounds {
                    first_no,
                    last_no,
                    size,
                    ring_cap: RING_CAP,
                }
            }
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

    /// 回放会话的细粒度回放状态（Ready/Running/Paused/Finished/Stopped/Error；
    /// 非回放会话或会话不存在返回 None）。
    pub fn replay_state(&self, id: &str) -> Option<ReplayState> {
        let sessions = self.sessions.read();
        let h = sessions.get(id)?;
        // 先落守卫再解引用：读守卫的临时值不能活到表达式尾（同 session_log_path）
        let state = h.runtime.replay_state.read();
        *state
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
        if matches!(h.kind, SessionKind::Replay) {
            return Err(anyhow::anyhow!("回放会话不支持落盘"));
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
        if matches!(h.kind, SessionKind::Replay) {
            return Err(anyhow::anyhow!("回放会话不支持落盘"));
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
                    // 离线分页会话：line_count=文件数据行数（ring 恒空，不能报 0）
                    line_count: match &h.offline {
                        Some(r) => r.lock().size(),
                        None => h.runtime.ring.len(),
                    },
                    ring_cap: RING_CAP,
                }
            })
            .collect()
    }

    pub fn bridge_snapshot(&self, id: &str) -> Option<Vec<BridgeLine>> {
        self.sessions.read().get(id).map(|h| match &h.offline {
            // 离线分页会话：快照=分页走完整个文件（REST 显式调用；io 失败退空）
            Some(r) => r.lock().snapshot().unwrap_or_default(),
            None => h.runtime.ring.snapshot(),
        })
    }

    /// 长轮询：返回 `no > since` 的行 + 当前 `lastNo`（无会话返回 None）。
    pub fn bridge_follow(&self, id: &str, since: u64) -> Option<(Vec<BridgeLine>, u64)> {
        self.sessions.read().get(id).map(|h| match &h.offline {
            Some(r) => r.lock().follow(since).unwrap_or((Vec::new(), 0)),
            None => (h.runtime.ring.lines_since(since), h.runtime.ring.last_no()),
        })
    }

    /// 交换基线：只取当前 lastNo（不做全量行分配）。
    pub fn bridge_last_no(&self, id: &str) -> Option<u64> {
        self.sessions.read().get(id).map(|h| match &h.offline {
            Some(r) => r.lock().last_no(),
            None => h.runtime.ring.last_no(),
        })
    }

    pub fn bridge_stats(&self, id: &str) -> Option<BridgeStats> {
        let s = self.sessions.read();
        s.get(id).map(|h| match &h.offline {
            Some(r) => {
                let r = r.lock();
                let (first_no, last_no, first_ts, last_ts, first_epoch, last_epoch, size) =
                    r.bounds();
                let (rx_lines, tx_lines, rx_bytes, tx_bytes) = r.dir_counters();
                BridgeStats {
                    rx_lines,
                    tx_lines,
                    rx_bytes,
                    tx_bytes,
                    first_no,
                    last_no,
                    first_ts,
                    last_ts,
                    first_epoch,
                    last_epoch,
                    ring_cap: RING_CAP,
                    size,
                }
            }
            None => {
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
            if let Some(tx) = h.replay_tx.take() {
                let _ = tx.send(ReplayCmd::Stop);
            }
            if let Some(join) = h.join.take() {
                let _ = join.join();
            }
        }
    }
}

#[cfg(test)]
mod tests;
