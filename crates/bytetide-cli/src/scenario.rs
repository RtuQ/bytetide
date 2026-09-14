//! 场景执行（Stage 3 Task 5）：数据源经 core `Transport` 接入，实现
//! [`ScenarioHost`] trait 驱动 `run_scenario`，报告 JSON/JUnit 落 `--report`。
//!
//! # 输出纪律
//! 进度/错误/人读摘要 → stderr；报告只写 `--report` 文件（`--report -` 时写
//! stdout，这是 run 唯一的 stdout 输出）；无 `--report` 时 stdout 完全干净。
//! core runner 无逐步回调，人读「每步一行」从完成后的报告渲染（步数/耗时/错误码
//! 逐条对应报告，不杜撰实时性）。
//!
//! # 宿主适配（[`CliHost`]）
//! - 行缓冲：后台读线程按 core `stream_loop` 同款语义切行（`\n` 分割、行尾 `\r`
//!   剥离、空闲 150ms 刷出半行），行进 `Arc<Shared>`（`VecDeque<BridgeLine>` +
//!   单调 `no`），容量对齐 core `RING_CAP`（满了淘汰最旧行，游标语义与 ring 一致）。
//! - 读写分权：`Box<dyn Transport>` 挂 `Arc<Mutex<..>>`，读线程与 host 的
//!   send/signal 互斥借用（read timeout 200ms 保证写侧等待有界）。
//! - TX 本地回显：send 成功后把原文作为 dir=tx 行入缓冲（对齐桌面端 TX 回显入
//!   ring 语义，`wait dir:tx` 可用）；HEX 模式 host 侧严格解码（runner 只做变量
//!   替换，合法性按宿主契约归 host）。
//! - 断连：读线程 EOF/硬错误置 dead + 消息，后续 `send`/`lines_after` 报
//!   [`HostError::Transport`]（场景 failed → 连接失败语义 exit 1）。
//! - 睡眠：50ms 分片查取消（任务约定 ≤50ms）；`now_ms` 用 SystemTime 纪元毫秒。
//! - 不落盘、无 ring、无规则评估：CLI 场景运行只做「驱动 + 观察」。
//!
//! # 退出码（[`exit_code_for`]）
//! 0=passed；130=cancelled；failed 看首个 failed 步的稳定错误码：
//! `wait_timeout`/`assert_failed`（设备未按预期响应）→ 3，其余（host/传输错误、
//! 变量未定义等运行时错误）→ 1。
//!
//! # 测试后门（`BYTETIDE_SCENARIO_FAKE_HOST`，不暴露在 help）
//! 环境变量非空时不开传输，改用内存 [`FakeHost`] 执行（JSON 配置：`lines` 注入
//! 响应行、`cancelAfterCalls` 第 N 次宿主调用后置位取消、`failSend` 使 send 报
//! 错）。进程级集成测试（`tests/cli_scenario.rs`）借此确定性断言退出码/报告形状，
//! 并用内置取消开关替代真实 SIGINT（跨平台无信号差异）。仅测试用途。

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytetide_core::automation::model::{PinDef, Scenario, SendModeDef};
use bytetide_core::automation::report::{
    report_json, report_junit, ScenarioReport, ScenarioStatus, StepStatus,
};
use bytetide_core::automation::runner::{run_scenario, HostError, ScenarioHost};
use bytetide_core::automation::ValidatedScenario;
use bytetide_core::logfmt;
use bytetide_core::serial::manager::BridgeLine;
use bytetide_core::serial::port::{Dir, PortConfig};
use bytetide_core::serial::ring::RING_CAP;
use bytetide_core::serial::transport::{self, Pin, Transport};

use crate::args::RunArgs;
use crate::input::parse_hex_pairs;

/// 测试后门环境变量：非空 = 用内存 fake host 跑场景（JSON 配置见 [`FakeHostConfig`]）。
pub const FAKE_HOST_ENV: &str = "BYTETIDE_SCENARIO_FAKE_HOST";
/// RX 半行空闲刷出阈值（对齐 core stream_loop 的 150ms 空闲刷出）。
const IDLE_FLUSH: Duration = Duration::from_millis(150);
/// host 睡眠的取消检查分片（任务约定 ≤50ms）。
const SLEEP_SLICE: Duration = Duration::from_millis(50);
/// 行缓冲容量（对齐 core RING_CAP：满了淘汰最旧行，与 ring 游标语义一致）。
const LINE_CAP: usize = RING_CAP;

