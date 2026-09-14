//! `automation::runner` 单元测试（内联测试模块外移，行数限额 compliance）。

use super::*;
use crate::automation::matcher::{compile_matcher, LineMatcher};
use crate::automation::model::{
    validate_scenario, MatcherTemplate, Scenario, ScenarioStep, SCENARIO_SCHEMA, SCENARIO_VERSION,
};
use crate::automation::report::report_json;
use crate::serial::port::Dir;
use std::sync::Arc;

// ---------- 场景构造 ----------

fn lit(dir: Option<Dir>, text: &str) -> LineMatcher {
    LineMatcher {
        dir,
        literal: Some(text.into()),
        regex: None,
        hex: None,
        mask: None,
    }
}
fn rx_lit(text: &str) -> LineMatcher {
    lit(Some(Dir::Rx), text)
}
fn re(dir: Option<Dir>, pattern: &str) -> LineMatcher {
    LineMatcher {
        dir,
        literal: None,
        regex: Some(pattern.into()),
        hex: None,
        mask: None,
    }
}
fn hexm(dir: Option<Dir>, hex: &str) -> LineMatcher {
    LineMatcher {
        dir,
        literal: None,
        regex: None,
        hex: Some(hex.into()),
        mask: None,
    }
}
fn maskm(dir: Option<Dir>, mask: &str) -> LineMatcher {
    LineMatcher {
        dir,
        literal: None,
        regex: None,
        hex: None,
        mask: Some(mask.into()),
    }
}
fn send(text: &str, nl: bool) -> ScenarioStep {
    ScenarioStep::Send {
        mode: SendModeDef::Ascii,
        text: text.into(),
        append_newline: nl,
    }
}
fn send_hex(text: &str, nl: bool) -> ScenarioStep {
    ScenarioStep::Send {
        mode: SendModeDef::Hex,
        text: text.into(),
        append_newline: nl,
    }
}
fn delay(ms: u64) -> ScenarioStep {
    ScenarioStep::Delay { ms }
}
fn signal(pin: PinDef, level: bool) -> ScenarioStep {
    ScenarioStep::Signal { pin, level }
}
fn wait(m: LineMatcher, timeout_ms: u64) -> ScenarioStep {
    ScenarioStep::Wait {
        matcher: m,
        timeout_ms,
        save: None,
    }
}
fn wait_save(m: LineMatcher, timeout_ms: u64, var: &str, group: usize) -> ScenarioStep {
    ScenarioStep::Wait {
        matcher: m,
        timeout_ms,
        save: Some(CaptureSpec {
            variable: var.into(),
            group,
        }),
    }
}
fn assert_last(m: LineMatcher, within_last: usize, message: &str) -> ScenarioStep {
    ScenarioStep::Assert {
        matcher: m,
        within_last,
        message: message.into(),
    }
}
fn repeat(times: u32, steps: Vec<ScenarioStep>) -> ScenarioStep {
    ScenarioStep::Repeat { times, steps }
}
fn validated(vars: &[(&str, &str)], steps: Vec<ScenarioStep>) -> ValidatedScenario {
    let mut variables = BTreeMap::new();
    for (k, v) in vars {
        variables.insert((*k).to_string(), (*v).to_string());
    }
    validate_scenario(Scenario {
        schema: SCENARIO_SCHEMA.into(),
        version: SCENARIO_VERSION,
        name: "t".into(),
        variables,
        steps,
    })
    .expect("scenario must validate")
}

/// 手工构造（绕过静态校验）：runner 对畸形场景的防御路径测试用
/// （timeoutMs=0 / times=0 / 未定义引用已在校验层拒绝，运行期兜底仍须生效）。
fn manual(steps: Vec<ValidatedStep>) -> ValidatedScenario {
    ValidatedScenario {
        name: "t".into(),
        variables: BTreeMap::new(),
        steps,
    }
}

// ---------- FakeHost（假钟 + 行表 + 可编程错误/注入 + 调用序） ----------

