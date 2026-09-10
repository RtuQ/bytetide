use std::collections::{HashMap, VecDeque};

// Link 为具体类型后 read/write_all 需要 trait 在作用域内（as _ 不引入名字冲突）
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::port::{open_port, Dir, LogLine, PortConfig};
use crate::logfmt;
use crate::session::SessionLog;
use crate::serial::rules::{
    alert_eval, auto_reply_payload, AlertCfg, AlertWinState, AutoReplyCfg, CaptureCfg,
    capture_eval, clamp_capture_window,
};
use crate::sink::{CaptureInfo, EventSink};

/// 桥接环形缓冲容量（带原始字节的近期分析窗口；≈170B/行 × 10 万 ≈ 17MB/会话）。
pub const RING_CAP: usize = 100000;

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

/// ring 现存行号边界（前端「翻页补旧行」判断还能不能往前翻）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RingBounds {
    pub first_no: u64,
    pub last_no: u64,
    pub size: usize,
    pub ring_cap: usize,
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
    pub fn push(&self, line: &LogLine) -> u64 {
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
        no
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

    /// 游标拉取：`no > since_no` 的最旧 max 行（no 单调递增，二分定位）。
    pub fn lines_after_no(&self, since_no: u64, max: usize) -> Vec<BridgeLine> {
        let ring = self.ring.lock();
        let from = ring.partition_point(|l| l.no <= since_no);
        ring.iter().skip(from).take(max).cloned().collect()
    }

    /// 往前翻页：`no < before_no` 的最新 max 行（视图缓冲裁掉旧行后从 ring 回补用，
    /// 仍按 no 升序返回；ring 已翻到最早行时返回不足 max 或空）。
    pub fn lines_before_no(&self, before_no: u64, max: usize) -> Vec<BridgeLine> {
        let ring = self.ring.lock();
        let end = ring.partition_point(|l| l.no < before_no);
        let start = end.saturating_sub(max);
        ring.iter().skip(start).take(end - start).cloned().collect()
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

    /// 取 epoch_ms >= since 的全部行（升序）。第二返回值=更早的行已被 ring
    /// 淘汰（发生过淘汰且现存首行晚于 since）——档案据此标注「前置现场可能缺失」；
    /// 会话刚开始、数据天然不足窗口不算缺失。
    fn lines_since_epoch(&self, since_epoch_ms: u64) -> (Vec<BridgeLine>, bool) {
        let ring = self.ring.lock();
        let missing_earlier = self.seq.load(Ordering::Relaxed) as usize > ring.len()
            && ring.front().is_some_and(|l| l.epoch_millis > since_epoch_ms);
        let (mut lo, mut hi) = (0usize, ring.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if ring.get(mid).is_some_and(|l| l.epoch_millis < since_epoch_ms) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let out: Vec<BridgeLine> = ring.iter().skip(lo).cloned().collect();
        (out, missing_earlier)
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

/// 信号线引脚（输出方向：DTR/RTS；CTS/DSR 等输入引脚读取后续再加）。
#[derive(Clone, Copy, Debug)]
pub enum Pin {
    Dtr,
    Rts,
}

/// 发往读线程的控制命令：发送数据 / 清屏（截断文件）/ 信号线控制。
pub enum PortCmd {
    Send(SendRequest),
    Clear,
    /// 日志落盘控制（录制开关/分段共用）：携带新分段完整路径，读线程内
    /// flush+关闭当前文件后从该路径另起新文件继续录制。分段= manager 计算好
    /// 带时间戳的路径后发此命令；恢复录制=同上。
    RecOn(PathBuf),
    /// 暂停落盘：flush+关闭当前文件（ring/视图不受影响，仅停止写文件）。
    RecOff,
    /// DTR/RTS 置位：串口链路直接写引脚，网络源在链路层报“无信号线”。
    Signal { pin: Pin, level: bool },
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
    /// 当前日志文件路径（「分段」/读线程午夜轮转后随最新分段更新；「打开日志」指向当前文件）。
    /// 共享单元（短临界区只护路径本身）：writer 的切换全部在读线程内串行（「分段」命令
    /// 也经 PortCmd 进读线程），此处仅登记最新路径；严禁持它的锁再去锁 sessions（无死锁面）。
    log_path: Arc<RwLock<PathBuf>>,
    /// 连接时解析出的基准路径：分段命名始终基于它，避免 stem 越叠越长
    log_base: PathBuf,
    join: Option<thread::JoinHandle<()>>,
    buf: Arc<RingBuf>,
    plot: Arc<RwLock<PlotConfig>>,
    /// 前端推送的书签/告警历史镜像（REST 只读；后端不产生、不校验内容）。
    bookmarks: Arc<RwLock<Vec<BridgeBookmark>>>,
    alerts: Arc<RwLock<Vec<BridgeAlert>>>,
    /// AI 批注（REST 写入 + 前端同步的双向镜像）。
    annotations: Arc<RwLock<Vec<BridgeAnnotation>>>,
    /// 自动回复规则（前端推送；读线程内评估并直接回写设备）
    auto_reply: Arc<RwLock<AutoReplyCfg>>,
    /// 告警规则（前端推送；读线程内评估，命中走 mirror + alert-hit 事件）
    alert_cfg: Arc<RwLock<AlertCfg>>,
    /// 触发式现场捕获配置（前端推送；读线程内逐行评估触发）
    capture_cfg: Arc<RwLock<CaptureCfg>>,
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
        let buf = Arc::new(RingBuf::new());
        let plot = Arc::new(RwLock::new(PlotConfig::default()));
        let (write_tx, write_rx) = mpsc::channel::<PortCmd>();
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
            // 空 sessions_dir（CLI 缺省不落盘）：空路径交给 open_session_log 跳过录制
            None if sessions_dir.as_os_str().is_empty() => (PathBuf::new(), false),
            None => (default_log_path(&sessions_dir, &id), false),
        };
        let ts_format = log_config.line_ts_format.filter(|s| !s.is_empty());
        let midnight_rotate = log_config.midnight_rotate.unwrap_or(false);

        let cfg = config.clone();
        let id2 = id.clone();
        let stop2 = stop.clone();
        let buf2 = buf.clone();
        let auto_reply_cfg = Arc::new(RwLock::new(AutoReplyCfg::default()));
        let alert_cfg = Arc::new(RwLock::new(AlertCfg::default()));
        let capture_cfg = Arc::new(RwLock::new(CaptureCfg::default()));
        // 现场档案目录：不落盘（CLI）时为空路径，读线程内据此跳过捕获
        let captures_dir = if sessions_dir.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            sessions_dir.join("captures")
        };
        let alerts_mirror = Arc::new(RwLock::new(Vec::new()));
        let ar2 = auto_reply_cfg.clone();
        let al2 = alert_cfg.clone();
        let cap2 = capture_cfg.clone();
        let cdir2 = captures_dir.clone();
        let am2 = alerts_mirror.clone();
        let lp = log_path.clone();
        // lb = 午夜分段的基准路径；ls = 与 SessionHandle 共享的当前路径单元
        let lb = log_path.clone();
        let log_shared = Arc::new(RwLock::new(log_path.clone()));
        let ls = log_shared.clone();

        let handle = thread::Builder::new()
            .name(format!("reader-{}", id))
            .spawn(move || {
                // 串口与 TCP/UDP 源共用同一装配路径，按传输类型选择循环
                if is_net_transport(&cfg) {
                    net_loop(
                        cfg, id2, sink, stop2, write_rx, lp, lb, midnight_rotate, ls, ts_format,
                        custom_path, buf2, ar2, al2, cap2, cdir2, am2,
                    )
                } else {
                    reader_loop(
                        cfg, id2, sink, stop2, write_rx, lp, lb, midnight_rotate, ls, ts_format,
                        custom_path, buf2, ar2, al2, cap2, cdir2, am2,
                    )
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
                log_path: log_shared,
                log_base: log_path,
                join: Some(handle),
                buf,
                plot,
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: alerts_mirror.clone(),
                annotations: Arc::new(RwLock::new(Vec::new())),
                auto_reply: auto_reply_cfg.clone(),
                alert_cfg: alert_cfg.clone(),
                capture_cfg,
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
                log_path: Arc::new(RwLock::new(path.clone())),
                log_base: path,
                join: None,
                buf,
                plot,
                bookmarks: Arc::new(RwLock::new(Vec::new())),
                alerts: Arc::new(RwLock::new(Vec::new())),
                annotations: Arc::new(RwLock::new(Vec::new())),
                auto_reply: Arc::new(RwLock::new(AutoReplyCfg::default())),
                alert_cfg: Arc::new(RwLock::new(AlertCfg::default())),
                capture_cfg: Arc::new(RwLock::new(CaptureCfg::default())),
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
            .send(PortCmd::Signal { pin, level })
            .map_err(|_| anyhow::anyhow!("通道已关闭"))?;
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
        Ok(h.buf.lines_after_no(since_no, max))
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
        Ok(h.buf.lines_before_no(before_no, max))
    }

    /// ring 现存行号边界（空环全 0）：前端判断「上滑还有没有旧行可回补」。
    pub fn ring_bounds(&self, id: &str) -> anyhow::Result<RingBounds> {
        let sessions = self.sessions.read();
        let h = sessions
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        let (first_no, last_no, _, _, _, _, size) = h.buf.bounds();
        Ok(RingBounds { first_no, last_no, size, ring_cap: RING_CAP })
    }

    /// 前端推送实时规则（自动回复/告警）：拉模型下评估在后端读线程，
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
        *h.auto_reply.write() = auto_reply;
        *h.alert_cfg.write() = alerts;
        *h.capture_cfg.write() = capture;
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
            .send(PortCmd::RecOn(np.clone()))
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
            .send(PortCmd::RecOff)
            .map_err(|_| anyhow::anyhow!("通道已关闭（会话未连接）"))
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

/// 默认日志路径：`sessions_dir/{id}.log`（桌面端传 app_data_dir()/sessions）。
fn default_log_path(sessions_dir: &std::path::Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{}.log", id))
}

/// `%S`（主机地址）实参：网络源 = host:port（tcp-client 目标 / tcp-server、udp 监听地址，
/// 缺省值与 `establish_link` 一致）；串口源 = 端口名（与 `%H` 相同）。
fn host_of(config: &PortConfig) -> String {
    match config.transport.as_deref() {
        Some("tcp-client") => format!(
            "{}:{}",
            config.tcp_host.clone().unwrap_or_else(|| "127.0.0.1".into()),
            config.tcp_port.unwrap_or(23)
        ),
        Some("tcp-server") => format!(
            "{}:{}",
            bind_host_of(config),
            config.tcp_port.unwrap_or(9000)
        ),
        Some("udp") => format!(
            "{}:{}",
            bind_host_of(config),
            config.udp_local_port.unwrap_or(0)
        ),
        _ => config.name.clone(),
    }
}

/// 分段日志路径：在基准路径扩展名前插入 `-YYYYMMDD-HHMMSS`；同秒内再次分段
/// 用 `-2`/`-3` 递增去重。`exists` 由调用方注入（真实 fs / 测试桩），保持纯函数可测。
fn next_segment_path(
    base: &std::path::Path,
    now: &chrono::DateTime<chrono::Local>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> PathBuf {
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let dir = base.parent().unwrap_or(std::path::Path::new(""));
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into());
    let ext = base.extension().map(|s| s.to_string_lossy().into_owned());
    let name = |n: Option<u32>| match (&ext, n) {
        (Some(e), Some(n)) => format!("{stem}-{stamp}-{n}.{e}"),
        (Some(e), None) => format!("{stem}-{stamp}.{e}"),
        (None, Some(n)) => format!("{stem}-{stamp}-{n}"),
        (None, None) => format!("{stem}-{stamp}"),
    };
    let mut cand = dir.join(name(None));
    let mut n = 2;
    while exists(&cand) {
        cand = dir.join(name(Some(n)));
        n += 1;
    }
    cand
}

/// 午夜分段判定：开关启用且本地日期相对上次检查已变更（跨天 / 时钟跳变）才成立。
fn segment_due(enabled: bool, last_date: chrono::NaiveDate, now_date: chrono::NaiveDate) -> bool {
    enabled && last_date != now_date
}

#[allow(clippy::too_many_arguments)]
fn reader_loop(
    config: PortConfig,
    session_id: String,
    sink: Arc<dyn EventSink>,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    log_path: PathBuf,
    log_base: PathBuf,
    midnight_rotate: bool,
    log_shared: Arc<RwLock<PathBuf>>,
    ts_format: Option<String>,
    custom_path: bool,
    ring: Arc<RingBuf>,
    auto_reply: Arc<RwLock<AutoReplyCfg>>,
    alert_cfg: Arc<RwLock<AlertCfg>>,
    capture_cfg: Arc<RwLock<CaptureCfg>>,
    captures_dir: PathBuf,
    alerts_mirror: Arc<RwLock<Vec<BridgeAlert>>>,
) {
    let (session_log, ts_fmt) =
        open_session_log(&*sink, &session_id, &log_path, custom_path, ts_format);

    let mut port = match open_port(&config) {
        Ok(p) => p,
        Err(e) => {
            sink.error(
                &session_id,
                &format!("打开串口 {} 失败: {}", config.name, e),
            );
            sink.status(&session_id, "error");
            return;
        }
    };
    sink.status(&session_id, "connected");

    let mut link = Link::Serial(port.as_mut());
    stream_loop(
        &mut link,
        "串口连接已断开",
        "写入串口失败",
        &*sink,
        &session_id,
        stop,
        write_rx,
        &ts_fmt,
        session_log,
        log_base,
        midnight_rotate,
        log_shared,
        ring,
        auto_reply,
        alert_cfg,
        capture_cfg,
        captures_dir,
        alerts_mirror,
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

fn sink_line(line: &LogLine, session_log: &mut Option<SessionLog>, ring: &RingBuf) -> u64 {
    if let Some(w) = session_log.as_mut() {
        w.append(line);
    }
    ring.push(line)
}

fn finish_loop(session_log: Option<SessionLog>, sink: &dyn EventSink, session_id: &str) {
    if let Some(mut w) = session_log {
        let _ = w.flush();
    }
    sink.status(session_id, "disconnected");
}

/// 打开会话日志文件并上报 connecting；打开失败仅在自定义路径时告警。
/// 返回 (session_log, ts_fmt)，供串口/网络两类循环共用。
fn open_session_log(
    sink: &dyn EventSink,
    session_id: &str,
    log_path: &std::path::Path,
    custom_path: bool,
    ts_format: Option<String>,
) -> (Option<SessionLog>, String) {
    // 空路径 = 不落盘（CLI 缺省）：不建文件、不改错误状态，仅上报 connecting
    if log_path.as_os_str().is_empty() {
        let ts_fmt = ts_format.unwrap_or_else(|| "%h:%m:%s.%t".to_string());
        sink.status(session_id, "connecting");
        return (None, ts_fmt);
    }
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let session_log = match SessionLog::create(log_path) {
        Ok(w) => Some(w),
        Err(e) => {
            if custom_path {
                sink.error(
                    session_id,
                    &format!("日志路径无效/不可写: {}: {}", log_path.display(), e),
                );
            }
            None
        }
    };
    let ts_fmt = ts_format.unwrap_or_else(|| "%h:%m:%s.%t".to_string());
    sink.status(session_id, "connecting");
    (session_log, ts_fmt)
}

/// stream_loop 的链路视图：串口（含信号线控制）或网络。合并为单一对象是因为
/// io 读写与 DTR/RTS 置位都唯一借用同一个 `Box<dyn SerialPort>`（无法同时
/// 传两个 `&mut`）；`stream_loop` 因此从泛型 `T: Read+Write` 改为具体 `Link`。
enum Link<'a> {
    Serial(&'a mut dyn serialport::SerialPort),
    Net(&'a mut NetLink),
}

impl std::io::Read for Link<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Link::Serial(p) => p.read(buf),
            Link::Net(l) => l.read(buf),
        }
    }
}

impl std::io::Write for Link<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Link::Serial(p) => p.write(buf),
            Link::Net(l) => l.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Link::Serial(p) => p.flush(),
            Link::Net(l) => l.flush(),
        }
    }
}

impl Link<'_> {
    fn set_signal(&mut self, pin: Pin, level: bool) -> std::io::Result<()> {
        // serialport 的引脚方法返回其自有 Result<T, serialport::Error> 别名，映射成 io::Error
        let map = |e: serialport::Error| std::io::Error::other(e.to_string());
        match self {
            Link::Serial(p) => match pin {
                Pin::Dtr => p.write_data_terminal_ready(level).map_err(map),
                Pin::Rts => p.write_request_to_send(level).map_err(map),
            },
            Link::Net(_) => Err(std::io::Error::other("网络源无信号线")),
        }
    }
}

// ---------- 触发式现场捕获（行车记录仪） ----------

/// 一次已触发的捕获：档案 writer + 后续窗口截止时刻。
/// writer 用 Option 承载创建失败（降级为不落盘但数据流不受影响）。
struct CaptureRun {
    writer: Option<SessionLog>,
    path: PathBuf,
    deadline_ms: u64,
    lines: u64,
    /// "keyword" | "alert" | "disconnect"
    trigger: &'static str,
    rule: String,
    at: u64,
}

/// 现场档案路径：{dir}/{id}-cap-YYYYMMDD-HHMMSS[-N].log（同秒冲突 -2/-3 递增，与分段规则一致）。
fn next_capture_path(
    dir: &std::path::Path,
    id: &str,
    now: &chrono::DateTime<chrono::Local>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> PathBuf {
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut n = 1u32;
    loop {
        let name = if n == 1 {
            format!("{id}-cap-{stamp}.log")
        } else {
            format!("{id}-cap-{stamp}-{n}.log")
        };
        let p = dir.join(name);
        if !exists(&p) {
            return p;
        }
        n += 1;
    }
}

/// 触发一次捕获：建档案、写头注释、把 ring 里 pre_ms 窗口的行回溯写入
///（含触发行本身——触发时它已入 ring）。创建失败仅 sink.error 不中断数据流。
fn capture_start(
    session_id: &str,
    cfg: &CaptureCfg,
    captures_dir: &std::path::Path,
    ring: &RingBuf,
    trigger: &'static str,
    rule: &str,
    at_ms: u64,
    sink: &dyn EventSink,
) -> Option<CaptureRun> {
    if captures_dir.as_os_str().is_empty() {
        return None;
    }
    let pre = clamp_capture_window(cfg.pre_ms);
    let post = clamp_capture_window(cfg.post_ms);
    let now = chrono::Local::now();
    let path = next_capture_path(captures_dir, session_id, &now, |p| p.exists());
    let mut writer = match SessionLog::create(&path) {
        Ok(w) => Some(w),
        Err(e) => {
            sink.error(
                session_id,
                &format!("现场档案创建失败 {}: {}", path.display(), e),
            );
            return None;
        }
    };
    let mut lines = 0u64;
    if let Some(w) = writer.as_mut() {
        // 头注释行：前端解析按 `#` 跳过，供人肉/工具辨识来源
        w.write_raw_line(&format!(
            "# bytetide-capture v1 trigger={trigger} rule={} at_ms={at_ms} at={}",
            rule.replace('\n', " ").replace('\r', " ").replace('\t', " "),
            now.format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
        ));
        let (snap, missing) = ring.lines_since_epoch(at_ms.saturating_sub(pre));
        if missing {
            w.write_raw_line("# 注意：更早的行已超出 ring 窗口，部分前置现场缺失");
        }
        for bl in &snap {
            w.append(&LogLine {
                ts: bl.ts.clone(),
                dir: bl.dir,
                text: bl.text.clone(),
                bytes: bl.bytes.clone(),
                epoch_millis: bl.epoch_millis,
            });
            lines += 1;
        }
        let _ = w.flush();
    }
    // armed 通知（稀疏）：前端据此点亮「捕获中」呼吸指示；capture_saved 即解除
    sink.capture_active(session_id, rule);
    Some(CaptureRun {
        writer,
        path,
        deadline_ms: at_ms.saturating_add(post),
        lines,
        trigger,
        rule: rule.to_string(),
        at: at_ms,
    })
}

/// armed 期间的每行追加 + 到期收尾（未 armed 为 no-op）。
fn cap_on_line(
    cap: &mut Option<CaptureRun>,
    line: &LogLine,
    sink: &dyn EventSink,
    session_id: &str,
    now: u64,
) {
    let Some(run) = cap.as_mut() else { return };
    if let Some(w) = run.writer.as_mut() {
        w.append(line);
    }
    run.lines += 1;
    if now >= run.deadline_ms {
        cap_finalize(cap, sink, session_id);
    }
}

/// 收尾：flush + capture_saved 稀疏事件（未 armed 为 no-op）。
fn cap_finalize(cap: &mut Option<CaptureRun>, sink: &dyn EventSink, session_id: &str) {
    let Some(mut run) = cap.take() else { return };
    if let Some(mut w) = run.writer.take() {
        let _ = w.flush();
    }
    sink.capture_saved(
        session_id,
        CaptureInfo {
            path: run.path.to_string_lossy().into_owned(),
            trigger: run.trigger.to_string(),
            rule: run.rule,
            lines: run.lines,
            at: run.at,
        },
    );
}

/// 逐行触发评估：关键词规则命中或告警联动即触发；armed 中再次命中顺延后续窗口
///（连续事故合并为一个档案）。
fn cap_maybe_arm(
    cap: &mut Option<CaptureRun>,
    cfg: &CaptureCfg,
    captures_dir: &std::path::Path,
    ring: &RingBuf,
    alert_hit: Option<&str>,
    line: &LogLine,
    sink: &dyn EventSink,
    session_id: &str,
) {
    if !cfg.enabled {
        return;
    }
    let hits = capture_eval(cfg, &line.text);
    let hit = hits
        .first()
        .map(|r| (r.pattern.as_str(), "keyword"))
        .or_else(|| alert_hit.map(|p| (p, "alert")));
    let Some((rule, trigger)) = hit else { return };
    let now = now_ms();
    match cap.as_mut() {
        Some(run) => run.deadline_ms = now.saturating_add(clamp_capture_window(cfg.post_ms)),
        None => {
            if let Some(run) =
                capture_start(session_id, cfg, captures_dir, ring, trigger, rule, now, sink)
            {
                *cap = Some(run);
            }
        }
    }
}

/// 串口与 TCP/UDP 共用的读循环：行切分、TX 回显、空闲半行刷出。
/// 数据不经事件推送（IPC 洪水会把消费者调度饿死）：行进 ring（消费方
/// 按 `no` 游标拉取）与落盘文件；只有状态/错误/低频事件走 sink。
#[allow(clippy::too_many_arguments)]
fn stream_loop(
    io: &mut Link,
    eof_msg: &str,
    write_err_msg: &str,
    sink: &dyn EventSink,
    session_id: &str,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    ts_fmt: &str,
    mut session_log: Option<SessionLog>,
    log_base: PathBuf,
    midnight_rotate: bool,
    log_shared: Arc<RwLock<PathBuf>>,
    ring: Arc<RingBuf>,
    auto_reply: Arc<RwLock<AutoReplyCfg>>,
    alert_cfg: Arc<RwLock<AlertCfg>>,
    capture_cfg: Arc<RwLock<CaptureCfg>>,
    captures_dir: PathBuf,
    alerts_mirror: Arc<RwLock<Vec<BridgeAlert>>>,
) {
    let mut buf = vec![0u8; 65536];
    let mut line_buf: Vec<u8> = Vec::new();
    let mut last_idle_flush = Instant::now();
    // 告警窗口/冷却状态（每会话独占，随读线程生灭）与待上报命中
    let mut alert_states: HashMap<String, AlertWinState> = HashMap::new();
    let mut fired_alerts: Vec<BridgeAlert> = Vec::new();
    // 触发式现场捕获：armed = 已触发、尚在写后续窗口
    let mut cap: Option<CaptureRun> = None;
    // 午夜自动分段：当前分段所属的本地日期 + 1 秒节流的检查时钟
    let mut seg_date = chrono::Local::now().date_naive();
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
                            let tx = LogLine {
                                ts: logfmt::format_ts(ts_fmt),
                                dir: Dir::Tx,
                                text: req.text.clone(),
                                bytes: None,
                                epoch_millis: now_ms(),
                            };
                            sink_line(&tx, &mut session_log, &ring);
                            // armed 捕获在后续窗口内：TX 回显一并入档
                            cap_on_line(&mut cap, &tx, sink, session_id, now_ms());
                        }
                        Err(_) => sink.error(session_id, write_err_msg),
                    }
                }
                PortCmd::Clear => {
                    if let Some(w) = session_log.as_mut() {
                        let _ = w.clear();
                    }
                    ring.clear();
                }
                PortCmd::RecOn(path) => {
                    // 分段/恢复录制：flush+关闭旧文件后另起新文件；创建失败则报错停写
                    //（ring/视图不受影响），下一条 RecOn 可再试
                    if let Some(mut w) = session_log.take() {
                        let _ = w.flush();
                    }
                    session_log = match SessionLog::create(&path) {
                        Ok(w) => Some(w),
                        Err(e) => {
                            sink.error(
                                session_id,
                                &format!("另起新日志失败 {}: {}", path.display(), e),
                            );
                            None
                        }
                    };
                }
                PortCmd::RecOff => {
                    if let Some(mut w) = session_log.take() {
                        let _ = w.flush();
                    }
                }
                PortCmd::Signal { pin, level } => {
                    if let Err(e) = io.set_signal(pin, level) {
                        sink.error(session_id, &format!("设置信号线失败: {e}"));
                    }
                }
            }
        }

        // 午夜自动分段（1 秒节流）：本地日期变更时无条件推进 seg_date；writer 在位才轮转
        //（录制关闭时不建文件，恢复录制本就会另起新文件）。与 RecOn/RecOff 同在读线程内
        // 串行切 writer，互斥天然成立；log_shared 只写路径单元，不碰 sessions 锁。
        if last_date_check.elapsed() >= Duration::from_secs(1) {
            last_date_check = Instant::now();
            let now = chrono::Local::now();
            if segment_due(midnight_rotate, seg_date, now.date_naive()) {
                seg_date = now.date_naive();
                if session_log.is_some() {
                    if let Some(mut w) = session_log.take() {
                        let _ = w.flush();
                    }
                    let np = next_segment_path(&log_base, &now, |p| p.exists());
                    session_log = match SessionLog::create(&np) {
                        Ok(w) => {
                            // 「打开日志」/REST 导出指向最新分段
                            *log_shared.write() = np.clone();
                            Some(w)
                        }
                        Err(e) => {
                            sink.error(
                                session_id,
                                &format!("午夜另起新日志失败 {}: {}", np.display(), e),
                            );
                            None
                        }
                    };
                }
            }
        }

        match io.read(&mut buf) {
            Ok(0) => {
                sink.error(session_id, eof_msg);
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
                        let ring_no = sink_line(&line, &mut session_log, &ring);
                        apply_rx_rules(
                            &line, ring_no, io, &mut session_log, &ring, ts_fmt,
                            &auto_reply.read().clone(), &alert_cfg.read().clone(),
                            &mut alert_states, &mut fired_alerts, &mut cap,
                            &capture_cfg.read().clone(), &captures_dir,
                            sink, session_id, write_err_msg,
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
                    let ring_no = sink_line(&line, &mut session_log, &ring);
                    apply_rx_rules(
                        &line, ring_no, io, &mut session_log, &ring, ts_fmt,
                        &auto_reply.read().clone(), &alert_cfg.read().clone(), &mut alert_states,
                        &mut fired_alerts, &mut cap, &capture_cfg.read().clone(), &captures_dir,
                        sink, session_id, write_err_msg,
                    );
                    last_idle_flush = Instant::now();
                }
            }
            Err(e) => {
                sink.error(session_id, &format!("读取错误: {e}"));
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
        if let Some(run) = cap.as_ref() {
            if now_ms() >= run.deadline_ms {
                cap_finalize(&mut cap, sink, session_id);
            }
        }
    }

    // 断连现场：会话结束前把最后 pre_ms 窗口抓成档案（设备重启/掉线现场最珍贵）
    {
        let cfg = capture_cfg.read().clone();
        if cap.is_none() && cfg.enabled && cfg.on_disconnect {
            let now = now_ms();
            cap = capture_start(
                session_id, &cfg, &captures_dir, &ring, "disconnect", "断连", now, sink,
            );
        }
    }
    cap_finalize(&mut cap, sink, session_id);
    finish_loop(session_log, sink, session_id);
}

