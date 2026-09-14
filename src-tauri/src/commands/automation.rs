//! 场景自动化命令层（Stage 3 Task 3）：校验摘要 / 启动 / 停止 / 状态 / 报告。
//!
//! 分层（同 commands/replay.rs 做法）：tauri 壳只差 State 提取与 AppHandle emit——
//! 纯逻辑（校验摘要、会话守卫、运行登记表、host 适配）拆为可测单元，集成测试
//! tests/automation_commands.rs 与命令走同一路径。
//!
//! # 运行登记表 [`AutomationRegistry`]
//! runId（`run{N}` 单调递增）→ RunEntry{sessionId, cancel, status, report, join,
//! progress}。并发约束：**同一会话同时只允许一个运行中场景**（二次 start 报稳定
//! 文案；不同会话可并行）。运行线程：`run_scenario` 在 spawn 线程内跑，完成时经
//! finish_run 回写 status/report 并入完成队列；**保留最近 [`KEEP_COMPLETED`]（50）
//! 份 completed，只逐出 completed 条目（running 不入队，永不逐出）**。stop 幂等：
//! 未知/已完成 runId no-op。断开取消：[`cancel_and_disconnect`]（disconnect_cmd
//! 调用）先置 cancel 并 join 运行线程（runner 的 sleep/poll 都查 cancel，≤ 一个
//! 25ms 分片内退出），再 manager.disconnect——保证场景以 cancelled 收场，而不是
//! 因会话消失误报传输错误。
//!
//! # 事件契约（payload camelCase）
//! - `scenario-progress`：每叶子步开始一条、稀疏。core runner 无逐步 hook，进度由
//!   host 适配层 [`RunnerHost`] 按「新叶子步的 host 调用签名」近似识别（send/signal
//!   必响；Delay 仅 >`WAIT_POLL_SLICE_MS` 时响——≤25ms 与 Wait 轮询分片不可区分，
//!   轻微少计；Wait/Assert 首轮拉取响、后续轮询不响）。payload
//!   {runId, sessionId, currentStep, totalSteps, kind}。**不加逐行事件**。
//! - `scenario-finished`：恰一次，payload = [`ScenarioRunView`]。
//!
//! # Host 适配 [`ManagerScenarioHost`]
//! send → `manager.send`（SendModeDef→SendMode 转换在此；**换行由 runner 追加
//! （appendNewline），host 不处理**；hex 模式按 runner 契约由 host 先校验文本——
//! manager 读线程的 decode_hex 对非法字符静默丢弃，必须在上游拦截报
//! `HostError::Transport`）；signal → `manager.set_signal`；last_no →
//! `manager.bridge_last_no(id).unwrap_or(0)`；lines_after → `manager.
//! ring_lines_after_no`（严格大于 since、按 no 升序，语义即 runner 文档所指
//! `RingBuf::lines_after_no`）；sleep → 分段睡眠（≤25ms）检查 cancel；now_ms →
//! 系统 UNIX 纪元毫秒。离线/回放会话被 manager 以稳定文案拒绝，host 原样透传为
//! `HostError::Transport`。

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytetide_core::automation::report::StepStatus;
use bytetide_core::automation::{
    report_json, report_junit, run_scenario, validate_scenario, HostError, PinDef, Scenario,
    ScenarioHost, ScenarioReport, ScenarioStatus, SendModeDef, ValidatedScenario, ValidatedStep,
    WAIT_POLL_SLICE_MS,
};
use bytetide_core::serial::manager::{BridgeLine, Pin, SendMode, SendRequest};
use bytetide_core::serial::PortManager;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 完成报告保留份数（只逐出 completed 条目）。
const KEEP_COMPLETED: usize = 50;

/// 事件 emit 回调（命令层从 AppHandle.emit 注入；集成测试用记录闭包）。
pub type EmitFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

// ===== 视图 / 摘要 DTO（serde camelCase 即前端契约） =====

/// 校验错误详情（稳定 code + 索引路径 + message）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioErrorInfo {
    pub code: String,
    pub path: String,
    pub message: String,
}

/// `scenario_validate_cmd` 返回：校验是否通过 + 执行步静态上界 + 场景名。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedScenarioSummary {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScenarioErrorInfo>,
    pub step_count: u64,
    pub name: String,
}

/// 进度水位（当前执行到第几个叶子步 / 静态总步数；core runner 无逐步 hook，
/// 事件驱动近似识别，故 plan 形状中的 path 缺省）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressView {
    pub current_step: u64,
    pub total_steps: u64,
}

