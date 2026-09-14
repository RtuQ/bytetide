//! 场景运行器（Stage 3 Task 2）：迭代式确定性执行引擎 + 宿主抽象 [`ScenarioHost`]。
//!
//! # 宿主契约（[`ScenarioHost`]）
//! - [`ScenarioHost::last_no`]：ring 末行 `no`（空 = 0）。
//! - [`ScenarioHost::lines_after`]：**严格大于** `since` 的最旧 `max` 行、按 `no`
//!   升序——语义对齐 [`crate::serial::ring::RingBuf::lines_after_no`]，桌面/CLI
//!   适配器（Task 3/5）直接委托它实现。
//! - [`ScenarioHost::sleep`]：可取消睡眠；进入时或睡眠中 `cancel` 置位须尽快返回
//!   [`HostError::Cancelled`]。Wait 轮询分片经它睡眠。
//! - [`ScenarioHost::now_ms`]：报告时间戳与 Wait deadline 的唯一时间源（可注入
//!   假钟做确定性测试；生产实现返回 UNIX 纪元毫秒）。
//! - [`ScenarioHost::send`]：`mode` 只决定 host 侧编码语义；runner 只做变量替换与
//!   换行追加，hex 文本合法性由 host 校验并报 [`HostError::Transport`]。
//!
//! # HostError 稳定错误码
//! `host_backpressure`（队列满被拒）/ `host_transport`（传输/会话层故障，消息透传）/
//! `host_cancelled`（sleep 取消）。除 `Cancelled` 外任一 host 错误 → 场景 failed、
//! 后续步 skipped（fail fast）。
//!
//! # 执行语义（显式栈状态机，不递归）
//! - **Wait 游标 = 场景级**：场景开始取 `baseline = host.last_no()` 一次；每个 Wait
//!   只看 `no > cursor` 的新行；每处理完一批（一次 `lines_after` 返回，无论命中与否）
//!   `cursor = 批内最大 no`——「观察过的批次即消费」，同批中命中行之后的行对后续
//!   Wait 不可见。因此 Wait 永不重匹配旧行；场景开始前已存在的行对 Wait 不可见
//!   （Assert 的回看不受此限，取 `host.last_no()` 现值）。
//! - **Wait 轮询**：deadline = 步开始 now + timeoutMs；每轮拉一批 → 命中即成功
//!   （`matched_no` = 命中行 no）；否则 now ≥ deadline 报 `wait_timeout`；否则睡
//!   `min(25ms, 剩余)`（[`WAIT_POLL_SLICE_MS`]）。timeoutMs=0 仍有一次拉取机会。
//! - **捕获 save**：regex matcher 对命中行 text 取 captures——group 0 = 整个正则
//!   匹配、>0 = 对应捕获组（组未参与匹配存空串）；literal/hex/mask（校验层只允许
//!   group 0）= 命中行整行 text。写入变量表，后续步骤 substitute 立即可见。
//! - **Assert 回看**：在「最近 within_last 行」内找（不消费 Wait 游标）；命中取
//!   **最新**一条的 no。`within_last = 0` 视为 1（只看最后一行）——0 不是「全部
//!   历史」。未命中报 `assert_failed`，message = 替换后的 Assert.message。
//! - **Repeat**：times=0 整体跳过（无叶子无报告）；路径后缀 `#{k}` 为 0 基迭代序，
//!   嵌套由外到内逐层追加（`steps[6].steps[1].steps[0]#1#0`）。
//! - **步数上限**：执行中计数（repeat 实际展开），第 10,001 个叶子步触发
//!   `step_limit_exceeded` → failed（静态校验恰好 10,000 步的场景不受影响；
//!   运行期上限同时防御绕过校验的手工构造 [`ValidatedScenario`]）。
//! - **取消**：叶子/Repeat 步开始前、Wait 每次拉取/睡眠前查 cancel；sleep 中取消经
//!   [`HostError::Cancelled`] 传播；每步成功完成后复查 cancel（host 调用之后的
//!   步级体现）——已完成的步仍记 passed，运行其后终止。取消后：被中断步与全部
//!   未执行步记 skipped（error=cancelled），场景 status=cancelled。
//! - **失败即停**：host 错误 / wait 超时 / assert 未命中 / 运行期变量替换失败 /
//!   步数超限 → 场景 failed；当前步 failed，未执行步 skipped（按本会执行的顺序
//!   展开 Repeat 剩余迭代补齐 skipped 条目，预算 [`MAX_EXECUTED_STEPS`] 防失控）。
//!
//! # 稳定步级错误码
//! `wait_timeout` / `assert_failed` / `undefined_variable` / `step_limit_exceeded` /
//! `host_backpressure` / `host_transport` / `host_cancelled`（host 错误码原样用作
//! 步级码）/ `cancelled`（skipped 步统一标记，JUnit `<skipped message="cancelled">`）。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::automation::matcher::{substitute, CompiledMatcher};
use crate::automation::model::{
    CaptureSpec, PinDef, SendModeDef, ValidatedScenario, ValidatedStep, MAX_EXECUTED_STEPS,
};
use crate::automation::report::{
    ScenarioReport, ScenarioStatus, StepErrorInfo, StepReport, StepStatus,
};
use crate::serial::ring::BridgeLine;

