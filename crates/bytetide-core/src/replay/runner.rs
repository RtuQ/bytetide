//! 回放调度线程：离线日志按相邻行 `epoch_millis` 差值重放为伪实时会话。
//!
//! # 调度语义（plan Task 6 Step 1 清单全覆盖）
//!
//! - 时序源=相邻行原始时间差 Δ；`delay = min(Δ, max_gap_ms) / speed`（**钳制作用于
//!   原始 Δ**，实际睡眠 = 钳后值 ÷ 速度 → 恒 ≤ `max_gap_ms / speed`，plan
//!   「10-second gap cap」语义的确立与测试见 `gap_cap_limits_actual_sleep`）。
//!   首行 / seek / loop 后首行的 Δ 视为 0（立即入库）。
//! - 逐行 `runtime.ingest(line, IngestOrigin::Replay)`：原顺序、行 ts/epoch 保持
//!   原值不重打（回放历史）；告警照常评估并经 sink 稀疏上报；自动回复与捕获触发
//!   由 Replay origin 结构性排除。
//! - 暂停立即停住（等待分段 ≤[`SLICE_MS`] 排水命令）；**Resume 从恢复时刻重算下一行
//!   完整延迟**（不补偿暂停期间累计的 wall-clock 迟到——无爆发追赶）。SetSpeed 运行中
//!   把当前剩余等待按速度比例重标定，暂停中生效于恢复后的下一次重算。
//! - SeekLine(n)：ring 清屏（seq 不回退，对齐 [`crate::serial::ring::RingBuf::clear`]
//!   语义）、reader 经 `read_page(after=n-1)` 锚点直达第 n 行（无「从第 N 行开始」
//!   直达接口即由此等效实现）、水位=n-1、下一行立即入库；Finished 下复活为 Running。
//! - EOF：looped 则 ring 清屏回到行 1 继续（插一小段睡眠防极速热旋），否则 Finished
//!   （线程驻留等待 SeekLine 复活或 Stop，不退出）。
//! - Stop / 控制通道关闭：置 Stopped 退出线程。
//! - 畸形行：offline 解析已跳过（坏行不占行号），runner 无需处理。
//!
//! # 状态上报
//!
//! 细粒度状态写 `SessionRuntime::replay_state`；会话级状态照 live 惯例经
//! sink.status/error 稀疏上报并写入共享 SessionStatus：起跑→connected（Running）、
//! 暂停/恢复不换会话状态（细粒度走 replay_state 查询，T7 接线控制面事件）、
//! Finished/Stopped→disconnected（仅当仍处活动态，重复收尾不重复发事件）、
//! 页读 IO 错误→error。
//!
//! # 可测试性
//!
//! [`ReplayClock`] 可注入：生产 [`WallClock`]（单调墙钟+线程睡眠）；测试虚拟时钟
//! （即时推进并记录每段睡眠、支持闸门冻结调度），暂停/调速的时序断言全部确定性。

use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::offline::OfflineReader;
use crate::serial::port::LogLine;
use crate::serial::runtime::{IngestOrigin, SessionRuntime, SessionStatus};
use crate::sink::EventSink;

use super::model::{valid_speed, ReplayCmd, ReplayConfig, ReplayState};

/// 睡眠分段/暂停轮询上限：Pause/Stop/SetSpeed 最长 50ms 响应（plan 指定）。
const SLICE_MS: u64 = 50;
/// 单页读取/seek 直达页大小（对齐 offline 稀疏索引步长，跳扫 ≤ 一页可接受）。
const PAGE_LINES: usize = 4096;

/// 可注入时钟/睡眠：生产 [`WallClock`]；测试虚拟时钟。调度只消费 `now_ms` 的差值。
pub trait ReplayClock: Send + Sync + 'static {
    /// 当前单调毫秒（起点任意）。
    fn now_ms(&self) -> u64;
    /// 睡眠一段（实现必须真实阻塞或推进虚拟时钟）；调度按 ≤50ms 分段调用、
    /// 段间排水命令。
    fn sleep_ms(&self, ms: u64);
}

/// 生产时钟：线程启动时刻起算的单调墙钟 + [`std::thread::sleep`]。
struct WallClock {
    start: Instant,
}

impl ReplayClock for WallClock {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn sleep_ms(&self, ms: u64) {
        if ms > 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }
}