/// `scenario_status_cmd` 返回 / `scenario-finished` 事件载荷。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioRunView {
    pub run_id: String,
    pub session_id: String,
    /// `running|passed|failed|cancelled`。
    pub status: String,
    pub started_epoch_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// failed 时取首个失败步的 `{code}: {message}`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ProgressView>,
}

// ===== 纯逻辑：校验摘要 / 会话守卫 / 断开取消 / 启动入口 =====

/// 静态执行叶步上界（Repeat 展开；口径与 runner 运行期计数一致）。
fn count_executed_leaves(steps: &[ValidatedStep]) -> u64 {
    steps
        .iter()
        .map(|s| match s {
            ValidatedStep::Repeat { times, steps } => {
                u64::from(*times).saturating_mul(count_executed_leaves(steps))
            }
            _ => 1,
        })
        .sum()
}

/// 校验场景并产出摘要（`scenario_validate_cmd` 与启动前置共用；失败不出 Err，
/// 以 ok=false + 稳定 code 返回）。
pub fn validate_summary(scenario: Scenario) -> ValidatedScenarioSummary {
    let name = scenario.name.clone();
    match validate_scenario(scenario) {
        Ok(v) => {
            let step_count = count_executed_leaves(&v.steps);
            ValidatedScenarioSummary {
                ok: true,
                error: None,
                step_count,
                name: v.name,
            }
        }
        Err(e) => ValidatedScenarioSummary {
            ok: false,
            error: Some(ScenarioErrorInfo {
                code: e.code.as_str().to_string(),
                path: e.path,
                message: e.message,
            }),
            step_count: 0,
            name,
        },
    }
}

/// 场景启动会话守卫（Task 8 Step 3 放宽）：live 会话全步型可跑；offline 无条件
/// 拒绝；replay 仅当场景含 send/signal 步（含 Repeat 体）时拒绝——回放只读，
/// wait/assert/delay/repeat 对回放产生的行有效（plan Stage 3 里程碑「scenario
/// wait/assert behavior works against replay」）。命令层与集成测试共用路径。
pub fn ensure_session_runnable(
    manager: &PortManager,
    session_id: &str,
    steps: &[ValidatedStep],
) -> Result<(), String> {
    let Some(mode) = manager.session_mode(session_id) else {
        return Err("会话不存在".to_string());
    };
    match mode {
        "offline" => Err("离线会话不支持场景".to_string()),
        "replay" if contains_device_steps(steps) => Err("回放会话不支持发送步骤".to_string()),
        _ => Ok(()),
    }
}

/// 场景是否含发送面步骤（send/signal，穿透 Repeat 体）——回放会话拒绝的唯一判据。
fn contains_device_steps(steps: &[ValidatedStep]) -> bool {
    steps.iter().any(|s| match s {
        ValidatedStep::Send { .. } | ValidatedStep::Signal { .. } => true,
        ValidatedStep::Repeat { steps, .. } => contains_device_steps(steps),
        _ => false,
    })
}

/// 断开会话：先取消该会话运行中的场景（join 运行线程，保证 cancelled 收场与
/// finished 事件先于断开送达），再 manager.disconnect。disconnect_cmd 与集成
/// 测试共用路径。
pub fn cancel_and_disconnect(
    registry: &AutomationRegistry,
    manager: &PortManager,
    session_id: &str,
) -> Result<(), String> {
    registry.cancel_session(session_id);
    manager.disconnect(session_id).map_err(|e| e.to_string())
}

/// `scenario_start_cmd` 主体：校验先行（失败 Err 带稳定 code）→ 会话守卫 →
/// 登记表启动。命令层与集成测试共用路径。
pub fn start_scenario(
    registry: &AutomationRegistry,
    manager: &Arc<PortManager>,
    session_id: &str,
    scenario: Scenario,
    emit: EmitFn,
) -> Result<String, String> {
    let validated = validate_scenario(scenario).map_err(|e| format!("场景校验失败: {e}"))?;
    ensure_session_runnable(manager, session_id, &validated.steps)?;
    registry.start(session_id, validated, manager.clone(), emit)
}

// ===== 运行登记表 =====

/// 运行状态（视图字符串即事件/查询契约）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunStatus {
    Running,
    Passed,
    Failed,
    Cancelled,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn from_report(status: ScenarioStatus) -> Self {
        match status {
            ScenarioStatus::Passed => Self::Passed,
            ScenarioStatus::Failed => Self::Failed,
            ScenarioStatus::Cancelled => Self::Cancelled,
        }
    }
}