/// Wait 轮询睡眠分片（plan 指定 20–50ms，取中值 25ms）。
pub const WAIT_POLL_SLICE_MS: u64 = 25;
/// Wait 单批拉取行数上限（批消费后游标推进，剩余行下一轮继续拉）。
pub const WAIT_BATCH_LINES: usize = 1024;
/// skipped 报告生成预算（防御绕过校验的手工场景；经校验的场景 executed+skipped
/// 恒 ≤ [`MAX_EXECUTED_STEPS`]，预算不会截断合法报告）。
const SKIPPED_BUDGET: u64 = MAX_EXECUTED_STEPS;

/// 宿主操作错误（稳定错误码见模块文档）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// 背压：host 发送/接收队列满或被限流，操作被拒。
    Backpressure,
    /// 传输/会话层故障（连接断开、hex 解码失败、当前会话不支持该操作等），
    /// 携带 host 提供的具体消息。
    Transport(String),
    /// 取消（仅 [`ScenarioHost::sleep`] 约定返回）：`cancel` 已置位。
    Cancelled,
}

impl HostError {
    /// 稳定错误码（GUI/CLI 按码分支，勿改字面量）。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Backpressure => "host_backpressure",
            Self::Transport(_) => "host_transport",
            Self::Cancelled => "host_cancelled",
        }
    }

    /// 人类可读消息（`Transport` 透传 host 消息）。
    pub fn message(&self) -> &str {
        match self {
            Self::Backpressure => "operation rejected: host backpressure",
            Self::Transport(message) => message,
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for HostError {}

/// 场景执行宿主：runner 与具体传输（桌面会话/CLI 连接/测试假件）之间的窄接口。
pub trait ScenarioHost {
    /// 发送文本（编码语义由 host 按 `mode` 解释）。
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError>;
    /// 置信号线电平（不支持信号线的会话报 [`HostError::Transport`]）。
    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError>;
    /// ring 末行 `no`（无行 = 0）。
    fn last_no(&self) -> u64;
    /// `no > since` 的最旧 `max` 行，按 `no` 升序（对齐 `RingBuf::lines_after_no`）。
    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError>;
    /// 可取消睡眠：进入时或睡眠中 `cancel` 置位须尽快返回 [`HostError::Cancelled`]。
    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError>;
    /// 当前墙钟毫秒（报告时间戳与 Wait deadline 的唯一时间源；可注入假钟）。
    fn now_ms(&self) -> u64;
}

/// 执行场景（迭代式；确定性：同输入 + 同 host 行为 + 同钟 → 相同报告）。
pub fn run_scenario(
    scenario: &ValidatedScenario,
    host: &mut dyn ScenarioHost,
    cancel: &AtomicBool,
) -> ScenarioReport {
    let started = host.now_ms();
    let mut vars = scenario.variables.clone();
    let baseline = host.last_no();
    let mut cursor = baseline;
    let mut reports: Vec<StepReport> = Vec::new();
    let mut executed: u64 = 0;
    let mut final_status: Option<ScenarioStatus> = None;

    // 显式栈帧（steps 持克隆：每场景一次深拷贝、Regex 为 Arc 引用计数近乎免费，
    // 换取无自引用借用的迭代式所有权）。
    let mut stack: Vec<Frame> = vec![Frame {
        steps: scenario.steps.clone(),
        index: 0,
        path: "steps".to_string(),
        repeat: None,
    }];

    'run: loop {
        // 帧耗尽管理：Repeat 还有剩余迭代则开启下一轮，否则弹栈；栈空 = 全部完成
        loop {
            let Some(frame) = stack.last_mut() else {
                break 'run;
            };
            if frame.index < frame.steps.len() {
                break;
            }
            let restart = match &mut frame.repeat {
                Some(rep) if rep.remaining > 1 => {
                    rep.remaining -= 1;
                    rep.iteration += 1;
                    true
                }
                _ => false,
            };
            if restart {
                frame.index = 0;
            } else {
                stack.pop();
            }
        }
        // 取消检查：每个步（叶子/Repeat）开始前
        if cancel.load(Ordering::Relaxed) {
            final_status = Some(ScenarioStatus::Cancelled);
            break 'run;
        }
        let (index, block_path) = {
            let frame = stack.last().expect("frame exists");
            (frame.index, frame.path.clone())
        };
        let step_path = format!("{block_path}[{index}]");
        let step = stack.last().expect("frame exists").steps[index].clone();
        stack.last_mut().expect("frame exists").index = index + 1;

        if let ValidatedStep::Repeat { times, steps } = &step {
            if *times > 0 {
                stack.push(Frame {
                    steps: steps.clone(),
                    index: 0,
                    path: format!("{step_path}.steps"),
                    repeat: Some(RepeatState {
                        remaining: *times,
                        iteration: 0,
                    }),
                });
            }
            continue 'run; // times=0：整体跳过（无叶子无报告）
        }

        // 叶子步：运行期步数上限（第 MAX_EXECUTED_STEPS+1 个叶子触发）
        if executed >= MAX_EXECUTED_STEPS {
            let now = host.now_ms();
            reports.push(StepReport {
                path: format!("{step_path}{}", iter_suffix(&stack)),
                kind: kind_of(&step),
                started_epoch_ms: now,
                duration_ms: 0,
                matched_no: None,
                error: Some(step_error(
                    "step_limit_exceeded",
                    format!("executed step budget of {MAX_EXECUTED_STEPS} exhausted"),
                )),
                status: StepStatus::Failed,
            });
            final_status = Some(ScenarioStatus::Failed);
            break 'run;
        }
        executed += 1;
        let path = format!("{step_path}{}", iter_suffix(&stack));
        let outcome = run_leaf(&step, &path, host, cancel, &mut vars, &mut cursor);
        reports.push(outcome.report);
        if let Some(status) = outcome.abort {
            final_status = Some(status);
            break 'run;
        }
        // 步后取消检查（host 调用之后的步级体现）：已完成的步保持 passed
        if cancel.load(Ordering::Relaxed) {
            final_status = Some(ScenarioStatus::Cancelled);
            break 'run;
        }
    }

    let finished = host.now_ms();
    let status = final_status.unwrap_or(ScenarioStatus::Passed);
    // 未执行步全部补 skipped（按本会执行的顺序展开）
    skip_remaining(&stack, finished, &mut reports);
    ScenarioReport {
        name: scenario.name.clone(),
        status,
        started_epoch_ms: started,
        finished_epoch_ms: finished,
        duration_ms: finished.saturating_sub(started),
        variables: vars,
        steps: reports,
    }
}

// ============ 内部：显式栈帧与叶子步执行 ============

/// Repeat 帧状态：`remaining` 含当前迭代（进入时 = times）。
struct RepeatState {
    remaining: u32,
    iteration: u32,
}

/// 执行栈帧：一个线性步骤块（根块或某层 Repeat 体）。
struct Frame {
    steps: Vec<ValidatedStep>,
    index: usize,
    /// 块路径（`steps` 或 `steps[i].steps`），叶子路径 = `{path}[{i}]` + 迭代后缀。
    path: String,
    repeat: Option<RepeatState>,
}

/// 叶子步执行结果：报告 + 是否中止整个运行（failed / cancelled）。
struct LeafOutcome {
    report: StepReport,
    abort: Option<ScenarioStatus>,
}

fn kind_of(step: &ValidatedStep) -> &'static str {
    match step {
        ValidatedStep::Send { .. } => "send",
        ValidatedStep::Delay { .. } => "delay",
        ValidatedStep::Signal { .. } => "signal",
        ValidatedStep::Wait { .. } => "wait",
        ValidatedStep::Assert { .. } => "assert",
        ValidatedStep::Repeat { .. } => "repeat",
    }
}