/// 启动回放线程（生产装配入口）：立即置 Running 并上报 connected；
/// 控制通道关闭或 Stop 命令前线程常驻（Finished 后驻留等待 SeekLine 复活）。
pub fn spawn_replay(
    reader: OfflineReader,
    runtime: Arc<SessionRuntime>,
    config: ReplayConfig,
    commands: Receiver<ReplayCmd>,
    sink: Arc<dyn EventSink>,
    session_id: String,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("replay-{session_id}"))
        .spawn(move || {
            let clock = WallClock {
                start: Instant::now(),
            };
            run_replay(
                reader, runtime, config, commands, &*sink, session_id, &clock,
            );
        })
        .expect("spawn replay thread failed")
}

/// 回放主循环（测试经同一实现注入虚拟时钟）。
fn run_replay(
    reader: OfflineReader,
    runtime: Arc<SessionRuntime>,
    config: ReplayConfig,
    commands: Receiver<ReplayCmd>,
    sink: &dyn EventSink,
    session_id: String,
    clock: &dyn ReplayClock,
) {
    ReplayRunner {
        reader,
        runtime,
        commands,
        sink,
        session_id,
        clock,
        speed: config.speed,
        looped: config.looped,
        max_gap_ms: config.max_gap_ms,
        cursor: 0,
        prev_epoch: None,
        page: Vec::new(),
        page_idx: 0,
        deadline: None,
        state: ReplayState::Ready,
    }
    .run();
}

/// 单轮控制流：Continue=继续主循环，Exit=已置 Stopped、线程退出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Exit,
}

/// 回放调度状态机（字段语义见 [`run_replay`] 与模块文档）。
struct ReplayRunner<'a> {
    reader: OfflineReader,
    runtime: Arc<SessionRuntime>,
    commands: Receiver<ReplayCmd>,
    sink: &'a dyn EventSink,
    session_id: String,
    clock: &'a dyn ReplayClock,
    /// 当前速度（实际睡眠 = min(Δ, max_gap_ms) / speed）
    speed: f64,
    /// EOF 后是否清 ring 回到行 1 循环
    looped: bool,
    /// 相邻行原始时间差钳制上限（毫秒）
    max_gap_ms: u64,
    /// 文件水位：最后已 ingest 的文件行 no（read_page 游标；与 ring no 无关）
    cursor: u64,
    /// 上一已 ingest 行的原始 epoch；None=会话/seek/loop 后的首行（Δ=0 立即入库）
    prev_epoch: Option<u64>,
    /// 当前页缓冲（read_page 产物；行 no 连续 = cursor+1+page_idx 起）
    page: Vec<LogLine>,
    page_idx: usize,
    /// 下一行到期时刻（clock 基准毫秒）；None=待重算（起跑/暂停恢复/seek/调速后）
    deadline: Option<u64>,
    state: ReplayState,
}