/// 逐 RX 行规则评估：自动回复（读线程内直接回写设备，不依赖宿主存活）、
/// 告警（命中攒批上报）与现场捕获触发（关键词/告警联动；armed 时本行入档）。
/// 仅 RX 参与评估；TX 回显不受规则影响（armed 时照常入档）。
#[allow(clippy::too_many_arguments)]
fn apply_rx_rules(
    line: &LogLine,
    ring_no: u64,
    io: &mut Link,
    session_log: &mut Option<SessionLog>,
    ring: &RingBuf,
    ts_fmt: &str,
    auto_reply: &AutoReplyCfg,
    alert_cfg: &AlertCfg,
    alert_states: &mut HashMap<String, AlertWinState>,
    fired_alerts: &mut Vec<BridgeAlert>,
    cap: &mut Option<CaptureRun>,
    capture_cfg: &CaptureCfg,
    captures_dir: &std::path::Path,
    sink: &dyn EventSink,
    session_id: &str,
    write_err_msg: &str,
) {
    if line.dir != Dir::Rx {
        return;
    }
    let alerts_before = fired_alerts.len();
    // 自动回复：首条命中规则即回
    if let Some((payload, mode)) = auto_reply_payload(auto_reply, &line.text) {
        let bytes = if mode == "hex" {
            decode_hex(&payload)
        } else {
            payload.clone().into_bytes()
        };
        if !bytes.is_empty() {
            match io.write_all(&bytes) {
                Ok(()) => {
                    let tx = LogLine {
                        ts: logfmt::format_ts(ts_fmt),
                        dir: Dir::Tx,
                        text: payload,
                        bytes: None,
                        epoch_millis: now_ms(),
                    };
                    sink_line(&tx, session_log, ring);
                    // armed 捕获在后续窗口内：自动回复的 TX 也入档
                    cap_on_line(cap, &tx, sink, session_id, now_ms());
                }
                Err(_) => sink.error(session_id, write_err_msg),
            }
        }
    }
    // 告警：攒批（命中事件稀疏，不会形成 IPC 洪水）
    let now = now_ms();
    for rule in alert_eval(alert_cfg, alert_states, &line.text, now) {
        fired_alerts.push(BridgeAlert {
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
    // 现场捕获：关键词命中或本行告警联动触发；armed 时本行继续入档（快照已含触发行）
    let alert_hit = if fired_alerts.len() > alerts_before {
        fired_alerts.last().map(|a| a.pattern.as_str())
    } else {
        None
    };
    cap_maybe_arm(cap, capture_cfg, captures_dir, ring, alert_hit, line, sink, session_id);
    cap_on_line(cap, line, sink, session_id, now_ms());
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
    sink: Arc<dyn EventSink>,
    stop: Arc<AtomicBool>,
    write_rx: mpsc::Receiver<PortCmd>,
    log_path: PathBuf,
    log_base: PathBuf,
    midnight_rotate: bool,
    log_shared: Arc<RwLock<PathBuf>>,
    ts_format: Option<String>,
    custom_path: bool,
    ring: Arc<RingBuf>,
    auto_reply: Arc<RwLock<AutoReplyCfg>>,
    alert_cfg: Arc<RwLock<AlertCfg>>,
    capture_cfg: Arc<RwLock<CaptureCfg>>,
    captures_dir: PathBuf,
    alerts_mirror: Arc<RwLock<Vec<BridgeAlert>>>,
) {
    let desc = describe_transport(&config);
    let (session_log, ts_fmt) =
        open_session_log(&*sink, &session_id, &log_path, custom_path, ts_format);

    let mut link = match establish_link(&config) {
        Ok(l) => l,
        Err(e) => {
            sink.error(&session_id, &format!("建立 {desc} 失败: {e}"));
            sink.status(&session_id, "error");
            return;
        }
    };
    sink.status(&session_id, "connected");

    let mut net = Link::Net(&mut link);
    stream_loop(
        &mut net,
        "网络连接已断开",
        "网络写入失败",
        &*sink,
        &session_id,
        stop,
        write_rx,
        &ts_fmt,
        session_log,
        log_base,
        midnight_rotate,
        log_shared,
        ring,
        auto_reply,
        alert_cfg,
        capture_cfg,
        captures_dir,
        alerts_mirror,
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
        }        let nos: Vec<u64> = buf.snapshot().iter().map(|l| l.no).collect();
        assert_eq!(nos, vec![1, 2, 3]);
        assert_eq!(buf.last_no(), 3);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn lines_since_epoch_slices_and_reports_missing() {
        let buf = RingBuf::new();
        // 空 ring：空快照且不标缺失
        assert!(buf.lines_since_epoch(1).0.is_empty());
        for i in 0..5 {
            buf.push(&mk_log("00:00:00.001", Dir::Rx, &format!("l{i}"), None, 1000 + i * 10));
        }
        // since 落在首行之前：全量且无缺失
        let (snap, missing) = buf.lines_since_epoch(500);
        assert_eq!(snap.len(), 5);
        assert!(!missing);
        // since 命中第 3 行（epoch=1020）：取 1020..=1040 三行
        let (snap, missing) = buf.lines_since_epoch(1020);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].text, "l2");
        assert!(!missing);
        // since 早于全部行且 ring 未淘汰：全量、不标缺失
        let small = RingBuf::new();
        for i in 0..5 {
            small.push(&mk_log("00:00:00.001", Dir::Rx, &format!("s{i}"), None, 1000 + i * 10));
        }
        let (snap, missing) = small.lines_since_epoch(1000);
        assert_eq!(snap.len(), 5);
        assert!(!missing);
    }

    #[test]
    fn lines_since_epoch_flags_evicted_head() {
        // 真实淘汰：灌满 ring 溢出 5 行后，现存首行(=6)已晚于 since=1000
        // → 更早行被覆盖，标缺失
        let buf = RingBuf::new();
        let n = (RING_CAP + 5) as u64;
        for i in 0..n {
            buf.push(&mk_log("t", Dir::Rx, &format!("e{i}"), None, 1000 + i));
        }
        let (snap, missing) = buf.lines_since_epoch(1000);
        assert_eq!(snap.len(), RING_CAP);
        assert_eq!(snap[0].text, "e5");
        assert!(missing);
        // since 在现存首行之前：虽然发生过淘汰，但请求窗口内数据完整
        let (_, missing) = buf.lines_since_epoch(1000 + RING_CAP as u64);
        assert!(!missing);
    }

    #[test]
    fn next_capture_path_avoids_conflicts() {
        let dir = std::path::Path::new("/tmp");
        let now = chrono::Local::now();
        let taken: Vec<std::path::PathBuf> = vec![];
        let p1 = next_capture_path(dir, "s1", &now, |p| taken.contains(&p.to_path_buf()));
        assert_eq!(p1.file_name().unwrap().to_string_lossy(), format!("s1-cap-{}.log", now.format("%Y%m%d-%H%M%S")));
        let stamp = p1.clone();
        let p2 = next_capture_path(dir, "s1", &now, |p| p == &stamp);
        assert_eq!(p2.file_name().unwrap().to_string_lossy(), format!("s1-cap-{}-2.log", now.format("%Y%m%d-%H%M%S")));
        let stamp2 = p2.clone();
        let p3 = next_capture_path(dir, "s1", &now, |p| p == &stamp || p == &stamp2);
        assert!(p3.file_name().unwrap().to_string_lossy().ends_with("-3.log"));
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
    fn lines_after_no_cursor_pages() {
        // 拉模型游标：no>since 的最旧 max 行；游标推进不重不漏；翻页到拉空
        let buf = RingBuf::new();
        for i in 0..10 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let p1 = buf.lines_after_no(0, 4);
        assert_eq!(p1.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        let p2 = buf.lines_after_no(4, 4);
        assert_eq!(p2.iter().map(|l| l.no).collect::<Vec<_>>(), vec![5, 6, 7, 8]);
        let p3 = buf.lines_after_no(8, 4);
        assert_eq!(p3.iter().map(|l| l.no).collect::<Vec<_>>(), vec![9, 10]);
        // 拉空：游标已到最新
        assert!(buf.lines_after_no(10, 4).is_empty());
        // 游标超前（ring 淘汰/新会话）也安全
        assert!(buf.lines_after_no(999, 4).is_empty());
    }

    #[test]
    fn lines_after_no_survives_clear() {
        // 清屏 seq 单调不回退：游标保持原位，只拉新行
        let buf = RingBuf::new();
        for i in 0..5 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        buf.clear();
        assert!(buf.lines_after_no(5, 4).is_empty());
        buf.push(&mk_log("t", Dir::Rx, "new", None, 100));
        let after = buf.lines_after_no(5, 4);
        assert_eq!(after.iter().map(|l| l.no).collect::<Vec<_>>(), vec![6]);
        assert_eq!(after[0].text, "new");
    }

    #[test]
    fn lines_before_no_pages_backwards() {
        // 往前翻页：no<before 的最新 max 行，升序返回；翻到 ring 最早行返空
        let buf = RingBuf::new();
        for i in 0..10 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let p1 = buf.lines_before_no(10, 4);
        assert_eq!(p1.iter().map(|l| l.no).collect::<Vec<_>>(), vec![6, 7, 8, 9]);
        let p2 = buf.lines_before_no(6, 4);
        assert_eq!(p2.iter().map(|l| l.no).collect::<Vec<_>>(), vec![2, 3, 4, 5]);
        let p3 = buf.lines_before_no(2, 4);
        assert_eq!(p3.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1]);
        // 已翻到最早：安全返空；before_no 超前（比最新还大）取最新 max 行
        assert!(buf.lines_before_no(1, 4).is_empty());
        let p4 = buf.lines_before_no(999, 3);
        assert_eq!(p4.iter().map(|l| l.no).collect::<Vec<_>>(), vec![8, 9, 10]);
        assert!(buf.lines_before_no(0, 4).is_empty());
    }

    #[test]
    fn lines_before_no_empty_ring() {
        let buf = RingBuf::new();
        assert!(buf.lines_before_no(100, 4).is_empty());
        buf.push(&mk_log("t", Dir::Rx, "x", None, 0));
        assert!(buf.lines_before_no(1, 4).is_empty());
        assert_eq!(buf.lines_before_no(2, 4)[0].no, 1);
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
                auto_reply: Arc::new(RwLock::new(AutoReplyCfg::default())),
                alert_cfg: Arc::new(RwLock::new(AlertCfg::default())),
                capture_cfg: Arc::new(RwLock::new(CaptureCfg::default())),
                stop,
                write_tx: tx,
                log_path: Arc::new(RwLock::new(PathBuf::from("x.log"))),
                log_base: PathBuf::from("x.log"),
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
    fn open_session_log_empty_path_skips_recording() {
        // CLI 缺省不落盘：空路径 -> 不建文件（session_log=None），仍上报 connecting、无错误事件
        let sink = crate::sink::VecSink::default();
        let (session_log, ts_fmt) =
            open_session_log(&sink, "s1", std::path::Path::new(""), false, None);
        assert!(session_log.is_none());
        assert_eq!(ts_fmt, "%h:%m:%s.%t");
        assert_eq!(
            sink.0.lock().clone(),
            vec!["status s1 connecting".to_string()]
        );
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
    fn segment_path_inserts_stamp_before_extension() {
        use chrono::TimeZone;
        let dt = chrono::Local.with_ymd_and_hms(2026, 9, 4, 15, 30, 12).unwrap();
        // 默认命名 {id}.log：时间戳插在扩展名前
        let p = next_segment_path(std::path::Path::new("/data/sessions/s1.log"), &dt, |_| false);
        assert_eq!(
            p,
            PathBuf::from("/data/sessions/s1-20260904-153012.log")
        );
        // 无扩展名路径同样成立
        let p = next_segment_path(std::path::Path::new("/logs/COM3"), &dt, |_| false);
        assert_eq!(p, PathBuf::from("/logs/COM3-20260904-153012"));
    }

    #[test]
    fn segment_path_avoids_collision_with_counter_suffix() {
        use chrono::TimeZone;
        let dt = chrono::Local.with_ymd_and_hms(2026, 9, 4, 15, 30, 12).unwrap();
        // 首个候选已存在（同秒内再次分段）：-2 递增去重
        let seen = std::cell::Cell::new(0);
        let p = next_segment_path(std::path::Path::new("/data/s1.log"), &dt, |_| {
            seen.set(seen.get() + 1);
            seen.get() == 1
        });
        assert_eq!(p, PathBuf::from("/data/s1-20260904-153012-2.log"));
    }

    #[test]
    fn segment_due_only_when_enabled_and_date_changed() {
        let d7 = chrono::NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
        let d8 = chrono::NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        assert!(!segment_due(false, d7, d8)); // 开关关闭永不轮转
        assert!(!segment_due(true, d7, d7)); // 同日不轮转
        assert!(segment_due(true, d7, d8)); // 跨天轮转
        assert!(segment_due(true, d8, d7)); // 时钟回拨视为变更（一次性轮转）
    }

    #[test]
    fn host_of_network_and_serial() {
        // tcp-client：目标 host:port
        let tcp = PortConfig {
            transport: Some("tcp-client".into()),
            tcp_host: Some("192.168.1.9".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&tcp), "192.168.1.9:9000");
        // tcp-server：监听地址缺省 0.0.0.0（与 establish_link 一致）
        let srv = PortConfig {
            transport: Some("tcp-server".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&srv), "0.0.0.0:9000");
        // udp：本地监听端口
        let udp = PortConfig {
            transport: Some("udp".into()),
            udp_local_port: Some(5000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&udp), "0.0.0.0:5000");
        // 串口源：端口名（与 %H 相同）
        let serial = PortConfig {
            name: "COM3".into(),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&serial), "COM3");
    }
}