fn step_error(code: &str, message: impl Into<String>) -> StepErrorInfo {
    StepErrorInfo {
        code: code.to_string(),
        message: message.into(),
    }
}

fn host_step_error(e: &HostError) -> StepErrorInfo {
    step_error(e.code(), e.message())
}

fn skipped_report(path: String, kind: &'static str, started: u64, duration: u64) -> StepReport {
    StepReport {
        path,
        kind,
        started_epoch_ms: started,
        duration_ms: duration,
        matched_no: None,
        error: Some(step_error("cancelled", "cancelled")),
        status: StepStatus::Skipped,
    }
}

fn passed_report(
    path: String,
    kind: &'static str,
    started: u64,
    now: u64,
    matched_no: Option<u64>,
) -> StepReport {
    StepReport {
        path,
        kind,
        started_epoch_ms: started,
        duration_ms: now.saturating_sub(started),
        matched_no,
        error: None,
        status: StepStatus::Passed,
    }
}

fn failed_report(
    path: String,
    kind: &'static str,
    started: u64,
    now: u64,
    matched_no: Option<u64>,
    error: StepErrorInfo,
) -> StepReport {
    StepReport {
        path,
        kind,
        started_epoch_ms: started,
        duration_ms: now.saturating_sub(started),
        matched_no,
        error: Some(error),
        status: StepStatus::Failed,
    }
}