impl ReplayRunner<'_> {
    fn run(&mut self) {
        self.set_state(ReplayState::Running);
        self.runtime.state.write().set_status(
            self.sink,
            &self.session_id,
            SessionStatus::Connected,
        );
        loop {
            if self.drain_commands() == Flow::Exit {
                return;
            }
            match self.state {
                ReplayState::Running => {}
                ReplayState::Paused => {
                    // 暂停：真实时钟阻塞等命令（不推进虚拟时钟、不忙旋、不发会话状态）
                    match self.commands.recv_timeout(Duration::from_millis(SLICE_MS)) {
                        Ok(cmd) => {
                            if self.apply_cmd(cmd) == Flow::Exit {
                                return;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            self.finish(ReplayState::Stopped);
                            return;
                        }
                    }
                    continue;
                }
                // 终态驻留：Finished 可被 SeekLine 复活；Stop/通道关闭退出
                ReplayState::Finished | ReplayState::Stopped | ReplayState::Error => {
                    match self.commands.recv() {
                        Ok(cmd) => {
                            if self.apply_cmd(cmd) == Flow::Exit {
                                return;
                            }
                        }
                        Err(_) => {
                            self.finish(ReplayState::Stopped);
                            return;
                        }
                    }
                    continue;
                }
                ReplayState::Ready => unreachable!("runner 起跑即 Running"),
            }
            // 取下一行（页耗尽续读；EOF 收尾/循环）
            if !self.refill() {
                continue;
            }
            // wait_due 期间可能暂停/seek：非 Running 交回主循环按新状态处理
            if self.state != ReplayState::Running {
                continue;
            }
            let pending = self.page[self.page_idx].clone();
            if self.wait_due(&pending) == Flow::Exit {
                return;
            }
            if self.state != ReplayState::Running {
                continue;
            }
            // 等待期间可能 seek 重定位（page/cursor 已换、page_idx 归零）：
            // 以当前水位上的行为准入库，丢弃等待前的旧行
            let Some(line) = self.page.get(self.page_idx).cloned() else {
                continue;
            };
            // 入库：原顺序、原 ts/epoch；告警稀疏上报（自动回复/捕获由 Replay origin
            // 结构性排除——见 SessionRuntime::ingest）
            let outcome = self.runtime.ingest(&line, IngestOrigin::Replay, self.sink);
            self.cursor += 1;
            // 控制面进度水位（T7：manager 查询面 → ReplayView.line）
            *self.runtime.replay_cursor.write() = self.cursor;
            self.page_idx += 1;
            self.prev_epoch = Some(line.epoch_millis);
            self.deadline = None;
            if !outcome.alerts.is_empty() {
                self.sink.alert_hits(&self.session_id, outcome.alerts);
            }
        }
    }

    /// 页耗尽时续读一页。返回 false=EOF 已收尾（循环重置或 Finished/Error 驻留），
    /// 主循环重入按状态分支处理。
    fn refill(&mut self) -> bool {
        if self.page_idx < self.page.len() {
            return true;
        }
        match self.reader.read_page(self.cursor, PAGE_LINES) {
            Ok(page) if !page.is_empty() => {
                self.page = page;
                self.page_idx = 0;
                true
            }
            Ok(_) => {
                // EOF：looped 则清 ring 回到行 1 继续（插一小段睡眠防极速热旋），
                // 否则 Finished 驻留（可被 SeekLine 复活）
                if self.looped {
                    self.runtime.ring.clear();
                    self.cursor = 0;
                    // 控制面进度水位同步回卷（下一轮 ingest 再推进）
                    *self.runtime.replay_cursor.write() = 0;
                    self.prev_epoch = None;
                    self.deadline = None;
                    self.clock.sleep_ms(SLICE_MS);
                } else {
                    self.finish(ReplayState::Finished);
                }
                false
            }
            Err(e) => {
                // 源文件页读 IO 错误：状态置 Error 且终止（读失败不回落 disconnected）
                self.set_state(ReplayState::Error);
                self.runtime.state.write().set_error(
                    self.sink,
                    &self.session_id,
                    &format!("回放读取失败: {e}"),
                    Some(SessionStatus::Error),
                );
                false
            }
        }
    }

    /// 到期等待：deadline 未定则按「下一行 Δ 钳制 ÷ 速度」从当下起算；
    /// 分段 ≤[`SLICE_MS`] 睡眠、段间排水命令（Pause 立即停住 / Stop 退出 /
    /// SetSpeed 重标定 / Seek 重定位）。
    fn wait_due(&mut self, line: &LogLine) -> Flow {
        let delay = self.line_delay_ms(line);
        if delay == 0 {
            return Flow::Continue;
        }
        if self.deadline.is_none() {
            self.deadline = Some(self.clock.now_ms() + scaled_ms(delay, self.speed));
        }
        loop {
            if self.drain_commands() == Flow::Exit {
                return Flow::Exit;
            }
            if self.state != ReplayState::Running {
                // 暂停/seek 后一律从当下重算（Pause/Resume/Seek 均置 deadline=None）
                self.deadline = None;
                return Flow::Continue;
            }
            let Some(dl) = self.deadline else {
                return Flow::Continue;
            };
            let now = self.clock.now_ms();
            if now >= dl {
                return Flow::Continue;
            }
            self.clock.sleep_ms((dl - now).min(SLICE_MS));
        }
    }

    /// 下一行相对上一行的原始延迟（毫秒，未除速度）：Δ 钳制 `max_gap_ms`；
    /// 会话/seek/loop 后首行（prev_epoch=None）为 0（立即入库）。
    fn line_delay_ms(&self, line: &LogLine) -> u64 {
        match self.prev_epoch {
            None => 0,
            Some(prev) => line.epoch_millis.saturating_sub(prev).min(self.max_gap_ms),
        }
    }

    /// 排水全部待处理命令；控制通道关闭（宿主放弃控制/断开）=Stop 语义。
    fn drain_commands(&mut self) -> Flow {
        loop {
            match self.commands.try_recv() {
                Ok(cmd) => {
                    if self.apply_cmd(cmd) == Flow::Exit {
                        return Flow::Exit;
                    }
                }
                Err(TryRecvError::Empty) => return Flow::Continue,
                Err(TryRecvError::Disconnected) => {
                    self.finish(ReplayState::Stopped);
                    return Flow::Exit;
                }
            }
        }
    }

    /// 应用单条命令（Stop 返回 Exit）。
    fn apply_cmd(&mut self, cmd: ReplayCmd) -> Flow {
        match cmd {
            ReplayCmd::Stop => {
                self.finish(ReplayState::Stopped);
                return Flow::Exit;
            }
            ReplayCmd::Pause => {
                if self.state == ReplayState::Running {
                    self.set_state(ReplayState::Paused);
                    self.deadline = None; // Resume 从当下重算（不补偿暂停期迟到）
                }
            }
            ReplayCmd::Resume => {
                if self.state == ReplayState::Paused {
                    self.set_state(ReplayState::Running);
                    self.deadline = None; // 主循环按恢复时刻重算下一行完整延迟
                }
            }
            ReplayCmd::SeekLine(n) => self.seek(n),
            ReplayCmd::SetSpeed(s) => {
                if valid_speed(s) {
                    let old = self.speed;
                    self.speed = s;
                    // 运行中的当前等待按速度比例重标定（剩余是真实 ms）；
                    // 暂停中 deadline=None，新速度在恢复后的重算中生效
                    if let Some(dl) = self.deadline {
                        let now = self.clock.now_ms();
                        let remaining = dl.saturating_sub(now);
                        let scaled = (remaining as f64 * old / s).round() as u64;
                        self.deadline = Some(now + scaled);
                    }
                }
            }
            ReplayCmd::SetLoop(b) => self.looped = b,
        }
        Flow::Continue
    }

    /// SeekLine(n)：ring 清屏（seq 不回退，对齐 `RingBuf::clear` 语义）、reader 经
    /// `read_page(after=n-1)` 锚点直达第 n 行、水位=n-1、下一行立即入库（prev_epoch
    /// 置 None）。Finished 状态复活为 Running 并回发 connected（用户显式重播）。
    /// Paused 下保持暂停（恢复后从第 n 行立即继续）。越界钳制到 [1, line_count]。
    fn seek(&mut self, n: u64) {
        let line_count = self.reader.line_count();
        if line_count == 0 {
            return; // 空文件无处可跳
        }
        let target = n.max(1).min(line_count);
        match self.reader.read_page(target - 1, PAGE_LINES) {
            Ok(page) => {
                self.runtime.ring.clear();
                self.cursor = target - 1;
                // 控制面进度水位=目标-1（恢复/运行中 ingest 下一行后再推进）
                *self.runtime.replay_cursor.write() = self.cursor;
                self.page = page;
                self.page_idx = 0;
                self.prev_epoch = None;
                self.deadline = None;
                if self.state == ReplayState::Finished {
                    self.set_state(ReplayState::Running);
                    self.runtime.state.write().set_status(
                        self.sink,
                        &self.session_id,
                        SessionStatus::Connected,
                    );
                }
            }
            Err(e) => {
                // 定位失败不改回放状态：记 last_error 供 REST 观测，调度照旧
                self.runtime.state.write().set_error(
                    self.sink,
                    &self.session_id,
                    &format!("回放定位失败: {e}"),
                    None,
                );
            }
        }
    }

    fn set_state(&mut self, s: ReplayState) {
        self.state = s;
        *self.runtime.replay_state.write() = Some(s);
    }

    /// 收尾：置回放状态；Finished/Stopped 照 live 惯例把活动态会话置 disconnected
    /// （Error 已置位不覆盖；重复收尾不重复发事件）。
    fn finish(&mut self, s: ReplayState) {
        self.set_state(s);
        if matches!(s, ReplayState::Finished | ReplayState::Stopped) {
            let mut st = self.runtime.state.write();
            if matches!(
                st.status,
                SessionStatus::Connecting | SessionStatus::Connected
            ) {
                st.set_status(self.sink, &self.session_id, SessionStatus::Disconnected);
            }
        }
    }
}