struct RunEntry {
    session_id: String,
    cancel: Arc<AtomicBool>,
    status: RunStatus,
    started_epoch_ms: u64,
    progress: Option<ProgressView>,
    report: Option<ScenarioReport>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    runs: HashMap<String, RunEntry>,
    /// 完成顺序的 runId 队列（只含 completed；running 不入队 → 永不逐出）。
    completed: VecDeque<String>,
}

/// 场景运行登记表（AppState 持有；Arc 共享给运行线程回写完成态）。
#[derive(Clone, Default)]
pub struct AutomationRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

impl AutomationRegistry {
    /// 启动场景：同会话并发守卫 → 登记 Running → spawn 运行线程。校验与会话
    /// 守卫由调用方（`start_scenario`）先行完成。
    pub fn start(
        &self,
        session_id: &str,
        scenario: ValidatedScenario,
        manager: Arc<PortManager>,
        emit: EmitFn,
    ) -> Result<String, String> {
        let total_steps = count_executed_leaves(&scenario.steps);
        let (run_id, cancel) = {
            let mut st = self.inner.lock();
            if st
                .runs
                .values()
                .any(|r| r.session_id == session_id && r.status == RunStatus::Running)
            {
                return Err(format!("同一会话同时只允许运行一个场景: {session_id}"));
            }
            let run_id = format!("run{}", st.next_id);
            st.next_id += 1;
            let cancel = Arc::new(AtomicBool::new(false));
            st.runs.insert(
                run_id.clone(),
                RunEntry {
                    session_id: session_id.to_string(),
                    cancel: cancel.clone(),
                    status: RunStatus::Running,
                    started_epoch_ms: system_now_ms(),
                    progress: None,
                    report: None,
                    join: None,
                },
            );
            (run_id, cancel)
        };
        let registry = self.clone();
        let rid = run_id.clone();
        let sid = session_id.to_string();
        let spawn = std::thread::Builder::new()
            .name(format!("scenario-{run_id}"))
            .spawn(move || {
                let mut host = RunnerHost {
                    inner: ManagerScenarioHost::new(manager, sid.clone()),
                    registry: registry.clone(),
                    run_id: rid.clone(),
                    session_id: sid,
                    total_steps,
                    step: 0,
                    emit,
                    last: Cell::new(None),
                };
                let report = run_scenario(&scenario, &mut host, &cancel);
                let view = registry.finish_run(&rid, report);
                if let Some(view) = view {
                    if let Ok(payload) = serde_json::to_value(&view) {
                        (host.emit)("scenario-finished", payload);
                    }
                }
            });
        match spawn {
            Ok(handle) => {
                if let Some(e) = self.inner.lock().runs.get_mut(&run_id) {
                    e.join = Some(handle);
                }
                Ok(run_id)
            }
            Err(e) => {
                self.inner.lock().runs.remove(&run_id);
                Err(format!("场景运行线程启动失败: {e}"))
            }
        }
    }

    /// 运行线程完成回写：status/report 落表、入完成队列并按上限逐出最旧
    /// completed（running 不入队永不逐出）。返回完成视图（emit 由调用方负责）。
    fn finish_run(&self, run_id: &str, report: ScenarioReport) -> Option<ScenarioRunView> {
        let mut st = self.inner.lock();
        {
            let entry = st.runs.get_mut(run_id)?;
            entry.status = RunStatus::from_report(report.status);
            entry.report = Some(report);
            entry.join = None;
        }
        st.completed.push_back(run_id.to_string());
        while st.completed.len() > KEEP_COMPLETED {
            let Some(oldest) = st.completed.pop_front() else {
                break;
            };
            // 队列只含 completed id，直接移除安全
            st.runs.remove(&oldest);
        }
        let entry = st.runs.get(run_id)?;
        Some(build_view(run_id, entry))
    }

    /// 进度水位回写（运行线程内、每叶子步开始时）。
    fn set_progress(&self, run_id: &str, current_step: u64, total_steps: u64) {
        if let Some(e) = self.inner.lock().runs.get_mut(run_id) {
            e.progress = Some(ProgressView {
                current_step,
                total_steps,
            });
        }
    }