fn interrupted(path: String, kind: &'static str, started: u64, now: u64) -> LeafOutcome {
    LeafOutcome {
        report: skipped_report(path, kind, started, now.saturating_sub(started)),
        abort: Some(ScenarioStatus::Cancelled),
    }
}

fn failed(
    path: String,
    kind: &'static str,
    started: u64,
    now: u64,
    error: StepErrorInfo,
) -> LeafOutcome {
    LeafOutcome {
        report: failed_report(path, kind, started, now, None, error),
        abort: Some(ScenarioStatus::Failed),
    }
}

/// 迭代后缀：栈上各层 Repeat 帧的当前迭代（0 基）串接，如 `#1#0`。
fn iter_suffix(stack: &[Frame]) -> String {
    let mut suffix = String::new();
    for frame in stack {
        if let Some(rep) = &frame.repeat {
            suffix.push('#');
            suffix.push_str(&rep.iteration.to_string());
        }
    }
    suffix
}

/// Wait save 捕获值：regex 对命中行 text 取 captures（组未参与存空串）；
/// literal/hex/mask（校验层只允许 group 0）= 命中行整行 text。
fn capture_value(matcher: &CompiledMatcher, spec: &CaptureSpec, line: &BridgeLine) -> String {
    match matcher.regex() {
        Some(re) => re
            .captures(&line.text)
            .and_then(|caps| caps.get(spec.group))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default(),
        None => line.text.clone(),
    }
}