// ============ 入口 ============

/// `run` 子命令入口：加载校验场景 → 连数据源 → 驱动 runner → 报告/退出码。
pub fn cmd_run(args: &RunArgs) -> i32 {
    // 1. 场景加载 + 静态校验：文件不可读/JSON 非法/校验失败 → 2（先于连接）
    let scenario = match load_scenario(&args.scenario) {
        Ok(v) => v,
        Err(msg) => {
            eprintln!("场景错误: {msg}");
            return 2;
        }
    };
    // 2. 数据源（clap required group 已保证有源；防御性兜底）
    let config = match crate::args::to_port_config(&args.source) {
        Ok(Some(c)) => c,
        _ => {
            eprintln!("参数错误: 未指定数据源");
            return 2;
        }
    };
    // 3. Ctrl-C：只置取消旗标，runner 在步边界/睡眠分片处收敛
    let cancel = Arc::new(AtomicBool::new(false));
    if ctrlc::set_handler({
        let c = cancel.clone();
        move || c.store(true, Ordering::Relaxed)
    })
    .is_err()
    {
        eprintln!("警告: 无法注册 Ctrl-C 处理器，取消将等到当前步超时");
    }
    // 4. 宿主：测试后门优先；否则连数据源（连接失败 → 1）
    let desc = crate::describe_config(&config);
    let mut host: Box<dyn ScenarioHost> = match fake_host_from_env(&cancel) {
        Some(Ok(h)) => Box::new(h),
        Some(Err(msg)) => {
            eprintln!("参数错误: {FAKE_HOST_ENV} 解析失败: {msg}");
            return 2;
        }
        None => {
            eprintln!("正在连接 {desc}…");
            match CliHost::open(&config) {
                Ok(h) => {
                    eprintln!("已连接 {desc}");
                    Box::new(h)
                }
                Err(e) => {
                    eprintln!("连接失败: {e}");
                    return 1;
                }
            }
        }
    };
    // 5. 执行（Drop 收尾：CliHost 析构停读线程并 join）
    let report = run_scenario(&scenario, host.as_mut(), &cancel);
    drop(host);
    // 6. 人读进度 + 摘要（stderr）
    print_progress(&report);
    // 7. 报告只写 --report（"-" = stdout）；写失败 → 1
    if let Err(e) = write_report(args, &report) {
        eprintln!("写报告失败: {e}");
        return 1;
    }
    exit_code_for(&report)
}

/// 场景文件读取 + 反序列化 + 静态校验（稳定错误串直接透出）。
fn load_scenario(path: &Path) -> Result<ValidatedScenario, String> {
    let shown = path.display();
    let raw = std::fs::read_to_string(path).map_err(|e| format!("无法读取 {shown}: {e}"))?;
    let scenario: Scenario =
        serde_json::from_str(&raw).map_err(|e| format!("{shown} 不是合法场景 JSON: {e}"))?;
    validate_out(scenario)
}

/// `validate_scenario` 的错误串化包装（放外层便于单测替身）。
fn validate_out(scenario: Scenario) -> Result<ValidatedScenario, String> {
    bytetide_core::automation::validate_scenario(scenario).map_err(|e| e.to_string())
}

// ============ 退出码判定 ============

/// 场景报告 → 进程退出码：
/// - passed → 0；cancelled → 130；
/// - failed → 首个 failed 步的稳定错误码：`wait_timeout` / `assert_failed`
///   （设备未按预期响应）→ 3；其余（host/传输错误、变量未定义等运行时错误）→ 1。
///   runner 不变式保证 failed 必有 failed 步；防御性兜底 1。
pub fn exit_code_for(report: &ScenarioReport) -> i32 {
    match report.status {
        ScenarioStatus::Passed => 0,
        ScenarioStatus::Cancelled => 130,
        ScenarioStatus::Failed => report
            .steps
            .iter()
            .find(|s| s.status == StepStatus::Failed)
            .map_or(1, |s| {
                let assertion = match &s.error {
                    // 首要判据：稳定错误码（host 错误发生在 wait/assert 内也归 1）
                    Some(e) => e.code == "wait_timeout" || e.code == "assert_failed",
                    // 防御兜底：报告形状异常时退回 kind 判定
                    None => s.kind == "wait" || s.kind == "assert",
                };
                if assertion {
                    3
                } else {
                    1
                }
            }),
    }
}

