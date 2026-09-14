//! `automation::runner` 单元测试（内联测试模块外移，行数限额 compliance）。

use super::*;
use crate::automation::matcher::LineMatcher;
use crate::automation::model::{
    validate_scenario, Scenario, ScenarioStep, SCENARIO_SCHEMA, SCENARIO_VERSION,
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
    let s = validated(&[], vec![wait(rx_lit("NEVER"), 0)]);
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

// ---------- assert ----------

#[test]
fn assert_searches_last_n_lines_newest_match_wins() {
    let mut h = FakeHost::new();
    h.push_line(Dir::Rx, "ping 7", None);
    h.push_line(Dir::Rx, "ping 8", None);
    h.push_line(Dir::Rx, "ping 9", None);
    // 窗口 = 最近 2 行 {8, 9}：命中 8（matcher 不做变量替换，message 才替换）
    let s = validated(
        &[("v", "8")],
        vec![assert_last(lit(None, "ping 8"), 2, "expected ping ${v}")],
    );
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps[0].matched_no, Some(2));
    // 窗口外（7）与未命中（10）都失败，报错信息 = 替换后的 Assert.message
    let s = validated(
        &[("v", "10")],
        vec![assert_last(lit(None, "ping 10"), 2, "no ${v} seen")],
    );
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(r.steps[0].matched_no, None);
    let err = r.steps[0].error.as_ref().expect("error");
    assert_eq!(err.code, "assert_failed");
    assert_eq!(err.message, "no 10 seen");
}

#[test]
fn assert_zero_within_last_means_last_line_only() {
    let mut h = FakeHost::new();
    h.push_line(Dir::Rx, "A", None);
    h.push_line(Dir::Rx, "B", None);
    let s = validated(&[], vec![assert_last(lit(None, "B"), 0, "want B")]);
    let r = run(&mut h, &s);
    assert_eq!(
        r.status,
        ScenarioStatus::Passed,
        "withinLast=0 ≡ 只看最后一行"
    );

    let s = validated(&[], vec![assert_last(lit(None, "A"), 0, "want A")]);
    let r = run(&mut h, &s);
    assert_eq!(
        r.status,
        ScenarioStatus::Failed,
        "窗口只剩最后一行 B，A 不可见"
    );
    assert_eq!(
        r.steps[0].error.as_ref().map(|e| e.message.as_str()),
        Some("want A")
    );
}

#[test]
fn assert_window_larger_than_history_sees_everything() {
    let mut h = FakeHost::new();
    h.push_line(Dir::Rx, "old", None);
    let s = validated(&[], vec![assert_last(lit(None, "old"), 100, "m")]);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps[0].matched_no, Some(1));
}

#[test]
fn assert_on_empty_history_fails() {
    let s = validated(&[], vec![assert_last(lit(None, "x"), 5, "nothing there")]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(
        r.steps[0].error.as_ref().map(|e| e.message.as_str()),
        Some("nothing there")
    );
}

// ---------- repeat ----------

#[test]
fn nested_repeat_paths_and_order_are_zero_based() {
    let s = validated(
        &[],
        vec![repeat(
            2,
            vec![send("a", false), repeat(2, vec![send("b", false)])],
        )],
    );
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    let paths: Vec<&str> = r.steps.iter().map(|st| st.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "steps[0].steps[0]#0",
            "steps[0].steps[1].steps[0]#0#0",
            "steps[0].steps[1].steps[0]#0#1",
            "steps[0].steps[0]#1",
            "steps[0].steps[1].steps[0]#1#0",
            "steps[0].steps[1].steps[0]#1#1",
        ]
    );
    let texts: Vec<&str> = h.sends.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(texts, vec!["a", "b", "b", "a", "b", "b"]);
}

#[test]
fn repeat_zero_times_skips_body_without_reports() {
    let s = validated(
        &[],
        vec![repeat(0, vec![send("x", false)]), send("y", false)],
    );
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps.len(), 1, "repeat 本身不是叶子，times=0 无报告");
    assert_eq!(r.steps[0].path, "steps[1]");
    assert_eq!(h.sends.len(), 1);
}

#[test]
fn runtime_undefined_variable_fails_scenario() {
    // 静态校验对 Repeat 体保守放行（times=0 时捕获变量实际未定义），运行期兜底
    let s = validated(
        &[],
        vec![
            repeat(0, vec![wait_save(lit(None, "OK"), 100, "v", 0)]),
            send("${v}", false),
        ],
    );
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let err = r.steps[0].error.as_ref().expect("error");
    assert_eq!(err.code, "undefined_variable");
    assert_eq!(err.message, "undefined variable: ${v}");
    assert_eq!(r.steps.len(), 1, "失败后无 skipped 兄弟（repeat 已结束）");
}

// ---------- 步数上限 ----------