    /// 停止运行（幂等）：置 cancel 并 join 运行线程；未知/已完成 runId no-op。
    pub fn stop(&self, run_id: &str) {
        let join = {
            let mut st = self.inner.lock();
            match st.runs.get_mut(run_id) {
                Some(e) if e.status == RunStatus::Running => {
                    e.cancel.store(true, Ordering::Relaxed);
                    e.join.take()
                }
                _ => None,
            }
        };
        if let Some(join) = join {
            let _ = join.join();
        }
    }

    /// 取消某会话的全部运行中场景（断开前调用）：置 cancel 并逐个 join。
    /// 不持锁 join（运行线程完成回写需要锁），无死锁面。
    pub fn cancel_session(&self, session_id: &str) {
        let joins: Vec<std::thread::JoinHandle<()>> = {
            let mut st = self.inner.lock();
            st.runs
                .values_mut()
                .filter(|r| r.session_id == session_id && r.status == RunStatus::Running)
                .filter_map(|r| {
                    r.cancel.store(true, Ordering::Relaxed);
                    r.join.take()
                })
                .collect()
        };
        for join in joins {
            let _ = join.join();
        }
    }

    /// 运行视图查询（未知 runId 报稳定文案；前端对逐出/迟到期事件据此忽略）。
    pub fn view(&self, run_id: &str) -> Result<ScenarioRunView, String> {
        let st = self.inner.lock();
        let entry = st
            .runs
            .get(run_id)
            .ok_or_else(|| format!("场景运行不存在: {run_id}"))?;
        Ok(build_view(run_id, entry))
    }

    /// 报告序列化：`json` → pretty JSON，`junit` → JUnit XML；未完成 / 未知
    /// run / 未知格式分别报稳定文案。
    pub fn report(&self, run_id: &str, format: &str) -> Result<String, String> {
        let st = self.inner.lock();
        let entry = st
            .runs
            .get(run_id)
            .ok_or_else(|| format!("场景运行不存在: {run_id}"))?;
        let renderer = match format {
            "json" => |r: &ScenarioReport| report_json(r).map_err(|e| e.to_string()),
            "junit" => |r: &ScenarioReport| Ok(report_junit(r)),
            other => return Err(format!("未知报告格式: {other}，支持 json|junit")),
        };
        let report = entry
            .report
            .as_ref()
            .ok_or_else(|| format!("场景尚未完成，报告未生成: {run_id}"))?;
        renderer(report)
    }
}

/// 组装运行视图（failed 时取首个失败步错误作 run 级 error）。
fn build_view(run_id: &str, entry: &RunEntry) -> ScenarioRunView {
    let error = if entry.status == RunStatus::Failed {
        entry.report.as_ref().and_then(|r| {
            r.steps
                .iter()
                .find(|s| s.status == StepStatus::Failed)
                .and_then(|s| s.error.as_ref())
                .map(|e| format!("{}: {}", e.code, e.message))
        })
    } else {
        None
    };
    ScenarioRunView {
        run_id: run_id.to_string(),
        session_id: entry.session_id.clone(),
        status: entry.status.as_str().to_string(),
        started_epoch_ms: entry.started_epoch_ms,
        duration_ms: entry.report.as_ref().map(|r| r.duration_ms),
        error,
        progress: entry.progress,
    }
}

/// 系统 UNIX 纪元毫秒（登记表时间戳 / host now_ms 唯一时间源）。
fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

// ===== Host 适配 =====

/// 经 PortManager 操作指定会话的 `ScenarioHost` 实现（桌面端适配，CLI 另写）。
pub struct ManagerScenarioHost {
    manager: Arc<PortManager>,
    session_id: String,
}

impl ManagerScenarioHost {
    fn new(manager: Arc<PortManager>, session_id: String) -> Self {
        Self {
            manager,
            session_id,
        }
    }

    /// hex 发送文本校验（runner 契约：合法性由 host 校验；换行符按空白忽略）。
    fn validate_hex(text: &str) -> Result<(), HostError> {
        let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        if !cleaned.is_empty()
            && cleaned.len().is_multiple_of(2)
            && cleaned.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Ok(());
        }
        Err(HostError::Transport(
            "hex 发送文本非法: 须为偶数个十六进制字符（空白忽略）".to_string(),
        ))
    }
}