// ============ stderr 进度与报告写出 ============

/// 每个叶子步一行人读进度 + 末行摘要（全部 stderr）。
fn print_progress(report: &ScenarioReport) {
    let total = report.steps.len();
    for (i, step) in report.steps.iter().enumerate() {
        let tag = match step.status {
            StepStatus::Passed => "OK",
            StepStatus::Failed => "FAILED",
            StepStatus::Skipped => "SKIP",
        };
        eprintln!(
            "[{}/{}] {} {} {} {}ms",
            i + 1,
            total,
            step.kind,
            step.path,
            tag,
            step.duration_ms
        );
        if let Some(e) = &step.error {
            eprintln!("    {}: {}", e.code, e.message);
        }
    }
    eprintln!(
        "场景 \"{}\" {}: {}/{} 步，耗时 {}ms",
        report.name,
        status_str(report.status),
        report
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Passed)
            .count(),
        total,
        report.duration_ms
    );
}

fn status_str(status: ScenarioStatus) -> &'static str {
    match status {
        ScenarioStatus::Passed => "passed",
        ScenarioStatus::Failed => "failed",
        ScenarioStatus::Cancelled => "cancelled",
    }
}

/// 报告序列化并写出：json（pretty + 尾换行）/ junit；`--report -` 写 stdout，
/// 其余写文件，缺省不写。
fn write_report(args: &RunArgs, report: &ScenarioReport) -> Result<(), String> {
    let content = match args.report_format.as_str() {
        "junit" => report_junit(report),
        _ => {
            report_json(report)
                .map_err(|e| format!("报告序列化失败: {e}"))?
                .to_owned()
                + "\n"
        }
    };
    match args.report.as_deref() {
        None => Ok(()),
        Some("-") => io::stdout()
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string()),
        Some(path) => std::fs::write(path, content).map_err(|e| format!("{path}: {e}")),
    }
}

// ============ 公共小件 ============

/// 纪元毫秒（core `now_ms` 为 pub(crate)，CLI 侧同义实现）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 原始字节 → BridgeLine（对齐 core `make_rx_line`：lossy 文本；仅该行含非法
/// UTF-8 时附带原始 bytes，禁止把 U+FFFD 再编码当字节用）。
fn make_line(no: u64, dir: Dir, raw: &[u8]) -> BridgeLine {
    BridgeLine {
        no,
        ts: logfmt::format_ts(""),
        dir,
        text: String::from_utf8_lossy(raw).into_owned(),
        bytes: if std::str::from_utf8(raw).is_err() {
            Some(raw.to_vec())
        } else {
            None
        },
        epoch_millis: now_ms(),
        r#match: None,
    }
}

/// RX 字节流 → 行切分器（对齐 core stream_loop：`\n` 分割、行尾一个 `\r` 剥离、
/// 未结束半行由调用方在空闲超时后刷出）。
struct LineAssembler {
    buf: Vec<u8>,
}

impl LineAssembler {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入一段读取字节，把切出的完整行追加到 `out`。
    fn push(&mut self, chunk: &[u8], out: &mut Vec<Vec<u8>>) {
        for &b in chunk {
            if b == b'\n' {
                let mut line = std::mem::take(&mut self.buf);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                out.push(line);
            } else {
                self.buf.push(b);
            }
        }
    }

    fn has_partial(&self) -> bool {
        !self.buf.is_empty()
    }

    /// 刷出当前半行（空闲超时调用）。
    fn take_partial(&mut self, out: &mut Vec<Vec<u8>>) {
        if !self.buf.is_empty() {
            out.push(std::mem::take(&mut self.buf));
        }
    }
}

