//! 显式步骤回调（on_step_started）单测：SpyHost 记录步边界，断言精确
//! path/kind/current/total（复审 P2-1 修复的回归面）。

use super::*;

// ---------- 显式步骤回调 on_step_started ----------

/// 包裹 FakeHost 记录 on_step_started 调用的 spy。
struct SpyHost {
    inner: FakeHost,
    steps: Vec<(String, &'static str, u64, u64)>,
}

impl ScenarioHost for SpyHost {
    fn send(&mut self, mode: SendModeDef, text: &str) -> Result<(), HostError> {
        self.inner.send(mode, text)
    }
    fn signal(&mut self, pin: PinDef, level: bool) -> Result<(), HostError> {
        self.inner.signal(pin, level)
    }
    fn last_no(&self) -> u64 {
        self.inner.last_no()
    }
    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.inner.lines_after(since, max)
    }
    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        self.inner.sleep(duration, cancel)
    }
    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }
    fn on_step_started(&mut self, path: &str, kind: &'static str, current: u64, total: u64) {
        self.steps.push((path.to_string(), kind, current, total));
    }
}

#[test]
fn on_step_started_reports_precise_boundary_kind_and_totals() {
    let s = validated(
        &[],
        vec![
            send("PING", true),
            wait(rx_lit("OK"), 100),
            wait(rx_lit("OK"), 100),
        ],
    );
    let mut spy = SpyHost {
        inner: FakeHost::new()
            .reply(Dir::Rx, "OK")
            .schedule(1_000_005, Dir::Rx, "OK"),
        steps: Vec::new(),
    };
    let cancel = Arc::clone(&spy.inner.cancel);
    let r = run_scenario(&s, &mut spy, &cancel);
    assert_eq!(r.status, ScenarioStatus::Passed);
    // 连续两个 Wait 各自精确上报（启发式漏报场景——第一个 Wait 的轮询不再吞掉第二个）
    assert_eq!(
        spy.steps,
        vec![
            ("steps[0]".to_string(), "send", 1, 3),
            ("steps[1]".to_string(), "wait", 2, 3),
            ("steps[2]".to_string(), "wait", 3, 3),
        ]
    );
}

#[test]
fn on_step_started_covers_short_delay_and_repeat_iteration_paths() {
    // 短 Delay（≤25ms，启发式完全不 tick）与 Repeat 迭代后缀均精确上报
    let s = validated(
        &[],
        vec![delay(5), repeat(2, vec![delay(1), send("x", false)])],
    );
    let mut spy = SpyHost {
        inner: FakeHost::new(),
        steps: Vec::new(),
    };
    let cancel = Arc::clone(&spy.inner.cancel);
    let r = run_scenario(&s, &mut spy, &cancel);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(
        spy.steps,
        vec![
            ("steps[0]".to_string(), "delay", 1, 5),
            ("steps[1].steps[0]#0".to_string(), "delay", 2, 5),
            ("steps[1].steps[1]#0".to_string(), "send", 3, 5),
            ("steps[1].steps[0]#1".to_string(), "delay", 4, 5),
            ("steps[1].steps[1]#1".to_string(), "send", 5, 5),
        ]
    );
}

// ---------- assert ----------

#[test]
fn assert_searches_last_n_lines_newest_match_wins() {
    let mut h = FakeHost::new();
    h.push_line(Dir::Rx, "ping 7", None);
    h.push_line(Dir::Rx, "ping 8", None);
    h.push_line(Dir::Rx, "ping 9", None);
    // 窗口 = 最近 2 行 {8, 9}：命中 8
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
    // times=0 已被静态校验拒绝（下界 1，repeat_too_few）；手工构造验证 runner
    // 的防御路径：整体跳过、无叶子无报告
    let s = manual(vec![
        ValidatedStep::Repeat {
            times: 0,
            steps: vec![ValidatedStep::Send {
                mode: SendModeDef::Ascii,
                template: "x".into(),
                append_newline: false,
            }],
        },
        ValidatedStep::Send {
            mode: SendModeDef::Ascii,
            template: "y".into(),
            append_newline: false,
        },
    ]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Passed);
    assert_eq!(r.steps.len(), 1, "repeat 本身不是叶子，times=0 无报告");
    assert_eq!(r.steps[0].path, "steps[1]");
    assert_eq!(h.sends.len(), 1);
}

#[test]
fn runtime_undefined_variable_fails_scenario() {
    // 静态校验已拒绝未定义引用（直线与 Repeat 体一致）；手工构造验证运行期兜底
    let s = manual(vec![ValidatedStep::Send {
        mode: SendModeDef::Ascii,
        template: "${v}".into(),
        append_newline: false,
    }]);
    let mut h = FakeHost::new();
    let r = run(&mut h, &s);
    assert_eq!(r.status, ScenarioStatus::Failed);
    let err = r.steps[0].error.as_ref().expect("error");
    assert_eq!(err.code, "undefined_variable");
    assert_eq!(err.message, "undefined variable: ${v}");
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