struct FakeHost {
    now: u64,
    lines: Vec<BridgeLine>,
    next_no: u64,
    sends: Vec<(SendModeDef, String)>,
    signals: Vec<(PinDef, bool)>,
    sleeps_ms: Vec<u64>,
    calls: Vec<&'static str>,
    err_send: Option<HostError>,
    err_signal: Option<HostError>,
    err_lines: Option<HostError>,
    err_sleep: Option<HostError>,
    auto_reply: Option<BridgeLine>,
    scheduled: Vec<(u64, BridgeLine)>,
    cancel_sleep_at: Option<u64>,
    cancel_after_calls: Option<usize>,
    cancel: Arc<AtomicBool>,
}

impl Default for FakeHost {
    fn default() -> Self {
        Self {
            now: 1_000_000,
            lines: Vec::new(),
            next_no: 1,
            sends: Vec::new(),
            signals: Vec::new(),
            sleeps_ms: Vec::new(),
            calls: Vec::new(),
            err_send: None,
            err_signal: None,
            err_lines: None,
            err_sleep: None,
            auto_reply: None,
            scheduled: Vec::new(),
            cancel_sleep_at: None,
            cancel_after_calls: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl FakeHost {
    fn new() -> Self {
        Self::default()
    }
    fn make_line(&self, dir: Dir, text: &str, bytes: Option<&[u8]>) -> BridgeLine {
        BridgeLine {
            no: 0,
            ts: "2026-01-01T00:00:00.000Z".into(),
            dir,
            text: text.into(),
            bytes: bytes.map(<[u8]>::to_vec),
            epoch_millis: self.now,
            r#match: None,
        }
    }
    fn push_line(&mut self, dir: Dir, text: &str, bytes: Option<&[u8]>) -> u64 {
        let no = self.next_no;
        let mut line = self.make_line(dir, text, bytes);
        line.no = no;
        self.next_no += 1;
        self.lines.push(line);
        no
    }
    /// 场景开始前已存在的行（对 Wait 不可见——baseline 语义）。
    #[must_use]
    fn preset(mut self, dir: Dir, text: &str) -> Self {
        self.push_line(dir, text, None);
        self
    }
    /// 每次 send 自动注入的应答行（请求→响应建模）。
    #[must_use]
    fn reply(mut self, dir: Dir, text: &str) -> Self {
        self.auto_reply = Some(self.make_line(dir, text, None));
        self
    }
    #[must_use]
    fn reply_bytes(mut self, text: &str, bytes: &[u8]) -> Self {
        self.auto_reply = Some(self.make_line(Dir::Rx, text, Some(bytes)));
        self
    }
    /// 睡眠累计到达 `at` 毫秒时刻注入的行（响应延迟到达）。
    #[must_use]
    fn schedule(mut self, at: u64, dir: Dir, text: &str) -> Self {
        let line = self.make_line(dir, text, None);
        let pos = self.scheduled.partition_point(|(t, _)| *t <= at);
        self.scheduled.insert(pos, (at, line));
        self
    }
    #[must_use]
    fn fail_send(mut self, e: HostError) -> Self {
        self.err_send = Some(e);
        self
    }
    #[must_use]
    fn fail_signal(mut self, e: HostError) -> Self {
        self.err_signal = Some(e);
        self
    }
    #[must_use]
    fn fail_lines(mut self, e: HostError) -> Self {
        self.err_lines = Some(e);
        self
    }
    #[must_use]
    fn fail_sleep(mut self, e: HostError) -> Self {
        self.err_sleep = Some(e);
        self
    }
    /// 睡眠跨过 `at`（host 时钟毫秒）时刻自动置位 cancel 并中断该次睡眠。
    #[must_use]
    fn cancel_sleep_at(mut self, at: u64) -> Self {
        self.cancel_sleep_at = Some(at);
        self
    }
    /// 第 n 次业务调用完成后自动置位 cancel。
    #[must_use]
    fn cancel_after(mut self, n: usize) -> Self {
        self.cancel_after_calls = Some(n);
        self
    }
    fn inject_due(&mut self) {
        while self
            .scheduled
            .first()
            .is_some_and(|(at, _)| *at <= self.now)
        {
            let (_, mut line) = self.scheduled.remove(0);
            line.no = self.next_no;
            self.next_no += 1;
            line.epoch_millis = self.now;
            self.lines.push(line);
        }
    }
    fn apply_cancel_after(&mut self) {
        if self
            .cancel_after_calls
            .is_some_and(|n| self.calls.len() >= n)
        {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl ScenarioHost for FakeHost {
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        self.calls.push("send");
        if let Some(e) = self.err_send.take() {
            return Err(e);
        }
        self.sends.push((mode, text.into()));
        if let Some(mut reply) = self.auto_reply.clone() {
            reply.no = self.next_no;
            self.next_no += 1;
            reply.epoch_millis = self.now;
            self.lines.push(reply);
        }
        self.apply_cancel_after();
        Ok(())
    }
    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError> {
        self.calls.push("signal");
        if let Some(e) = self.err_signal.take() {
            return Err(e);
        }
        self.signals.push((pin, level));
        self.apply_cancel_after();
        Ok(())
    }
    fn last_no(&self) -> u64 {
        self.lines.last().map(|l| l.no).unwrap_or(0)
    }
    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.calls.push("lines_after");
        if let Some(e) = self.err_lines.take() {
            return Err(e);
        }
        let out: Vec<BridgeLine> = self
            .lines
            .iter()
            .filter(|l| l.no > since)
            .take(max)
            .cloned()
            .collect();
        self.apply_cancel_after();
        Ok(out)
    }
    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        self.calls.push("sleep");
        if let Some(e) = self.err_sleep.take() {
            return Err(e);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        let ms = duration.as_millis() as u64;
        self.sleeps_ms.push(ms);
        let target = self.now + ms;
        if let Some(at) = self.cancel_sleep_at {
            if self.now < at && target >= at {
                self.now = at;
                self.inject_due();
                cancel.store(true, Ordering::Relaxed);
                return Err(HostError::Cancelled);
            }
        }
        self.now = target;
        self.inject_due();
        self.apply_cancel_after();
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        Ok(())
    }
    fn now_ms(&self) -> u64 {
        self.now
    }
}

fn run(host: &mut FakeHost, s: &ValidatedScenario) -> ScenarioReport {
    let cancel = Arc::clone(&host.cancel);
    run_scenario(s, host, &cancel)
}

// ---------- send / delay / signal ----------

#[test]
fn send_substitutes_variables_and_appends_newline() {
    let s = validated(&[("greeting", "hello")], vec![send("${greeting}!", true)]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.sends, vec![(SendModeDef::Ascii, "hello!\n".to_string())]);
    assert_eq!(h.calls, vec!["send"]);
    let step = &r.steps[0];
    assert_eq!(step.path, "steps[0]");
    assert_eq!(step.kind, "send");
    assert_eq!(step.status, StepStatus::Passed);
    assert_eq!(step.matched_no, None);
    assert!(step.error.is_none());
    // 报告时间来自 host 假钟
    assert_eq!(step.started_epoch_ms, 1_000_000);
    assert_eq!(step.duration_ms, 0);
}

#[test]
fn send_hex_mode_without_newline_passes_text_through() {
    let s = validated(&[("token", "7f")], vec![send_hex("aa ${token}", false)]);
    let mut h = FakeHost::new();
    run(&mut h, &s);
    assert_eq!(h.sends, vec![(SendModeDef::Hex, "aa 7f".to_string())]);
}

#[test]
fn delay_sleeps_exactly_requested_ms() {
    let s = validated(&[], vec![delay(120)]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.sleeps_ms, vec![120]);
    assert_eq!(h.calls, vec!["sleep"]);
    assert_eq!(r.steps[0].kind, "delay");
    assert_eq!(r.steps[0].started_epoch_ms, 1_000_000);
    assert_eq!(r.steps[0].duration_ms, 120);
    assert_eq!(r.finished_epoch_ms, 1_000_120);
    assert_eq!(r.duration_ms, 120);
}

#[test]
fn signal_records_pin_and_level() {
    let s = validated(
        &[],
        vec![signal(PinDef::Dtr, true), signal(PinDef::Rts, false)],
    );
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.signals, vec![(PinDef::Dtr, true), (PinDef::Rts, false)]);
    assert_eq!(r.steps.len(), 2);
    assert_eq!(r.steps[1].path, "steps[1]");
    assert_eq!(r.steps[1].kind, "signal");
}

// ---------- wait：命中 / 超时 / dir / hex / mask ----------

#[test]
fn wait_matches_reply_line_and_reports_matched_no() {
    let s = validated(&[], vec![send("PING", true), wait(rx_lit("OK"), 200)]);
    let mut h = FakeHost::new().reply(Dir::Rx, "device OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.sends, vec![(SendModeDef::Ascii, "PING\n".to_string())]);
    assert_eq!(r.steps[1].kind, "wait");
    assert_eq!(r.steps[1].matched_no, Some(1));
}

#[test]
fn wait_ignores_lines_that_existed_before_scenario_start() {
    // baseline = 场景开始时的 last_no：已有行对 Wait 不可见（游标语义钉死）
    let s = validated(&[], vec![wait(rx_lit("OK"), 50)]);
    let mut h = FakeHost::new().preset(Dir::Rx, "OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let step = &r.steps[0];
    assert_eq!(step.status, StepStatus::Failed);
    assert_eq!(
        step.error.as_ref().map(|e| e.code.as_str()),
        Some("wait_timeout")
    );
    // deadline 精确：总睡眠 = timeout，不多不少
    assert_eq!(h.sleeps_ms, vec![25, 25]);
    assert_eq!(r.finished_epoch_ms, 1_000_050);
}

#[test]
fn wait_zero_timeout_still_polls_once() {
    // timeoutMs=0 已被静态校验拒绝（下界 1，wait_too_short）；此处手工构造验证
    // runner 的防御路径：0 仍保证一次拉取机会、不进入轮询睡眠
    let s = manual(vec![ValidatedStep::Wait {
        matcher: MatcherTemplate::Static(compile_matcher(&rx_lit("NEVER")).expect("compile")),
        timeout_ms: 0,
        save: None,
    }]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(h.calls, vec!["lines_after"]);
    assert!(h.sleeps_ms.is_empty());
}

#[test]
fn wait_dir_filter_and_any_dir() {
    // dir=rx 不匹配 TX 行（reply 行在 send 时注入）
    let s = validated(&[], vec![send("PING", true), wait(rx_lit("OK"), 50)]);
    let mut h = FakeHost::new().reply(Dir::Tx, "OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);

    // dir 缺省匹配任意方向
    let s = validated(&[], vec![send("PING", true), wait(lit(None, "OK"), 50)]);
    let mut h = FakeHost::new().reply(Dir::Tx, "OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
}

#[test]
fn wait_hex_prefers_raw_bytes_and_falls_back_to_text() {
    // 二进制行：lossy 文本无法再编码命中，必须走原始 bytes
    let s = validated(
        &[],
        vec![send("PING", true), wait(hexm(None, "00 ff"), 100)],
    );
    let mut h = FakeHost::new().reply_bytes("lossy \u{fffd}", &[0x01, 0x00, 0xff]);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);

    // 纯文本行：bytes 缺失回退 text 的 UTF-8 编码
    let s = validated(
        &[],
        vec![send("PING", true), wait(hexm(None, "61 e4 b8 ad"), 100)],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "xa中y");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
}

#[test]
fn wait_mask_wildcard_matches_bytes() {
    let s = validated(
        &[],
        vec![send("PING", true), wait(maskm(None, "5a ??"), 100)],
    );
    let mut h = FakeHost::new().reply_bytes("", &[0x5a, 0x99]);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);

    let s = validated(
        &[],
        vec![send("PING", true), wait(maskm(None, "5a ??"), 50)],
    );
    let mut h = FakeHost::new().reply_bytes("", &[0x5b, 0x99]);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
}

// ---------- 游标推进与捕获 ----------

#[test]
fn wait_cursor_advances_per_batch_and_sees_new_lines_only() {
    // wait1 命中应答 no1（游标→1）；delay 期间新行 no2 到达；wait2 只能命中 no2
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait(rx_lit("OK"), 100),
            delay(10),
            wait(rx_lit("OK"), 100),
        ],
    );
    let mut h = FakeHost::new()
        .reply(Dir::Rx, "OK")
        .schedule(1_000_005, Dir::Rx, "OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps[1].matched_no, Some(1));
    assert_eq!(r.steps[3].matched_no, Some(2), "第二个 Wait 只能看到新行");
}

#[test]
fn wait_does_not_rematch_consumed_lines() {
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait(rx_lit("OK"), 100),
            wait(rx_lit("OK"), 50),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "OK");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(r.steps[1].matched_no, Some(1));
    assert_eq!(r.steps[2].status, StepStatus::Failed);
    assert_eq!(
        r.steps[2].error.as_ref().map(|e| e.code.as_str()),
        Some("wait_timeout")
    );
}

#[test]
fn regex_capture_saves_group_and_feeds_substitution() {
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait_save(
                re(Some(Dir::Rx), r"^PONG ([0-9A-Fa-f]{2})$"),
                100,
                "value",
                1,
            ),
            send("ACK ${value}", false),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "PONG 4f");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.sends[1].1, "ACK 4f");
    assert_eq!(r.variables.get("value").map(String::as_str), Some("4f"));
}

#[test]
fn regex_capture_group_zero_is_whole_match_and_literal_captures_whole_line() {
    // regex group 0 = 整个正则匹配
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait_save(re(Some(Dir::Rx), r"^PONG [0-9A-Fa-f]{2}$"), 100, "m", 0),
            send("${m}", false),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "PONG 4f");
    run(&mut h, &s);
    assert_eq!(h.sends[1].1, "PONG 4f");

    // literal/hex/mask 只允许 group 0（校验层保证）= 命中行整行 text
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait_save(lit(None, "OK"), 100, "line", 0),
            send("[${line}]", false),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "device OK");
    run(&mut h, &s);
    assert_eq!(h.sends[1].1, "[device OK]");
}