impl ScenarioHost for ManagerScenarioHost {
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        if mode == SendModeDef::Hex {
            Self::validate_hex(text)?;
        }
        let mode = match mode {
            SendModeDef::Ascii => SendMode::Ascii,
            SendModeDef::Hex => SendMode::Hex,
        };
        self.manager
            .send(
                &self.session_id,
                SendRequest {
                    mode,
                    text: text.to_string(),
                },
            )
            .map_err(|e| HostError::Transport(e.to_string()))
    }

    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError> {
        let pin = match pin {
            PinDef::Dtr => Pin::Dtr,
            PinDef::Rts => Pin::Rts,
        };
        self.manager
            .set_signal(&self.session_id, pin, level)
            .map_err(|e| HostError::Transport(e.to_string()))
    }

    fn last_no(&self) -> u64 {
        self.manager.bridge_last_no(&self.session_id).unwrap_or(0)
    }

    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.manager
            .ring_lines_after_no(&self.session_id, since, max)
            .map_err(|e| HostError::Transport(e.to_string()))
    }

    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        // 分段睡眠（≤WAIT_POLL_SLICE_MS）：进入时/每段结束查 cancel
        let slice = Duration::from_millis(WAIT_POLL_SLICE_MS);
        let mut remaining = duration;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(HostError::Cancelled);
            }
            if remaining.is_zero() {
                return Ok(());
            }
            let step = remaining.min(slice);
            std::thread::sleep(step);
            remaining = remaining.saturating_sub(step);
        }
    }

    fn now_ms(&self) -> u64 {
        system_now_ms()
    }
}

/// 新叶子步的 host 调用种类（progress tick 的「同叶/新叶」判定依据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostCall {
    Send,
    Signal,
    SleepShort,
    SleepLong,
    LinesAfter,
    LastNo,
}

/// 进度 host 装饰器：委托 [`ManagerScenarioHost`]，并按「新叶子步签名」稀疏发
/// `scenario-progress` 事件 + 回写视图进度。单叶内的 host 调用序是确定的：
/// Send=[send]、Signal=[signal]、Delay=[sleep]、Wait=[lines_after, (lines_after|
/// sleep≤25ms)*]、Assert=[last_no, lines_after]。据此 tick 规则：
/// - send / signal 必是新叶；
/// - sleep >25ms 必是 Delay 新叶（≤25ms 与 Wait 轮询分片不可区分 → 不 tick）；
/// - lines_after 在前一次调用不是 lines_after/sleep(≤25ms) 时是新叶首轮拉取
///   （前一次是 last_no → Assert，否则 Wait）。
struct RunnerHost {
    inner: ManagerScenarioHost,
    registry: AutomationRegistry,
    run_id: String,
    session_id: String,
    total_steps: u64,
    step: u64,
    emit: EmitFn,
    last: Cell<Option<HostCall>>,
}

impl RunnerHost {
    fn tick(&mut self, kind: &'static str) {
        self.step += 1;
        self.registry
            .set_progress(&self.run_id, self.step, self.total_steps);
        let payload = serde_json::json!({
            "runId": self.run_id,
            "sessionId": self.session_id,
            "currentStep": self.step,
            "totalSteps": self.total_steps,
            "kind": kind,
        });
        (self.emit)("scenario-progress", payload);
    }
}

impl ScenarioHost for RunnerHost {
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        self.tick("send");
        self.last.set(Some(HostCall::Send));
        self.inner.send(mode, text)
    }

    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError> {
        self.tick("signal");
        self.last.set(Some(HostCall::Signal));
        self.inner.signal(pin, level)
    }

    fn last_no(&self) -> u64 {
        self.last.set(Some(HostCall::LastNo));
        self.inner.last_no()
    }

    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        let fresh = !matches!(
            self.last.get(),
            Some(HostCall::LinesAfter) | Some(HostCall::SleepShort)
        );
        if fresh {
            let kind = if self.last.get() == Some(HostCall::LastNo) {
                "assert"
            } else {
                "wait"
            };
            self.tick(kind);
        }
        self.last.set(Some(HostCall::LinesAfter));
        self.inner.lines_after(since, max)
    }

    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        let long = duration > Duration::from_millis(WAIT_POLL_SLICE_MS);
        if long {
            self.tick("delay");
            self.last.set(Some(HostCall::SleepLong));
        } else {
            self.last.set(Some(HostCall::SleepShort));
        }
        self.inner.sleep(duration, cancel)
    }

    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }
}

// ===== Tauri 命令（薄壳：State 提取 + AppHandle emit） =====

fn app_emit(app: AppHandle) -> EmitFn {
    Arc::new(move |event, payload| {
        let _ = app.emit(event, payload);
    })
}