/// 执行一个叶子步。取消检查点：host 调用前（步入口 / Wait 每轮拉取与睡眠前）、
/// sleep 内部（经 cancel 标志）、每步成功完成后的步级复查在 run_scenario 主循环。
fn run_leaf(
    step: &ValidatedStep,
    path: &str,
    host: &mut dyn ScenarioHost,
    cancel: &AtomicBool,
    vars: &mut BTreeMap<String, String>,
    cursor: &mut u64,
) -> LeafOutcome {
    let kind = kind_of(step);
    let started = host.now_ms();
    match step {
        ValidatedStep::Send {
            mode,
            template,
            append_newline,
        } => {
            if cancel.load(Ordering::Relaxed) {
                return interrupted(path.to_string(), kind, started, started);
            }
            let mut payload = match substitute(template, vars) {
                Ok(text) => text,
                Err(e) => {
                    return failed(
                        path.to_string(),
                        kind,
                        started,
                        host.now_ms(),
                        step_error("undefined_variable", e.to_string()),
                    )
                }
            };
            if *append_newline {
                payload.push('\n');
            }
            match host.send(*mode, &payload) {
                Ok(()) => LeafOutcome {
                    report: passed_report(path.to_string(), kind, started, host.now_ms(), None),
                    abort: None,
                },
                Err(e) => failed(
                    path.to_string(),
                    kind,
                    started,
                    host.now_ms(),
                    host_step_error(&e),
                ),
            }
        }
        ValidatedStep::Delay { ms } => match host.sleep(Duration::from_millis(*ms), cancel) {
            Ok(()) => LeafOutcome {
                report: passed_report(path.to_string(), kind, started, host.now_ms(), None),
                abort: None,
            },
            Err(HostError::Cancelled) => {
                interrupted(path.to_string(), kind, started, host.now_ms())
            }
            Err(e) => failed(
                path.to_string(),
                kind,
                started,
                host.now_ms(),
                host_step_error(&e),
            ),
        },
        ValidatedStep::Signal { pin, level } => {
            if cancel.load(Ordering::Relaxed) {
                return interrupted(path.to_string(), kind, started, started);
            }
            match host.signal(*pin, *level) {
                Ok(()) => LeafOutcome {
                    report: passed_report(path.to_string(), kind, started, host.now_ms(), None),
                    abort: None,
                },
                Err(e) => failed(
                    path.to_string(),
                    kind,
                    started,
                    host.now_ms(),
                    host_step_error(&e),
                ),
            }
        }
        ValidatedStep::Wait {
            matcher,
            timeout_ms,
            save,
        } => {
            let deadline = started.saturating_add(*timeout_ms);
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return interrupted(path.to_string(), kind, started, host.now_ms());
                }
                match host.lines_after(*cursor, WAIT_BATCH_LINES) {
                    Err(e) => {
                        return failed(
                            path.to_string(),
                            kind,
                            started,
                            host.now_ms(),
                            host_step_error(&e),
                        )
                    }
                    Ok(batch) => {
                        // 观察过的批次即消费：无论命中与否，游标推进到批内最大 no
                        if let Some(newest) = batch.last() {
                            *cursor = newest.no;
                        }
                        if let Some(hit) = batch.iter().find(|line| {
                            matcher.matches(line.dir, &line.text, line.bytes.as_deref())
                        }) {
                            if let Some(spec) = save {
                                vars.insert(
                                    spec.variable.clone(),
                                    capture_value(matcher, spec, hit),
                                );
                            }
                            return LeafOutcome {
                                report: passed_report(
                                    path.to_string(),
                                    kind,
                                    started,
                                    host.now_ms(),
                                    Some(hit.no),
                                ),
                                abort: None,
                            };
                        }
                    }
                }
                let now = host.now_ms();
                if now >= deadline {
                    return failed(
                        path.to_string(),
                        kind,
                        started,
                        now,
                        step_error(
                            "wait_timeout",
                            format!("no matching line within {timeout_ms} ms"),
                        ),
                    );
                }
                let slice = (deadline - now).min(WAIT_POLL_SLICE_MS);
                match host.sleep(Duration::from_millis(slice), cancel) {
                    Err(HostError::Cancelled) => {
                        return interrupted(path.to_string(), kind, started, host.now_ms())
                    }
                    Err(e) => {
                        return failed(
                            path.to_string(),
                            kind,
                            started,
                            host.now_ms(),
                            host_step_error(&e),
                        )
                    }
                    Ok(()) => {}
                }
            }
        }
        ValidatedStep::Assert {
            matcher,
            within_last,
            template,
        } => {
            // 回看「最近 within_last 行」（0 视为 1：只看最后一行）；不消费 Wait 游标
            let last = host.last_no();
            let n = (*within_last).max(1);
            let since = last.saturating_sub(n as u64);
            let lines = match host.lines_after(since, n) {
                Ok(lines) => lines,
                Err(e) => {
                    return failed(
                        path.to_string(),
                        kind,
                        started,
                        host.now_ms(),
                        host_step_error(&e),
                    )
                }
            };
            let mut matched_no = None;
            for line in &lines {
                if matcher.matches(line.dir, &line.text, line.bytes.as_deref()) {
                    matched_no = Some(line.no);
                }
            }
            match matched_no {
                Some(no) => LeafOutcome {
                    report: passed_report(path.to_string(), kind, started, host.now_ms(), Some(no)),
                    abort: None,
                },
                None => {
                    let message = substitute(template, vars).unwrap_or_else(|e| e.to_string());
                    failed(
                        path.to_string(),
                        kind,
                        started,
                        host.now_ms(),
                        step_error("assert_failed", message),
                    )
                }
            }
        }
        ValidatedStep::Repeat { .. } => {
            unreachable!("repeat frames are dispatched by run_scenario")
        }
    }
}