#[test]
fn runtime_step_limit_exceeded_fails_at_10001st_leaf() {
    // 手工构造（绕过静态校验）：101×100 = 10,100 个潜在叶子
    let s = ValidatedScenario {
        name: "oversized".into(),
        variables: BTreeMap::new(),
        steps: vec![ValidatedStep::Repeat {
            times: 101,
            steps: vec![ValidatedStep::Repeat {
                times: 100,
                steps: vec![ValidatedStep::Delay { ms: 0 }],
            }],
        }],
    };
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    // 10,000 个通过 + 1 个超限失败 + 99 个未执行迭代补 skipped = 10,100
    assert_eq!(r.steps.len(), 10_100);
    assert_eq!(
        r.steps[..10_000]
            .iter()
            .filter(|st| st.status == StepStatus::Passed)
            .count(),
        10_000
    );
    let failed = &r.steps[10_000];
    assert_eq!(failed.status, StepStatus::Failed);
    assert_eq!(
        failed.error.as_ref().map(|e| e.code.as_str()),
        Some("step_limit_exceeded")
    );
    assert_eq!(failed.path, "steps[0].steps[0].steps[0]#100#0");
    assert_eq!(r.steps[10_001].path, "steps[0].steps[0].steps[0]#100#1");
    assert_eq!(r.steps[10_099].path, "steps[0].steps[0].steps[0]#100#99");
    assert!(r.steps[10_001..]
        .iter()
        .all(|st| st.status == StepStatus::Skipped));
}

#[test]
fn static_limit_boundary_exactly_10_000_passes() {
    let s = validated(&[], vec![repeat(MAX_EXECUTED_STEPS as u32, vec![delay(0)])]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps.len(), 10_000);
}

// ---------- 取消 ----------

#[test]
fn cancel_before_start_marks_all_steps_skipped() {
    let s = validated(&[], vec![send("a", false), delay(5)]);
    let mut h = FakeHost::new();
    h.cancel.store(true, Ordering::Relaxed);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Cancelled);
    assert_eq!(h.sends.len(), 0);
    assert_eq!(r.steps.len(), 2);
    for st in &r.steps {
        assert_eq!(st.status, StepStatus::Skipped);
        assert_eq!(
            st.error.as_ref().map(|e| e.code.as_str()),
            Some("cancelled")
        );
        assert_eq!(st.started_epoch_ms, r.started_epoch_ms);
    }
    assert_eq!(r.started_epoch_ms, r.finished_epoch_ms);
    assert_eq!(r.duration_ms, 0);
}

#[test]
fn cancel_during_delay_sleep_interrupts_run() {
    let s = validated(&[], vec![send("a", false), delay(100), send("b", false)]);
    let mut h = FakeHost::new().cancel_sleep_at(1_000_050);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Cancelled);
    assert_eq!(h.sends.len(), 1, "已完成的 send 保持 passed");
    assert_eq!(r.steps[0].status, StepStatus::Passed);
    // 被中断的 delay：skipped + cancelled，耗时记到取消时刻
    assert_eq!(r.steps[1].status, StepStatus::Skipped);
    assert_eq!(r.steps[1].started_epoch_ms, 1_000_000);
    assert_eq!(r.steps[1].duration_ms, 50);
    assert_eq!(r.steps[2].status, StepStatus::Skipped, "未执行步补 skipped");
    assert_eq!(r.steps[2].path, "steps[2]");
    assert_eq!(r.finished_epoch_ms, 1_000_050);
}

#[test]
fn cancel_during_wait_polling_interrupts_run() {
    let s = validated(&[], vec![wait(lit(None, "NEVER"), 1_000), send("b", false)]);
    let mut h = FakeHost::new().cancel_sleep_at(1_000_060);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Cancelled);
    assert_eq!(r.steps[0].status, StepStatus::Skipped);
    assert_eq!(
        r.steps[0].error.as_ref().map(|e| e.code.as_str()),
        Some("cancelled")
    );
    assert_eq!(r.steps[1].status, StepStatus::Skipped);
    // 25 + 25 + 跨 60 的分片中断
    assert_eq!(h.sleeps_ms, vec![25, 25, 25]);
    assert_eq!(r.finished_epoch_ms, 1_000_060);
}

#[test]
fn cancel_after_completed_step_keeps_it_passed() {
    let s = validated(&[], vec![send("a", false), send("b", false)]);
    let mut h = FakeHost::new().cancel_after(1);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Cancelled);
    assert_eq!(h.sends.len(), 1);
    assert_eq!(r.steps[0].status, StepStatus::Passed);
    assert_eq!(r.steps[1].status, StepStatus::Skipped);
}

// ---------- host 错误传播（fail fast） ----------