/// 校验场景（不启动）：ok=false 时 error 携带稳定 code/path/message。
#[tauri::command]
pub fn scenario_validate_cmd(scenario: Scenario) -> ValidatedScenarioSummary {
    validate_summary(scenario)
}

/// 启动场景：校验先行 → 会话守卫（缺失/离线/回放稳定文案）→ 登记表启动
/// （同会话并发守卫）。返回 `run{N}`。
#[tauri::command]
pub fn scenario_start_cmd(
    session_id: String,
    scenario: Scenario,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    start_scenario(
        &state.automation,
        &state.manager,
        &session_id,
        scenario,
        app_emit(app),
    )
}

/// 停止场景（幂等）：未知/已完成 runId no-op。
#[tauri::command]
pub fn scenario_stop_cmd(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.automation.stop(&run_id);
    Ok(())
}

/// 运行状态查询（前端对迟到期事件按「场景运行不存在」忽略）。
#[tauri::command]
pub fn scenario_status_cmd(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<ScenarioRunView, String> {
    state.automation.view(&run_id)
}

/// 报告获取：format = `json`（pretty JSON）| `junit`（JUnit XML）。
#[tauri::command]
pub fn scenario_report_cmd(
    run_id: String,
    format: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.automation.report(&run_id, &format)
}

#[cfg(test)]
mod tests {
    //! 命令层形状冻结：ScenarioRunView/ValidatedScenarioSummary 的 serde
    //! camelCase 键与 plan Task 3 对齐。
    use super::*;

    #[test]
    fn scenario_run_view_serializes_camel_case() {
        let v = serde_json::to_value(ScenarioRunView {
            run_id: "run7".into(),
            session_id: "s1".into(),
            status: "running".into(),
            started_epoch_ms: 1_000,
            duration_ms: None,
            error: None,
            progress: Some(ProgressView {
                current_step: 2,
                total_steps: 5,
            }),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "runId": "run7",
                "sessionId": "s1",
                "status": "running",
                "startedEpochMs": 1000,
                "progress": {"currentStep": 2, "totalSteps": 5}
            })
        );
        let done = serde_json::to_value(ScenarioRunView {
            run_id: "run8".into(),
            session_id: "s2".into(),
            status: "failed".into(),
            started_epoch_ms: 2_000,
            duration_ms: Some(250),
            error: Some("wait_timeout: no matching line within 10 ms".into()),
            progress: None,
        })
        .unwrap();
        assert_eq!(
            done,
            serde_json::json!({
                "runId": "run8",
                "sessionId": "s2",
                "status": "failed",
                "startedEpochMs": 2000,
                "durationMs": 250,
                "error": "wait_timeout: no matching line within 10 ms"
            })
        );
    }

    #[test]
    fn validated_summary_serializes_camel_case() {
        let ok = serde_json::to_value(ValidatedScenarioSummary {
            ok: true,
            error: None,
            step_count: 3,
            name: "demo".into(),
        })
        .unwrap();
        assert_eq!(
            ok,
            serde_json::json!({"ok": true, "stepCount": 3, "name": "demo"})
        );
        let bad = serde_json::to_value(ValidatedScenarioSummary {
            ok: false,
            error: Some(ScenarioErrorInfo {
                code: "invalid_regex".into(),
                path: "steps[0]".into(),
                message: "boom".into(),
            }),
            step_count: 0,
            name: "demo".into(),
        })
        .unwrap();
        assert_eq!(
            bad,
            serde_json::json!({
                "ok": false,
                "error": {"code": "invalid_regex", "path": "steps[0]", "message": "boom"},
                "stepCount": 0,
                "name": "demo"
            })
        );
    }

    #[test]
    fn count_executed_leaves_matches_runner_expansion() {
        let leaf = || vec![ValidatedStep::Delay { ms: 1 }];
        assert_eq!(count_executed_leaves(&leaf()), 1);
        // repeat(3){2 叶} + 1 叶 = 7
        let steps = vec![
            ValidatedStep::Delay { ms: 1 },
            ValidatedStep::Repeat {
                times: 3,
                steps: leaf(),
            },
        ];
        // 注：repeat 体 1 叶 × 3 + 外层 1 叶 = 4
        assert_eq!(count_executed_leaves(&steps), 4);
        // times=0 → 0
        let zero = vec![ValidatedStep::Repeat {
            times: 0,
            steps: leaf(),
        }];
        assert_eq!(count_executed_leaves(&zero), 0);
    }
}