#[test]
fn regex_non_participating_group_saves_empty_string() {
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait_save(re(None, r"PING(-x)?"), 100, "tail", 1),
            send("[${tail}]", false),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "PING");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(h.sends[1].1, "[]");
}

// ---------- matcher 模板变量替换（设计契约：${name} 可用于 matcher 模式） ----------

#[test]
fn wait_matcher_literal_substitutes_captured_variable() {
    // 先捕获 token，再用它构造后续 Wait 的 matcher（评审影响示例的最小复现）
    let s = validated(
        &[],
        vec![
            send("GET TOKEN", true),
            wait_save(re(Some(Dir::Rx), r"^TOKEN=(\w+)$"), 100, "token", 1),
            wait(lit(Some(Dir::Rx), "ACK ${token}"), 100),
        ],
    );
    let mut h =
        FakeHost::new()
            .reply(Dir::Rx, "TOKEN=ab12")
            .schedule(1_000_005, Dir::Rx, "ACK ab12");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed, "{r:?}");
    assert_eq!(r.steps[2].matched_no, Some(2));
}

#[test]
fn wait_matcher_dynamic_regex_and_undefined_variable_error() {
    // 动态 regex：替换后可编译并命中
    let s = validated(
        &[("addr", "4f")],
        vec![
            send("PING", true),
            wait(re(Some(Dir::Rx), r"^VAL ${addr}$"), 100),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "VAL 4f");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps[1].matched_no, Some(1));

    // 动态 regex 替换后非法（变量值破坏语法）→ 步级 invalid_regex
    let s = validated(
        &[("bad", "(unclosed")],
        vec![wait(re(None, "x${bad}y"), 100)],
    );
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let err = r.steps[0].error.as_ref().expect("error");
    assert_eq!(err.code, "invalid_regex");

    // 未定义引用在 matcher 模板 → 步级 undefined_variable（防御路径：
    // 静态校验已拒绝，手工构造验证 runner 兜底）
    let s = manual(vec![ValidatedStep::Wait {
        matcher: MatcherTemplate::Dynamic(lit(None, "${ghost}")),
        timeout_ms: 100,
        save: None,
    }]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(
        r.steps[0].error.as_ref().map(|e| e.code.as_str()),
        Some("undefined_variable")
    );
}

#[test]
fn assert_matcher_substitutes_and_dynamic_group_out_of_range_fails() {
    // assert matcher 含引用：替换后命中
    let mut h = FakeHost::new();
    h.push_line(Dir::Rx, "TEMP 42", None);
    let s = validated(
        &[("t", "42")],
        vec![assert_last(lit(None, "TEMP ${t}"), 1, "want temp")],
    );
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps[0].matched_no, Some(1));

    // 动态模板 save.group 越界 → 运行期 capture_group_invalid（静态模板在
    // validate 期已拦；动态模板的组数替换后才知道：${g} 展开成非捕获类，
    // 替换后的正则只有 1 个组，save.group=1 越界）
    let s = validated(
        &[("g", "[ab]")],
        vec![
            send("PING", true),
            wait_save(re(None, "^V ${g}$"), 100, "x", 1),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "V a");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(
        r.steps[1].error.as_ref().map(|e| e.code.as_str()),
        Some("capture_group_invalid")
    );
}

#[test]
fn dynamic_literal_rejects_nonzero_capture_group_at_runtime() {
    let s = validated(
        &[("token", "42")],
        vec![
            send("PING", true),
            wait_save(lit(None, "ACK ${token}"), 100, "saved", 1),
        ],
    );
    let mut h = FakeHost::new().reply(Dir::Rx, "ACK 42");
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(
        r.steps[1].error.as_ref().map(|e| e.code.as_str()),
        Some("capture_group_invalid")
    );
}

#[test]
fn dynamic_hex_and_mask_matchers_reject_nonzero_capture_group_at_runtime() {
    // 复审 R-P2-1 补全：动态 hex/mask 同 literal——只暴露 group 0，运行期越界
    // 报 capture_group_invalid（变量值须为合法 hex 对，否则先报 invalid_hex/
    // invalid_mask 而非本用例目标码）
    let hex_case = validated(
        &[("g", "0a")],
        vec![
            send("PING", true),
            wait_save(hexm(None, "50 ${g}"), 100, "x", 9),
        ],
    );
    let mut h = FakeHost::new().reply_bytes("bin", &[0x50, 0x0a]);
    let r = run(&mut h, &hex_case);
    assert_eq!(r.status, ScenarioStatus::Failed, "{r:?}");
    assert_eq!(
        r.steps[1].error.as_ref().map(|e| e.code.as_str()),
        Some("capture_group_invalid")
    );

    let mask_case = validated(
        &[("g", "4f")],
        vec![
            send("PING", true),
            wait_save(maskm(None, "5a ${g}"), 100, "x", 9),
        ],
    );
    let mut h = FakeHost::new().reply_bytes("msk", &[0x5a, 0x4f]);
    let r = run(&mut h, &mask_case);
    assert_eq!(r.status, ScenarioStatus::Failed, "{r:?}");
    assert_eq!(
        r.steps[1].error.as_ref().map(|e| e.code.as_str()),
        Some("capture_group_invalid")
    );
}

mod step_started;