#[test]
fn host_send_error_fails_scenario_and_skips_rest() {
    let s = validated(&[], vec![send("a", false), send("b", false)]);
    let mut h = FakeHost::new().fail_send(HostError::Transport("port gone".into()));
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert!(h.sends.is_empty());
    let err = r.steps[0].error.as_ref().expect("error");
    assert_eq!(err.code, "host_transport");
    assert_eq!(err.message, "port gone");
    assert_eq!(r.steps[1].status, StepStatus::Skipped);
}

#[test]
fn host_signal_backpressure_error() {
    let s = validated(&[], vec![signal(PinDef::Dtr, true)]);
    let mut h = FakeHost::new().fail_signal(HostError::Backpressure);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    assert_eq!(
        r.steps[0].error.as_ref().map(|e| e.code.as_str()),
        Some("host_backpressure")
    );
}

#[test]
fn host_lines_error_during_wait_fails_scenario() {
    let s = validated(&[], vec![send("PING", true), wait(lit(None, "OK"), 100)]);
    let mut h = FakeHost::new().fail_lines(HostError::Transport("ring gone".into()));
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let err = r.steps[1].error.as_ref().expect("error");
    assert_eq!(err.code, "host_transport");
    assert_eq!(err.message, "ring gone");
}

#[test]
fn host_cancelled_from_sleep_marks_run_cancelled() {
    // host 主动返回 Cancelled（标志位未置位）也归为 cancelled 而非 failed
    let s = validated(&[], vec![delay(100), send("b", false)]);
    let mut h = FakeHost::new().fail_sleep(HostError::Cancelled);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Cancelled);
    assert_eq!(r.steps[0].status, StepStatus::Skipped);
    assert_eq!(r.steps[1].status, StepStatus::Skipped);
}

// ---------- 失败步的 skipped 补齐（含 Repeat 剩余迭代） ----------

#[test]
fn failure_inside_repeat_marks_remaining_iterations_skipped() {
    let s = validated(
        &[],
        vec![
            send("go", false),
            repeat(2, vec![send("a", false), signal(PinDef::Rts, false)]),
            send("tail", false),
        ],
    );
    let mut h = FakeHost::new().fail_signal(HostError::Backpressure);
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let paths: Vec<&str> = r.steps.iter().map(|st| st.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "steps[0]",            // passed
            "steps[1].steps[0]#0", // a (passed)
            "steps[1].steps[1]#0", // signal failed
            "steps[1].steps[0]#1", // 迭代 2 的 a：skipped
            "steps[1].steps[1]#1", // 迭代 2 的 signal：skipped
            "steps[2]",            // tail：skipped
        ]
    );
    assert_eq!(r.steps[0].status, StepStatus::Passed);
    assert_eq!(r.steps[1].status, StepStatus::Passed);
    assert_eq!(r.steps[2].status, StepStatus::Failed);
    for st in &r.steps[3..] {
        assert_eq!(st.status, StepStatus::Skipped);
    }
}

// ---------- 报告确定性（JSON） ----------

#[test]
fn report_json_is_deterministic_across_identical_runs() {
    let s = validated(
        &[("cmd", "PING"), ("token", "7f")],
        vec![
            signal(PinDef::Dtr, true),
            send("${cmd}", true),
            wait_save(
                re(Some(Dir::Rx), r"^PONG ([0-9A-Fa-f]{2})$"),
                500,
                "value",
                1,
            ),
            assert_last(hexm(None, "50 4f 4e 47"), 10, "got ${value}"),
            delay(10),
            repeat(
                2,
                vec![
                    send_hex("aa ${token}", false),
                    wait(lit(None, "PONG"), 500),
                    signal(PinDef::Rts, false),
                ],
            ),
        ],
    );
    let make_host = || {
        FakeHost::new()
            .reply(Dir::Rx, "PONG 4f")
            .schedule(1_000_005, Dir::Rx, "PONG 4f")
    };
    let mut h1 = make_host();
    let r1 = run(&mut h1, &s);
    let mut h2 = make_host();
    let r2 = run(&mut h2, &s);
    assert_eq!(r1.status, ScenarioStatus::Passed, "{:?}", r1.steps);
    assert_eq!(r2.status, ScenarioStatus::Passed);
    let j1 = report_json(&r1).expect("json");
    let j2 = report_json(&r2).expect("json");
    assert_eq!(j1, j2, "同 host 序列两次运行的 JSON 必须逐字节相同");
    assert_eq!(r1.variables.get("value").map(String::as_str), Some("4f"));
    // 嵌套结构抽检：repeat 内 send hex 展开两次
    let hex_sends: Vec<&str> = h1
        .sends
        .iter()
        .filter(|(m, _)| *m == SendModeDef::Hex)
        .map(|(_, t)| t.as_str())
        .collect();
    assert_eq!(hex_sends, vec!["aa 7f", "aa 7f"]);
}