// ============ 内部：中止后未执行步的 skipped 补齐 ============

/// 中止后把未执行叶子按本会执行的顺序补 skipped 条目：自栈顶向栈底展开，每帧先补
/// 剩余兄弟、再补 Repeat 剩余迭代整体（预算 [`SKIPPED_BUDGET`] 防失控）。
fn skip_remaining(stack: &[Frame], now: u64, out: &mut Vec<StepReport>) {
    let mut budget = SKIPPED_BUDGET;
    for depth in (0..stack.len()).rev() {
        let frame = &stack[depth];
        // 外层帧的迭代后缀（不含本帧——本帧后缀按当前/未来迭代分别拼）
        let mut base = String::new();
        for upper in &stack[..depth] {
            if let Some(rep) = &upper.repeat {
                base.push('#');
                base.push_str(&rep.iteration.to_string());
            }
        }
        if frame.index < frame.steps.len() {
            let current = match &frame.repeat {
                Some(rep) => format!("{base}#{}", rep.iteration),
                None => base.clone(),
            };
            push_skipped_block(
                &frame.steps,
                frame.index,
                &frame.path,
                &current,
                now,
                &mut budget,
                out,
            );
        }
        if let Some(rep) = &frame.repeat {
            for it in (rep.iteration + 1)..(rep.iteration + rep.remaining) {
                push_skipped_block(
                    &frame.steps,
                    0,
                    &frame.path,
                    &format!("{base}#{it}"),
                    now,
                    &mut budget,
                    out,
                );
            }
        }
    }
}

/// 迭代展开 `steps[start..]` 的全部可达叶子（含嵌套 Repeat），逐个生成 skipped 报告。
/// 内层帧只引用 `steps` 派生的切片（不引用自身栈元素），无自引用借用。
fn push_skipped_block<'a>(
    steps: &'a [ValidatedStep],
    start: usize,
    path: &str,
    suffix: &str,
    now: u64,
    budget: &mut u64,
    out: &mut Vec<StepReport>,
) {
    struct SkipFrame<'a> {
        steps: &'a [ValidatedStep],
        index: usize,
        path: String,
        remaining: u32,
        iteration: u32,
    }
    let mut stack: Vec<SkipFrame<'a>> = vec![SkipFrame {
        steps,
        index: start,
        path: path.to_string(),
        remaining: 1,
        iteration: 0,
    }];
    loop {
        // 帧耗尽管理
        loop {
            let Some(top) = stack.last_mut() else {
                return;
            };
            if top.index < top.steps.len() {
                break;
            }
            if top.remaining > 1 {
                top.remaining -= 1;
                top.iteration += 1;
                top.index = 0;
            } else {
                stack.pop();
            }
        }
        if *budget == 0 {
            return;
        }
        let (index, block_path) = {
            let top = stack.last().expect("checked above");
            (top.index, top.path.clone())
        };
        let step_path = format!("{block_path}[{index}]");
        stack.last_mut().expect("checked above").index = index + 1;
        // 叶子后缀 = 外层后缀 + 本 walk 各层 Repeat 的当前迭代（根帧不计，suffix 已含外层）
        let leaf_suffix = {
            let mut s = String::from(suffix);
            for frame in stack.iter().skip(1) {
                s.push('#');
                s.push_str(&frame.iteration.to_string());
            }
            s
        };
        // 复制出当前帧的切片（&'a 与 skip 栈无关），避免 match 期间借用栈顶元素
        let frame_steps: &'a [ValidatedStep] = stack.last().expect("checked above").steps;
        match &frame_steps[index] {
            ValidatedStep::Repeat { times, steps } => {
                let (times, inner) = (*times, steps.as_slice());
                if times > 0 {
                    stack.push(SkipFrame {
                        steps: inner,
                        index: 0,
                        path: format!("{step_path}.steps"),
                        remaining: times,
                        iteration: 0,
                    });
                }
            }
            step => {
                *budget -= 1;
                out.push(skipped_report(
                    format!("{step_path}{leaf_suffix}"),
                    kind_of(step),
                    now,
                    0,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests;