/// host 与读线程共享的行缓冲与断连状态。
struct Shared {
    lines: Mutex<VecDeque<BridgeLine>>,
    seq: AtomicU64,
    last_no: AtomicU64,
    dead: AtomicBool,
    dead_msg: Mutex<String>,
}

impl Shared {
    fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(0),
            last_no: AtomicU64::new(0),
            dead: AtomicBool::new(false),
            dead_msg: Mutex::new(String::from("连接已关闭")),
        }
    }

    /// 入一行（单调 no；容量淘汰最旧行，与 core ring 语义一致）。
    fn push_line(&self, dir: Dir, raw: &[u8]) {
        let no = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let line = make_line(no, dir, raw);
        let mut lines = self.lock_lines();
        if lines.len() >= LINE_CAP {
            lines.pop_front();
        }
        lines.push_back(line);
        self.last_no.store(no, Ordering::Relaxed);
    }

    /// 读线程终态：后续 host 调用统一报该消息（场景 failed → 连接失败 exit 1）。
    fn mark_dead(&self, msg: String) {
        *self.lock_dead_msg() = msg;
        self.dead.store(true, Ordering::Relaxed);
    }

    fn dead_error(&self) -> HostError {
        HostError::Transport(self.lock_dead_msg().clone())
    }

    /// `no > since` 的最旧 max 行、按 no 升序（对齐 `RingBuf::lines_after_no`）。
    fn snapshot_after(&self, since: u64, max: usize) -> Vec<BridgeLine> {
        let lines = self.lock_lines();
        let from = lines.partition_point(|l| l.no <= since);
        lines.iter().skip(from).take(max).cloned().collect()
    }

    fn lock_lines(&self) -> std::sync::MutexGuard<'_, VecDeque<BridgeLine>> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_dead_msg(&self) -> std::sync::MutexGuard<'_, String> {
        self.dead_msg.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ============ CliHost：真实传输适配 ============

/// CLI 场景宿主：后台读线程积累 RX 行缓冲，send/signal 走 `Transport`。
/// Drop 时停读线程并 join（经 `Box<dyn ScenarioHost>` 的 vtable drop 仍会执行）。
pub struct CliHost {
    io: Arc<Mutex<Box<dyn Transport>>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl CliHost {
    /// 打开数据源链路并启动读线程（链路建立失败原样上抛 io::Error）。
    pub fn open(config: &PortConfig) -> io::Result<Self> {
        let transport = transport::open_transport(config)?;
        let io = Arc::new(Mutex::new(transport));
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let io = io.clone();
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("scenario-reader".into())
                .spawn(move || read_loop(io, stop, shared))?
        };
        Ok(Self {
            io,
            shared,
            stop,
            reader: Some(reader),
        })
    }
}

impl Drop for CliHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

impl ScenarioHost for CliHost {
    fn on_step_started(&mut self, path: &str, kind: &'static str, current: u64, total: u64) {
        // runner 显式步骤回调 → live stderr 进度（长场景运行中可见，报告完成后
        // 另有逐步结果输出）
        eprintln!("[{current}/{total}] → {kind} {path}");
    }

    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        if self.shared.dead.load(Ordering::Relaxed) {
            return Err(self.shared.dead_error());
        }
        let bytes = match mode {
            SendModeDef::Ascii => text.as_bytes().to_vec(),
            // runner 已做变量替换；HEX 合法性按宿主契约在 host 侧严格校验
            SendModeDef::Hex => parse_hex_pairs(text)
                .map_err(|e| HostError::Transport(format!("HEX 发送解码失败: {e}")))?,
        };
        {
            let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
            io.write_all(&bytes)
                .and_then(|()| io.flush())
                .map_err(|e| HostError::Transport(format!("发送失败: {e}")))?;
        }
        // TX 本地回显入缓冲（对齐桌面端 TX 回显入 ring；text 含 runner 追加的换行）
        self.shared.push_line(Dir::Tx, text.as_bytes());
        Ok(())
    }

    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError> {
        let pin = match pin {
            PinDef::Dtr => Pin::Dtr,
            PinDef::Rts => Pin::Rts,
        };
        let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        // 网络源在链路层报「网络源无信号线」→ HostError::Transport（场景 failed → 1）
        io.set_signal(pin, level)
            .map_err(|e| HostError::Transport(e.to_string()))
    }

    fn last_no(&self) -> u64 {
        self.shared.last_no.load(Ordering::Relaxed)
    }

    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        if self.shared.dead.load(Ordering::Relaxed) {
            return Err(self.shared.dead_error());
        }
        Ok(self.shared.snapshot_after(since, max))
    }

    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        let mut left = duration;
        while !left.is_zero() {
            if cancel.load(Ordering::Relaxed) {
                return Err(HostError::Cancelled);
            }
            let d = left.min(SLEEP_SLICE);
            std::thread::sleep(d);
            left -= d;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

/// 读线程主循环（对齐 core stream_loop 的行切分与空闲刷出；EOF/硬错误置 dead）。
fn read_loop(io: Arc<Mutex<Box<dyn Transport>>>, stop: Arc<AtomicBool>, shared: Arc<Shared>) {
    let mut asm = LineAssembler::new();
    let mut buf = [0u8; 4096];
    let mut last_data = Instant::now();
    let mut out: Vec<Vec<u8>> = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // 锁 scoped：读超时 200ms 保证 send/signal 的等待有界
        let read = io.lock().unwrap_or_else(|e| e.into_inner()).read(&mut buf);
        match read {
            Ok(0) => {
                shared.mark_dead("连接已断开（对端关闭）".into());
                return;
            }
            Ok(n) => {
                last_data = Instant::now();
                out.clear();
                asm.push(&buf[..n], &mut out);
                for raw in &out {
                    shared.push_line(Dir::Rx, raw);
                }
            }
            Err(e)
                if e.kind() == io::ErrorKind::TimedOut || e.kind() == io::ErrorKind::WouldBlock =>
            {
                // 空闲：把未结束的半行刷出（150ms 空闲，语义同 core stream_loop）
                if asm.has_partial() && last_data.elapsed() >= IDLE_FLUSH {
                    out.clear();
                    asm.take_partial(&mut out);
                    for raw in &out {
                        shared.push_line(Dir::Rx, raw);
                    }
                }
            }
            Err(e) => {
                shared.mark_dead(format!("读取错误: {e}"));
                return;
            }
        }
    }
}