/// 原始延迟按速度缩放（四舍五入到毫秒）。
fn scaled_ms(delay_ms: u64, speed: f64) -> u64 {
    (delay_ms as f64 / speed).round() as u64
}

#[cfg(test)]
mod tests {
    //! fake-clock 调度单测：原间隔/速度缩放/gap 钳制/暂停恢复不爆发/seek 游标重置/
    //! loop/stop/EOF/调速（运行中与暂停中）。虚拟时钟即时推进并记录每段睡眠，
    //! 闸门冻结保证「暂停/调速发生在第 N 段睡眠处」的确定性断言。

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Condvar, Mutex};

    use super::super::model::DEFAULT_MAX_GAP_MS;
    use super::*;
    use crate::offline::open_offline;
    use crate::serial::ring::BridgeLine;
    use crate::sink::VecSink;

    // ========================= 测试基建 =========================

    static TEST_SEQ: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bytetide-replay-{tag}-{}-{}",
            std::process::id(),
            TEST_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 虚拟时钟：`now` 只被 sleep 推进；记录每段睡眠供时序断言；
    /// `gate_after > 0` 时第 gate 段之后的睡眠调用阻塞（闸门），直到 `release`。
    struct FakeClock {
        now: AtomicU64,
        sleeps: Mutex<Vec<u64>>,
        gate_after: AtomicUsize,
        gated: AtomicBool,
        gate_mu: Mutex<()>,
        gate_cv: Condvar,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: AtomicU64::new(0),
                sleeps: Mutex::new(Vec::new()),
                gate_after: AtomicUsize::new(0),
                gated: AtomicBool::new(false),
                gate_mu: Mutex::new(()),
                gate_cv: Condvar::new(),
            }
        }

        /// 设闸：第 `after` 段之后的睡眠调用阻塞（须在起跑前设置）。
        fn gate(&self, after: usize) {
            self.gate_after.store(after, Ordering::Relaxed);
        }

        /// 放行（阻塞中的睡眠立即返回并记账）。
        fn release(&self) {
            self.gate_after.store(0, Ordering::Relaxed);
            self.gate_cv.notify_all();
        }

        fn gated(&self) -> bool {
            self.gated.load(Ordering::Relaxed)
        }

        fn now(&self) -> u64 {
            self.now.load(Ordering::Relaxed)
        }

        fn sleeps(&self) -> Vec<u64> {
            self.sleeps.lock().unwrap().clone()
        }

        /// 睡眠段总和（= 虚拟时钟推进量）。
        fn total_sleep(&self) -> u64 {
            self.sleeps().iter().sum()
        }

        /// 第 `from` 段（0 起）之后的睡眠总和（闸门后的时序断言用）。
        fn total_sleep_from(&self, from: usize) -> u64 {
            self.sleeps().iter().skip(from).sum()
        }
    }

    impl ReplayClock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::Relaxed)
        }

        fn sleep_ms(&self, ms: u64) {
            // 先查闸再记账：阻塞期间不上账，释放后本段才推进虚拟时钟
            let gate = self.gate_after.load(Ordering::Relaxed);
            if gate > 0 && self.sleeps.lock().unwrap().len() >= gate {
                self.gated.store(true, Ordering::Relaxed);
                let mut g = self.gate_mu.lock().unwrap();
                while self.gate_after.load(Ordering::Relaxed) > 0 {
                    g = self.gate_cv.wait(g).unwrap();
                }
                self.gated.store(false, Ordering::Relaxed);
            }
            self.sleeps.lock().unwrap().push(ms);
            self.now.fetch_add(ms, Ordering::Relaxed);
        }
    }

    /// 轮询直到条件成立（虚拟时钟线程异步推进，带真实超时上限）。
    fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while !f() {
            assert!(Instant::now() < deadline, "wait_until 超时");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// epoch 当日毫秒 → 合法 ts（hh:mm:ss.mmm，可被 offline 解析回原值）。
    fn ts_of_ms(ms: u64) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1_000 % 60,
            ms % 1_000
        )
    }

    /// 写时序日志：每行 `{ts(epoch)}\tRX\tline-{i}`（dir 全 RX 便于断言）。
    fn write_timed(dir: &Path, name: &str, epochs: &[u64]) -> PathBuf {
        let path = dir.join(name);
        let mut body = String::new();
        for (i, &e) in epochs.iter().enumerate() {
            body.push_str(&format!("{}\tRX\tline-{i}\n", ts_of_ms(e)));
        }
        std::fs::write(&path, body).unwrap();
        path
    }

    /// 测试运行体：临时文件 + runtime + 共享虚拟时钟 + 控制通道，构造即起跑。
    struct TestRun {
        dir: PathBuf,
        runtime: Arc<SessionRuntime>,
        sink: Arc<VecSink>,
        clock: Arc<FakeClock>,
        tx: Option<Sender<ReplayCmd>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl TestRun {
        /// 无闸起跑。
        fn new(config: ReplayConfig, epochs: &[u64]) -> Self {
            let dir = temp_dir("run");
            let path = write_timed(&dir, "t.log", epochs);
            Self::start(config, &path, dir, 0)
        }

        /// 带闸起跑：闸须在起跑前设置才保证确定性。
        fn new_gated(config: ReplayConfig, epochs: &[u64], gate_after: usize) -> Self {
            let dir = temp_dir("gated");
            let path = write_timed(&dir, "t.log", epochs);
            Self::start(config, &path, dir, gate_after)
        }

        /// 用现成文件起跑（畸形行等自定义内容用）。
        fn with_file(config: ReplayConfig, path: &Path) -> Self {
            let dir = temp_dir("file");
            Self::start(config, path, dir, 0)
        }

        fn start(config: ReplayConfig, path: &Path, dir: PathBuf, gate_after: usize) -> Self {
            let (_, reader) = open_offline(path).unwrap();
            let runtime = Arc::new(SessionRuntime::new());
            let (tx, rx) = std::sync::mpsc::channel();
            let clock = Arc::new(FakeClock::new());
            clock.gate(gate_after);
            let sink = Arc::new(VecSink::default());
            let sink2: Arc<dyn EventSink> = sink.clone();
            let clock2 = clock.clone();
            let rt2 = runtime.clone();
            let handle = std::thread::spawn(move || {
                run_replay(reader, rt2, config, rx, &*sink2, "t1".to_string(), &*clock2);
            });
            Self {
                dir,
                runtime,
                sink,
                clock,
                tx: Some(tx),
                handle: Some(handle),
            }
        }

        fn send(&self, cmd: ReplayCmd) {
            self.tx.as_ref().unwrap().send(cmd).unwrap();
        }

        fn events(&self) -> Vec<String> {
            self.sink.0.lock().clone()
        }

        fn state_is(&self, s: ReplayState) -> bool {
            *self.runtime.replay_state.read() == Some(s)
        }

        fn ring(&self) -> Vec<BridgeLine> {
            self.runtime.ring.snapshot()
        }

        fn ring_nos(&self) -> Vec<u64> {
            self.ring().iter().map(|l| l.no).collect()
        }

        /// drop 控制通道（通道关闭=Stop 语义）+ join + 清理临时目录。
        fn join(&mut self) {
            self.tx.take();
            if let Some(h) = self.handle.take() {
                h.join().unwrap();
            }
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    impl Drop for TestRun {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn cfg(speed: f64, looped: bool, max_gap_ms: u64) -> ReplayConfig {
        ReplayConfig {
            speed,
            looped,
            max_gap_ms,
        }
    }

    // ========================= 调度语义 =========================

    #[test]
    fn replays_original_intervals_and_preserves_line_values() {
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000, 3_500]);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        // 原顺序、原 ts/epoch/dir 不重打（回放历史）
        let snap = run.ring();
        assert_eq!(
            snap.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["line-0", "line-1", "line-2"]
        );
        assert_eq!(
            snap.iter().map(|l| l.epoch_millis).collect::<Vec<_>>(),
            [1_000, 2_000, 3_500]
        );
        assert_eq!(snap[0].ts, "00:00:01.000");
        assert_eq!(snap[0].dir, crate::serial::port::Dir::Rx);
        // 原间隔：两段 gap 睡眠合计 1000+1500（speed 1；虚拟时钟只被睡眠推进）
        assert_eq!(run.clock.total_sleep(), 2_500);
        // 会话状态照 live 惯例：起跑 connected、EOF disconnected
        let ev = run.events();
        assert!(ev.contains(&"status t1 connected".to_string()), "{ev:?}");
        assert!(ev.contains(&"status t1 disconnected".to_string()), "{ev:?}");
        run.join();
        assert!(
            run.state_is(ReplayState::Stopped),
            "join（通道关闭）→Stopped"
        );
    }

    #[test]
    fn speed_scales_delays() {
        let mut run = TestRun::new(cfg(2.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000, 3_500]);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(run.clock.total_sleep(), 1_250, "2500 ÷ 2");
        assert_eq!(run.ring().len(), 3);
        run.join();
    }

    #[test]
    fn gap_cap_limits_actual_sleep_to_max_gap_over_speed() {
        // 语义确立（回报项）：plan「10-second gap cap」读作——钳制作用于原始 Δ
        // （min(Δ, max_gap_ms)），实际睡眠=钳后值 ÷ speed → 恒 ≤ max_gap_ms/speed。
        // Δ=70_000 > 10_000：speed 1 → 睡 10_000；speed 2 → 5_000；speed 100 → 100。
        for (speed, want) in [(1.0, 10_000), (2.0, 5_000), (100.0, 100)] {
            let mut run = TestRun::new(cfg(speed, false, 10_000), &[1_000, 71_000]);
            wait_until(2_000, || run.state_is(ReplayState::Finished));
            assert_eq!(run.clock.total_sleep(), want, "speed {speed}");
            assert_eq!(run.ring().len(), 2);
            run.join();
        }
    }

    #[test]
    fn pause_stops_immediately_and_resume_recomputes_full_delay_without_burst() {
        let mut run = TestRun::new_gated(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000], 10);
        // 卡在第 11 段睡眠：line1 已入库、gap 进行到 500ms
        wait_until(2_000, || run.clock.gated());
        assert_eq!(run.ring().len(), 1, "gap 中段：仅 line1 入库");
        // 先发 Pause 再放闸：闸内睡眠返回、排水即停住
        run.send(ReplayCmd::Pause);
        run.clock.release();
        wait_until(2_000, || run.state_is(ReplayState::Paused));
        assert_eq!(run.ring().len(), 1, "暂停后不再推进");
        let now_at_pause = run.clock.now();
        // 暂停期间虚拟时钟冻结（等待走真实 recv_timeout，不推进虚拟时钟）
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(run.clock.now(), now_at_pause);
        // Resume：从恢复时刻重算下一行完整延迟（不补偿暂停前的等待进度 → 无爆发）
        run.send(ReplayCmd::Resume);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(run.ring().len(), 2);
        assert_eq!(run.clock.total_sleep_from(11), 1_000, "恢复后睡满整段 gap");
        assert_eq!(run.clock.now(), now_at_pause + 1_000);
        run.join();
    }

    #[test]
    fn seek_clears_ring_keeps_seq_monotonic_and_repositions_reader() {
        let mut run = TestRun::new_gated(
            cfg(1.0, false, DEFAULT_MAX_GAP_MS),
            &[1_000, 2_000, 3_000],
            25,
        );
        // gap1 完（20 段）→ line2 入库；gap2 进行 250ms 时卡住
        wait_until(2_000, || run.clock.gated());
        assert_eq!(run.ring().len(), 2);
        run.send(ReplayCmd::SeekLine(2));
        run.clock.release();
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        // ring 清屏后从文件行 2 重放；seq 不回退（此前 2 行 → 新 no 从 3 起）
        let snap = run.ring();
        assert_eq!(
            snap.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["line-1", "line-2"]
        );
        assert_eq!(snap.iter().map(|l| l.no).collect::<Vec<_>>(), [3, 4]);
        // seek 后首行立即入库（无 gap 睡眠）；line3 按新邻差睡满 1000
        // （闸门在途分段 50 + line3 的 1000）
        assert_eq!(run.clock.total_sleep_from(25), 1_050);
        run.join();
    }

    #[test]
    fn loop_replays_from_first_line_after_eof() {
        let mut run = TestRun::new_gated(cfg(1.0, true, DEFAULT_MAX_GAP_MS), &[1_000, 2_000], 21);
        // 两行文件每轮只有一段 gap（line-0→line-1 的 1000ms = 20 段）+ EOF 回卷睡眠
        // （1 段）= 每轮 21 段；闸在第 22 段睡眠 = 第二轮 gap1 首段 → ring 已被清屏
        // 并重灌 line-0（no=3）
        wait_until(2_000, || run.clock.gated());
        let snap = run.ring();
        assert_eq!(
            snap.iter()
                .map(|l| (l.no, l.text.as_str()))
                .collect::<Vec<_>>(),
            [(3, "line-0")],
            "EOF 后清 ring 回到行 1：seq 单调续（no=3）"
        );
        // 停止收尾
        run.send(ReplayCmd::Stop);
        run.clock.release();
        run.join();
        assert!(run.state_is(ReplayState::Stopped));
    }

    #[test]
    fn stop_exits_thread_sets_stopped_and_reports_disconnected() {
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000, 3_000]);
        wait_until(2_000, || run.ring().len() >= 2);
        run.send(ReplayCmd::Stop);
        // join 带超时：线程应在 ≤50ms（一个睡眠分段）内退出
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = run.handle.take().unwrap();
        std::thread::spawn(move || {
            let _ = handle.join();
            done_tx.send(()).unwrap();
        });
        wait_until(2_000, || done_rx.try_recv().is_ok());
        assert!(run.state_is(ReplayState::Stopped));
        assert!(
            run.events().contains(&"status t1 disconnected".to_string()),
            "{:?}",
            run.events()
        );
        run.join();
    }

    #[test]
    fn control_channel_close_stops_runner() {
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000, 3_000]);
        wait_until(2_000, || !run.ring().is_empty());
        run.join(); // drop 控制通道 → recv Err → Stopped 退出
        assert!(run.state_is(ReplayState::Stopped));
    }

    #[test]
    fn eof_finishes_parks_and_seek_revives() {
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000]);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(run.ring_nos(), [1, 2]);
        assert!(
            run.events().contains(&"status t1 disconnected".to_string()),
            "EOF 置 disconnected"
        );
        // 线程驻留：SeekLine 复活为 Running 并重放（seq 续 3,4）
        run.send(ReplayCmd::SeekLine(1));
        wait_until(2_000, || run.ring_nos() == [3, 4]);
        assert!(run.state_is(ReplayState::Finished), "复活后再次 EOF");
        run.join();
    }

    #[test]
    fn set_speed_while_running_rescales_pending_wait() {
        let mut run = TestRun::new_gated(
            cfg(1.0, false, DEFAULT_MAX_GAP_MS),
            &[1_000, 2_000, 3_000],
            25,
        );
        // gap2 进行 250ms：line3 剩余等待 750ms（deadline=2000，now=1250）
        wait_until(2_000, || run.clock.gated());
        run.send(ReplayCmd::SetSpeed(2.0));
        run.clock.release();
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        // 在途分段 50 + 剩余 700 × (1.0/2.0) = 350
        assert_eq!(run.clock.total_sleep_from(25), 400);
        assert_eq!(run.ring().len(), 3);
        run.join();
    }

    #[test]
    fn set_speed_while_paused_applies_on_resume() {
        let mut run = TestRun::new_gated(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000], 10);
        wait_until(2_000, || run.clock.gated());
        run.send(ReplayCmd::Pause);
        run.clock.release();
        wait_until(2_000, || run.state_is(ReplayState::Paused));
        run.send(ReplayCmd::SetSpeed(2.0));
        run.send(ReplayCmd::Resume);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(
            run.clock.total_sleep_from(11),
            500,
            "恢复后整段 gap ÷ 新速度"
        );
        run.join();
    }

    #[test]
    fn invalid_set_speed_is_ignored_and_boundary_accepted() {
        // 非法值忽略（维持 1.0）：睡眠仍为原速合计
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000]);
        for bad in [0.0, -1.0, 1000.0, f64::NAN, f64::INFINITY] {
            run.send(ReplayCmd::SetSpeed(bad));
        }
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(run.clock.total_sleep(), 1_000, "非法 SetSpeed 不生效");
        run.join();
        // 边界值生效：speed=100 → gap 1000 睡 10ms
        let mut run = TestRun::new(cfg(100.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000]);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        assert_eq!(run.clock.total_sleep(), 10);
        run.join();
    }

    #[test]
    fn set_loop_mid_run_enables_cycling_after_eof() {
        let mut run = TestRun::new(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &[1_000, 2_000]);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        run.send(ReplayCmd::SetLoop(true));
        run.send(ReplayCmd::SeekLine(1));
        // 二轮 no=3,4；观察到第三轮起（last_no≥5）才证明 SetLoop 真的循环
        wait_until(2_000, || run.runtime.ring.last_no() >= 5);
        run.send(ReplayCmd::Stop);
        run.join();
        assert!(run.state_is(ReplayState::Stopped));
    }

    #[test]
    fn malformed_lines_are_skipped_by_offline_parsing() {
        // 坏行/注释/空行不占行号（offline 解析契约端到端）：runner 只见数据行
        let dir = temp_dir("bad");
        let path = dir.join("mixed.log");
        std::fs::write(
            &path,
            "# header\n\n00:00:01.000\tRX\ta\nbad-no-tab\n00:00:02.000\tRX\tb\n",
        )
        .unwrap();
        let mut run = TestRun::with_file(cfg(1.0, false, DEFAULT_MAX_GAP_MS), &path);
        wait_until(2_000, || run.state_is(ReplayState::Finished));
        let snap = run.ring();
        assert_eq!(
            snap.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(run.clock.total_sleep(), 1_000);
        run.join();
    }
}