// ============ FakeHost：进程级集成测试后门（不暴露在 help） ============

/// fake host 配置（`BYTETIDE_SCENARIO_FAKE_HOST` 的 JSON 值，camelCase）。
/// 手工从 `serde_json::Value` 解析（CLI 不依赖 serde derive，省一条依赖线）。
#[derive(Debug, Default, PartialEq, Eq)]
struct FakeHostConfig {
    /// 首次交互（send/lines_after）时注入的 RX 响应行。
    lines: Vec<String>,
    /// 第 N 次宿主调用完成后置位取消旗标（1 基；模拟 Ctrl-C → exit 130）。
    cancel_after_calls: Option<u64>,
    /// send 报传输错误（宿主错误 → exit 1 路径）。
    fail_send: bool,
}

impl FakeHostConfig {
    fn from_value(v: &serde_json::Value) -> Result<Self, String> {
        let get = |key: &str| v.get(key);
        let lines = match get("lines") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .map(|it| {
                    it.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("lines 必须是字符串数组，收到 {it}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => return Err(format!("lines 必须是字符串数组，收到 {other}")),
        };
        let cancel_after_calls = match get("cancelAfterCalls") {
            None | Some(serde_json::Value::Null) => None,
            Some(n) => Some(
                n.as_u64()
                    .ok_or_else(|| format!("cancelAfterCalls 必须是非负整数，收到 {n}"))?,
            ),
        };
        let fail_send = match get("failSend") {
            None | Some(serde_json::Value::Null) => false,
            Some(b) => b
                .as_bool()
                .ok_or_else(|| format!("failSend 必须是布尔值，收到 {b}"))?,
        };
        Ok(Self {
            lines,
            cancel_after_calls,
            fail_send,
        })
    }
}

/// 内存 fake host：确定性、零硬件。`lines` 在首次交互时注入（场景 baseline 取
/// `last_no` 在先，wait 才能看到这些行）。
struct FakeHost {
    cfg: FakeHostConfig,
    lines: VecDeque<BridgeLine>,
    seq: u64,
    calls: u64,
    cancel: Arc<AtomicBool>,
    started: bool,
}

impl FakeHost {
    fn new(cfg: FakeHostConfig, cancel: Arc<AtomicBool>) -> Self {
        Self {
            cfg,
            lines: VecDeque::new(),
            seq: 0,
            calls: 0,
            cancel,
            started: false,
        }
    }

    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let lines = self.cfg.lines.clone();
        for text in lines {
            self.push_line(Dir::Rx, text.as_bytes());
        }
    }

    fn push_line(&mut self, dir: Dir, raw: &[u8]) {
        self.seq += 1;
        self.lines.push_back(make_line(self.seq, dir, raw));
    }

    /// 调用计数；达到 `cancelAfterCalls` 即置位取消（模拟 Ctrl-C 在步边界生效）。
    fn tick(&mut self) {
        self.calls += 1;
        if self.cfg.cancel_after_calls == Some(self.calls) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl ScenarioHost for FakeHost {
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        self.ensure_started();
        if self.cfg.fail_send {
            return Err(HostError::Transport("fake host: 发送失败".into()));
        }
        let _ = mode; // fake host 不做编码校验，原文入 TX 回显
        self.push_line(Dir::Tx, text.as_bytes());
        self.tick();
        Ok(())
    }

    fn signal(&mut self, _pin: PinDef, _level: bool) -> Result<(), HostError> {
        self.tick();
        Ok(())
    }

    fn last_no(&self) -> u64 {
        self.lines.back().map_or(0, |l| l.no)
    }

    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.ensure_started();
        self.tick();
        let from = self.lines.partition_point(|l| l.no <= since);
        Ok(self.lines.iter().skip(from).take(max).cloned().collect())
    }

    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        // fake 无可中断睡眠：入口查取消，睡醒复查（测试场景用小延时保持快速）
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        std::thread::sleep(duration);
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        self.tick();
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

/// 读取测试后门环境变量并解析配置；空串 = 默认配置。
fn fake_host_from_env(cancel: &Arc<AtomicBool>) -> Option<Result<FakeHost, String>> {
    let raw = std::env::var_os(FAKE_HOST_ENV)?
        .to_string_lossy()
        .into_owned();
    Some(parse_fake_config(&raw).map(|cfg| FakeHost::new(cfg, cancel.clone())))
}

fn parse_fake_config(raw: &str) -> Result<FakeHostConfig, String> {
    if raw.trim().is_empty() {
        return Ok(FakeHostConfig::default());
    }
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("不是合法 JSON: {e}"))?;
    FakeHostConfig::from_value(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytetide_core::automation::report::{ScenarioStatus, StepErrorInfo, StepReport};

    /// 构造报告：passed/failed 步由 `(kind, code)` 序列描述（None = 无错误即 passed）。
    fn report_with(status: ScenarioStatus, failed: &[(&str, Option<&str>)]) -> ScenarioReport {
        let steps = failed
            .iter()
            .map(|&(kind, code)| StepReport {
                path: "steps[0]".to_string(),
                kind: kind_of(kind),
                started_epoch_ms: 0,
                duration_ms: 1,
                matched_no: None,
                error: code.map(|c| StepErrorInfo {
                    code: c.to_string(),
                    message: "m".to_string(),
                }),
                status: if code.is_some() {
                    StepStatus::Failed
                } else {
                    StepStatus::Passed
                },
            })
            .collect();
        ScenarioReport {
            name: "t".to_string(),
            status,
            started_epoch_ms: 0,
            finished_epoch_ms: 1,
            duration_ms: 1,
            variables: Default::default(),
            steps,
        }
    }

    fn kind_of(kind: &str) -> &'static str {
        match kind {
            "send" => "send",
            "delay" => "delay",
            "signal" => "signal",
            "wait" => "wait",
            "assert" => "assert",
            _ => panic!("unknown kind {kind}"),
        }
    }

    // ---------- 退出码判定 ----------

    #[test]
    fn exit_code_passed_zero() {
        assert_eq!(exit_code_for(&report_with(ScenarioStatus::Passed, &[])), 0);
    }

    #[test]
    fn exit_code_cancelled_130() {
        assert_eq!(
            exit_code_for(&report_with(ScenarioStatus::Cancelled, &[])),
            130
        );
    }

    #[test]
    fn exit_code_wait_timeout_is_3() {
        assert_eq!(
            exit_code_for(&report_with(
                ScenarioStatus::Failed,
                &[("send", None), ("wait", Some("wait_timeout"))]
            )),
            3
        );
    }

    #[test]
    fn exit_code_assert_failed_is_3() {
        assert_eq!(
            exit_code_for(&report_with(
                ScenarioStatus::Failed,
                &[("send", None), ("assert", Some("assert_failed"))]
            )),
            3
        );
    }

    #[test]
    fn exit_code_host_error_on_send_is_1() {
        assert_eq!(
            exit_code_for(&report_with(
                ScenarioStatus::Failed,
                &[("send", Some("host_transport"))]
            )),
            1
        );
    }

    #[test]
    fn exit_code_host_error_during_wait_is_1_not_3() {
        // wait 步内的 host/传输错误是连接失败语义（1），不是断言失败（3）
        assert_eq!(
            exit_code_for(&report_with(
                ScenarioStatus::Failed,
                &[("wait", Some("host_transport"))]
            )),
            1
        );
    }

    #[test]
    fn exit_code_undefined_variable_is_1() {
        assert_eq!(
            exit_code_for(&report_with(
                ScenarioStatus::Failed,
                &[("send", Some("undefined_variable"))]
            )),
            1
        );
    }

    #[test]
    fn exit_code_failed_without_visible_failed_step_falls_back_to_1() {
        // runner 不变式外防御：无 failed 步的 failed 报告 → 1
        assert_eq!(exit_code_for(&report_with(ScenarioStatus::Failed, &[])), 1);
    }

    // ---------- 行切分 ----------

    #[test]
    fn assembler_splits_lf_strips_cr_and_keeps_partial() {
        let mut asm = LineAssembler::new();
        let mut out = Vec::new();
        asm.push(b"a\nb\r\nc", &mut out);
        assert_eq!(out, vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(asm.has_partial());
        out.clear();
        asm.push(b"d\n", &mut out);
        assert_eq!(out, vec![b"cd".to_vec()]);
        assert!(!asm.has_partial());
    }

    #[test]
    fn assembler_takes_partial_on_idle_flush() {
        let mut asm = LineAssembler::new();
        asm.push(b"half", &mut Vec::new());
        let mut out = Vec::new();
        asm.take_partial(&mut out);
        assert_eq!(out, vec![b"half".to_vec()]);
        assert!(!asm.has_partial());
        // 空缓冲刷出无事发生
        let mut out2 = Vec::new();
        asm.take_partial(&mut out2);
        assert!(out2.is_empty());
    }

    #[test]
    fn assembler_preserves_binary_bytes() {
        let mut asm = LineAssembler::new();
        let mut out = Vec::new();
        asm.push(&[0x00, 0xff, b'\n', 0x80], &mut out);
        assert_eq!(out, vec![vec![0x00, 0xff]]);
        assert!(asm.has_partial());
    }

    #[test]
    fn make_line_attaches_bytes_only_for_invalid_utf8() {
        let ok = make_line(1, Dir::Rx, "中文".as_bytes());
        assert_eq!(ok.text, "中文");
        assert_eq!(ok.bytes, None);

        let bad = make_line(2, Dir::Rx, &[0x61, 0x80, 0x62]);
        assert!(bad.text.contains('\u{fffd}'));
        assert_eq!(bad.bytes.as_deref(), Some(&[0x61, 0x80, 0x62][..]));
        assert_eq!(bad.no, 2);
    }

    // ---------- Shared 快照 ----------

    #[test]
    fn shared_snapshot_after_is_strictly_greater_and_ordered() {
        let shared = Shared::new();
        shared.push_line(Dir::Rx, b"a");
        shared.push_line(Dir::Tx, b"b");
        shared.push_line(Dir::Rx, b"c");
        assert_eq!(shared.last_no.load(Ordering::Relaxed), 3);
        let after = shared.snapshot_after(1, 10);
        assert_eq!(after.len(), 2);
        assert_eq!((after[0].no, after[1].no), (2, 3));
        assert_eq!(after[0].dir, Dir::Tx);
        // max 截断
        assert_eq!(shared.snapshot_after(0, 2).len(), 2);
    }

    #[test]
    fn shared_dead_error_carries_message() {
        let shared = Shared::new();
        assert!(!shared.dead.load(Ordering::Relaxed));
        shared.mark_dead("对端关闭".into());
        assert!(shared.dead.load(Ordering::Relaxed));
        assert_eq!(shared.dead_error(), HostError::Transport("对端关闭".into()));
    }

    // ---------- fake host ----------

    #[test]
    fn fake_config_parses_camel_case_with_defaults() {
        let cfg = parse_fake_config(r#"{"lines":["PONG"],"cancelAfterCalls":2}"#).unwrap();
        assert_eq!(cfg.lines, vec!["PONG".to_string()]);
        assert_eq!(cfg.cancel_after_calls, Some(2));
        assert!(!cfg.fail_send);
        let empty = parse_fake_config("").unwrap();
        assert_eq!(
            empty,
            FakeHostConfig {
                lines: Vec::new(),
                cancel_after_calls: None,
                fail_send: false
            }
        );
        assert!(parse_fake_config("not-json").is_err());
        // 字段类型错误各自报错
        assert!(parse_fake_config(r#"{"lines":[1]}"#).is_err());
        assert!(parse_fake_config(r#"{"cancelAfterCalls":-1}"#).is_err());
        assert!(parse_fake_config(r#"{"cancelAfterCalls":"x"}"#).is_err());
        assert!(parse_fake_config(r#"{"failSend":"yes"}"#).is_err());
    }

    #[test]
    fn fake_host_injects_lines_on_first_interaction_only() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut host = FakeHost::new(
            FakeHostConfig {
                lines: vec!["PONG".into()],
                ..Default::default()
            },
            cancel,
        );
        // baseline 阶段 last_no=0（注入尚未发生）
        assert_eq!(host.last_no(), 0);
        // 首次 lines_after 注入并可见
        let batch = host.lines_after(0, 10).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].no, 1);
        assert_eq!(batch[0].dir, Dir::Rx);
        // 之后不再重复注入
        assert_eq!(host.lines_after(1, 10).unwrap().len(), 0);
    }

    #[test]
    fn fake_host_send_records_tx_echo_and_can_fail() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut host = FakeHost::new(Default::default(), cancel.clone());
        host.send(SendModeDef::Ascii, "PING\n").unwrap();
        assert_eq!(host.last_no(), 1, "TX 回显入缓冲");
        assert_eq!(host.lines[0].dir, Dir::Tx);

        let mut failing = FakeHost::new(
            FakeHostConfig {
                fail_send: true,
                ..Default::default()
            },
            cancel,
        );
        assert!(matches!(
            failing.send(SendModeDef::Ascii, "PING"),
            Err(HostError::Transport(_))
        ));
    }

    #[test]
    fn fake_host_cancel_after_calls_sets_flag() {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut host = FakeHost::new(
            FakeHostConfig {
                cancel_after_calls: Some(2),
                ..Default::default()
            },
            cancel.clone(),
        );
        host.signal(PinDef::Dtr, true).unwrap();
        assert!(!cancel.load(Ordering::Relaxed));
        host.signal(PinDef::Rts, false).unwrap();
        assert!(cancel.load(Ordering::Relaxed), "第 2 次调用后置位");
    }

    #[test]
    fn fake_host_sleep_honors_cancel_at_entry() {
        let cancel = AtomicBool::new(true);
        let mut host = FakeHost::new(Default::default(), Arc::new(AtomicBool::new(false)));
        assert_eq!(
            host.sleep(Duration::from_millis(1), &cancel),
            Err(HostError::Cancelled)
        );
    }
}
