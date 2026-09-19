//! 跨特性集成测试（Stage 3 Task 8 Step 2/3）：规范输入对——仓库根
//! `tests/fixtures/replay-scenario.log` + `testdata/scenarios/replay-validation.json`
//! ——在 core 侧跑出等效报告，并端到端验证回放管线（ingest → ring → 告警 →
//! 只读场景 wait/assert）。
//!
//! # 三端等效（Step 2）
//! 同一对 fixture 在 core（本文件 `DialogueHost` 镜像对话）、桌面
//! （`src-tauri/tests/cross_feature.rs`，真实 loopback TCP 服务器按同一脚本
//! 应答）、CLI（`crates/bytetide-cli/tests/cross_feature.rs`，同款 TCP 服务器
//! 加 `bytetide run --report -`）三处执行，各自内嵌同一份期望常量（标注
//! 「三端同源」），对报告做时间戳归一化后比对。
//!
//! # 对话时序（三端镜像，matched_no 恒定）
//! 基线=0（首批行未出）→ 批 1：fixture 数据行 1..=3（wait[0] 命中 no=1）→
//! send "CAP?" 回显 no=4 → 批 2：VALUE 行 no=5（wait[2] 命中 + 捕获）→
//! send "SET 1234" 回显 no=6 → 批 3：ACK 行 no=7（wait[4] 命中）→ 尾批
//! （Wait 批消费语义要求尾段晚于 ACK 单独成批）：no=8..=13（wait[5] 命中
//! ALL CHECKS DONE no=11）→ assert 回看最近 10 行命中 no=7。桌面/CLI 服务器
//! 以「ACK 后 250ms 再发尾段」实现分批；core host 以「ACK 出现后的下一次
//! lines_after 才放尾段」等效镜像——批结构相同，matched_no 三端一致。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytetide_core::automation::{
    report_json, run_scenario, validate_scenario, HostError, Scenario, ScenarioHost, SendModeDef,
    ValidatedScenario,
};
use bytetide_core::replay::{ReplayCmd, ReplayConfig, ReplayState};
use bytetide_core::serial::manager::{BridgeLine, Pin, SendMode, SendRequest};
use bytetide_core::serial::port::Dir;
use bytetide_core::serial::ring::RING_CAP;
use bytetide_core::serial::rules::{AlertCfg, AlertRuleCfg, AutoReplyCfg, CaptureCfg};
use bytetide_core::serial::PortManager;
use bytetide_core::sink::VecSink;

// ============ 共享装载与期望（三端同源；改 fixture/scenario 必须三处同步） ============

/// 仓库根（crates/bytetide-core 上溯两级）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("manifest ancestors")
        .to_path_buf()
}

fn fixture_path() -> PathBuf {
    repo_root().join("tests/fixtures/replay-scenario.log")
}

fn scenario_path() -> PathBuf {
    repo_root().join("testdata/scenarios/replay-validation.json")
}

/// fixture 数据行（`#`/空行跳过；只按前两个 tab 切）。
struct FixtureLine {
    dir: Dir,
    text: String,
}

fn load_fixture_lines() -> Vec<FixtureLine> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read replay-scenario.log");
    let lines: Vec<FixtureLine> = raw
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut it = l.splitn(3, '\t');
            let ts = it.next().expect("ts column");
            assert!(!ts.is_empty(), "ts 列非空: {l:?}");
            let dir_s = it.next().expect("dir column");
            let text = it.next().expect("text column");
            let dir = if dir_s.trim().eq_ignore_ascii_case("tx") {
                Dir::Tx
            } else {
                Dir::Rx
            };
            FixtureLine {
                dir,
                text: text.to_string(),
            }
        })
        .collect();
    // fixture 头注声明 12 数据行；漂移先在这里红（三端常量以 12 行为准）
    assert_eq!(lines.len(), 12, "fixture 数据行数漂移，须同步三端期望常量");
    lines
}

fn canonical_scenario() -> Scenario {
    let raw = std::fs::read_to_string(scenario_path()).expect("read replay-validation.json");
    serde_json::from_str(&raw).expect("canonical scenario deserializes")
}

/// 报告时间戳归一化（三端同源）：剥 started/finished/duration 与每步
/// started/duration——三端时钟源不同（core 假钟 / 桌面·CLI 墙钟），其余字段
/// 必须逐字节一致。
fn normalized_report(v: &serde_json::Value) -> serde_json::Value {
    let mut obj = v.as_object().expect("report object").clone();
    for k in ["started_epoch_ms", "finished_epoch_ms", "duration_ms"] {
        obj.remove(k);
    }
    let steps = obj
        .get_mut("steps")
        .and_then(|s| s.as_array_mut())
        .expect("steps");
    for st in steps {
        if let Some(s) = st.as_object_mut() {
            s.remove("started_epoch_ms");
            s.remove("duration_ms");
        }
    }
    serde_json::Value::Object(obj)
}

/// 三端同源期望（live 对话模式；matched_no 语义见模块头注对话时序）。
fn expected_live_report() -> serde_json::Value {
    let step = |path: &str, kind: &str, matched_no: Option<u64>| {
        serde_json::json!({
            "path": path, "kind": kind, "matched_no": matched_no,
            "error": null, "status": "passed"
        })
    };
    serde_json::json!({
        "name": "replay-validation",
        "status": "passed",
        "variables": { "value": "1234" },
        "steps": [
            step("steps[0]", "wait", Some(1)),
            step("steps[1]", "send", None),
            step("steps[2]", "wait", Some(5)),
            step("steps[3]", "send", None),
            step("steps[4]", "wait", Some(7)),
            step("steps[5]", "wait", Some(11)),
            step("steps[6]", "assert", Some(7)),
        ]
    })
}

/// 三端同源期望（回放只读场景，core/桌面两处内嵌）：回放 ring 按文件序
/// 1..=12（TX 历史行占 no=5）。步序针对 Wait 批消费语义设计：gap 行（9）与
/// 尾段行以 >2×轮询分片（25ms）的原始间隔落盘、各自成批（回放 speed=1、
/// max_gap_ms=1000）；尾段内 10/11/12 允许合批——wait 只等末行 12，ALL CHECKS
/// DONE 由 assert 回看命中（批内消费不丢 assert 回看）。
fn expected_replay_readonly_report() -> serde_json::Value {
    let step = |path: &str, kind: &str, matched_no: u64| {
        serde_json::json!({
            "path": path, "kind": kind, "matched_no": matched_no,
            "error": null, "status": "passed"
        })
    };
    serde_json::json!({
        "name": "replay-readonly-validation",
        "status": "passed",
        "variables": {},
        "steps": [
            step("steps[0]", "wait", 9),
            step("steps[1]", "wait", 12),
            step("steps[2]", "assert", 10),
        ]
    })
}

/// 回放只读场景（无 send/signal——回放会话唯一可跑的场景形态；core/桌面两处
/// 内嵌同源 JSON）。
fn replay_readonly_scenario() -> &'static str {
    r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "replay-readonly-validation",
  "variables": {},
  "steps": [
    { "kind": "wait", "matcher": { "dir": "rx", "literal": "RESUME AFTER IDLE GAP" }, "timeoutMs": 20000 },
    { "kind": "wait", "matcher": { "dir": "rx", "literal": "tail padding line B" }, "timeoutMs": 10000 },
    { "kind": "assert", "matcher": { "dir": "rx", "literal": "ALL CHECKS DONE" }, "withinLast": 10, "message": "checkpoint seen" }
  ]
}"#
}

// ============ Step 2：三端等效报告（core 侧） ============

/// 对话时序的 core 侧镜像（桌面/CLI 用真实 TCP 服务器按同一脚本应答）。
/// `now_ms` 走 +1ms 假钟：报告时间戳确定性（归一化后无差异，但 core 报告可整体
/// 与期望 JSON 严格对齐）。
struct DialogueHost {
    lines: Vec<FixtureLine>,
    ring: VecDeque<BridgeLine>,
    seq: u64,
    sends: usize,
    head_done: bool,
    value_done: bool,
    ack_done: bool,
    tail_done: bool,
    now: u64,
    sent: Vec<String>,
}

impl DialogueHost {
    fn new(lines: Vec<FixtureLine>) -> Self {
        Self {
            lines,
            ring: VecDeque::new(),
            seq: 0,
            sends: 0,
            head_done: false,
            value_done: false,
            ack_done: false,
            tail_done: false,
            now: 1_000_000,
            sent: Vec::new(),
        }
    }

    fn push_line(&mut self, dir: Dir, text: &str) {
        self.seq += 1;
        let no = self.seq;
        self.ring.push_back(BridgeLine {
            no,
            ts: format!("fake-{no:04}"),
            dir,
            text: text.to_string(),
            bytes: None,
            epoch_millis: self.seq,
            r#match: None,
        });
    }

    /// 每次拉取前推进对话（一次只放一批，批结构与桌面/CLI 服务器分批一致）。
    fn reveal(&mut self) {
        if !self.head_done {
            for i in 0..3 {
                let text = self.lines[i].text.clone();
                self.push_line(Dir::Rx, &text);
            }
            self.head_done = true;
            return;
        }
        if self.sends >= 1 && !self.value_done {
            let text = self.lines[3].text.clone();
            self.push_line(Dir::Rx, &text);
            self.value_done = true;
            return;
        }
        if self.sends >= 2 && !self.ack_done {
            let text = self.lines[5].text.clone();
            self.push_line(Dir::Rx, &text);
            self.ack_done = true;
            return;
        }
        if self.ack_done && !self.tail_done {
            // ACK 已被观察（上一批返回过）后的下一批才放尾段——镜像服务器的 250ms
            for i in 6..self.lines.len() {
                let text = self.lines[i].text.clone();
                self.push_line(Dir::Rx, &text);
            }
            self.tail_done = true;
        }
    }
}

impl ScenarioHost for DialogueHost {
    fn send(&mut self, _mode: SendModeDef, text: &str) -> Result<(), HostError> {
        self.sends += 1;
        let echo = text.trim_end_matches('\n').to_string();
        self.sent.push(echo.clone());
        self.push_line(Dir::Tx, &echo);
        Ok(())
    }

    fn signal(
        &mut self,
        _pin: bytetide_core::automation::PinDef,
        _level: bool,
    ) -> Result<(), HostError> {
        Err(HostError::Transport("对话镜像无信号线".into()))
    }

    fn last_no(&self) -> u64 {
        self.ring.back().map_or(0, |l| l.no)
    }

    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.reveal();
        let from = self.ring.partition_point(|l| l.no <= since);
        Ok(self.ring.iter().skip(from).take(max).cloned().collect())
    }

    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        // 假钟瞬进：批结构已确定性，无需真实等待
        self.now += duration.as_millis() as u64;
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.now
    }
}

#[test]
fn canonical_pair_report_matches_shared_expectations_in_core() {
    let lines = load_fixture_lines();
    let scenario: ValidatedScenario = validate_scenario(canonical_scenario()).expect("validate");
    let mut host = DialogueHost::new(lines);
    let cancel = AtomicBool::new(false);
    let report = run_scenario(&scenario, &mut host, &cancel);

    assert_eq!(
        report.status,
        bytetide_core::automation::ScenarioStatus::Passed
    );
    // send 步收到的实参：变量替换 + 换行追加（捕获→引用链路在 core 侧的直接证据）
    assert_eq!(host.sent, vec!["CAP?".to_string(), "SET 1234".to_string()]);
    // 类型化断言（路径序 / 命中行号 / 捕获变量）
    let kinds: Vec<&'static str> = report.steps.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        ["wait", "send", "wait", "send", "wait", "wait", "assert"]
    );
    let paths: Vec<&str> = report.steps.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["steps[0]", "steps[1]", "steps[2]", "steps[3]", "steps[4]", "steps[5]", "steps[6]"]
    );
    let matched: Vec<Option<u64>> = report.steps.iter().map(|s| s.matched_no).collect();
    assert_eq!(
        matched,
        [Some(1), None, Some(5), None, Some(7), Some(11), Some(7)]
    );
    assert_eq!(
        report.variables.get("value").map(String::as_str),
        Some("1234")
    );

    // JSON 报告与三端同源期望归一化后逐字段一致
    let json: serde_json::Value =
        serde_json::from_str(&report_json(&report).expect("json")).unwrap();
    assert_eq!(normalized_report(&json), expected_live_report());
    // 假钟下报告确定性：同输入重跑逐字节相同
    let mut host2 = DialogueHost::new(load_fixture_lines());
    let report2 = run_scenario(&scenario, &mut host2, &cancel);
    assert_eq!(
        report_json(&report).unwrap(),
        report_json(&report2).unwrap()
    );
}

// ============ Step 3：回放管线（ingest → ring → 告警 → 守卫 → 只读场景） ============

/// 轮询直到条件成立（回放线程异步推进）。
fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "wait_until 超时");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn replay_pipeline_ingests_fixture_raises_alerts_and_guards() {
    let m = PortManager::new();
    let sink = Arc::new(VecSink::default());
    let (id, tx, index) = m
        .start_replay_indexed(
            &fixture_path(),
            ReplayConfig {
                speed: 10.0,
                ..ReplayConfig::default()
            },
            sink.clone(),
            PathBuf::new(),
        )
        .expect("start replay");
    assert_eq!(index.line_count, 12);
    assert_eq!(m.session_mode(&id), Some("replay"));

    // 暂停后灌告警规则再恢复：规则在行 7（告警样本）ingest 前就位。speed 10 下
    // 行 1..=8 在 ~35ms 内播完、行 7 前的累计仅 ~25ms——Pause 在起跑后数 ms 内
    // 落地，水位必 ≤8；若 CI 极端迟滞导致 >8 则这里先红（可见失败，不静默）
    tx.send(ReplayCmd::Pause).unwrap();
    wait_until(
        2_000,
        || matches!(m.replay_view(&id), Some((ReplayState::Paused, w)) if w <= 8),
    );
    m.set_live_rules(
        &id,
        AutoReplyCfg::default(),
        AlertCfg {
            enabled: true,
            rules: vec![AlertRuleCfg {
                id: "overheat".into(),
                pattern: "WARN temperature".into(),
                use_regex: false,
                case_sensitive: false,
                whole_word: false,
                min_count: 1,
                window_sec: 0,
                cooldown_sec: 0,
                level: "warn".into(),
                enabled: true,
            }],
        },
        CaptureCfg::default(),
    )
    .expect("set rules on replay session");
    tx.send(ReplayCmd::Resume).unwrap();

    // 全量 ingest：ring 行序 = fixture 数据行序（no 1..=12、dir/ts/epoch 保原值）
    let lines = load_fixture_lines();
    wait_until(10_000, || m.bridge_last_no(&id) == Some(12));
    let ring = m.ring_lines_after_no(&id, 0, 100).unwrap();
    assert_eq!(
        ring.iter().map(|l| l.no).collect::<Vec<_>>(),
        (1..=12).collect::<Vec<_>>()
    );
    for (l, f) in ring.iter().zip(&lines) {
        assert_eq!(l.text, f.text, "行序保持原文件序");
        assert_eq!(l.dir, f.dir);
    }
    assert_eq!(
        ring[8].ts, "00:00:25.000",
        ">10s gap 后行 ts 保原值（钳制只作用于睡眠）"
    );
    // 告警经 common ingest 评估并稀疏上报（fixture 行 7 = WARN temperature）
    wait_until(2_000, || {
        sink.0
            .lock()
            .iter()
            .any(|e| e == &format!("alert-hit {id} n=1"))
    });

    // 禁用操作稳定报错（T3/T6 守卫语义，报告/捕获对回放会话同样拒绝）
    let err = |e: anyhow::Error| e.to_string();
    assert_eq!(
        err(m
            .send(
                &id,
                SendRequest {
                    mode: SendMode::Ascii,
                    text: "x".into()
                }
            )
            .unwrap_err()),
        "replay_no_send|"
    );
    assert_eq!(
        err(m.set_signal(&id, Pin::Dtr, true).unwrap_err()),
        "replay_no_signal|"
    );
    assert_eq!(
        err(m.set_recording(&id, true).unwrap_err()),
        "replay_no_recording|"
    );
    assert_eq!(
        err(m.set_recording(&id, false).unwrap_err()),
        "replay_no_recording|"
    );
    assert_eq!(err(m.rotate_log(&id).unwrap_err()), "replay_no_recording|");

    // ring 上限字段可达（soak 断言的同源口径）
    assert_eq!(m.ring_bounds(&id).unwrap().ring_cap, RING_CAP);

    m.disconnect(&id).expect("disconnect");
    assert_eq!(m.replay_view(&id), None, "断开后会话移除");
}

/// manager 会话的 ScenarioHost 薄适配（镜像桌面 `ManagerScenarioHost` 的查询面：
/// send/signal 走 manager（回放必被拒）、行查询走 ring 游标、真睡眠 + 墙钟）。
struct ManagerHost<'a> {
    m: &'a PortManager,
    id: String,
}

impl ScenarioHost for ManagerHost<'_> {
    fn send(&mut self, _mode: SendModeDef, _text: &str) -> Result<(), HostError> {
        Err(HostError::Transport("回放会话不支持发送".into()))
    }
    fn signal(
        &mut self,
        _pin: bytetide_core::automation::PinDef,
        _level: bool,
    ) -> Result<(), HostError> {
        Err(HostError::Transport("回放会话不支持信号线".into()))
    }
    fn last_no(&self) -> u64 {
        self.m.bridge_last_no(&self.id).unwrap_or(0)
    }
    fn lines_after(&mut self, since: u64, max: usize) -> Result<Vec<BridgeLine>, HostError> {
        self.m
            .ring_lines_after_no(&self.id, since, max)
            .map_err(|e| HostError::Transport(e.to_string()))
    }
    fn sleep(&mut self, duration: Duration, cancel: &AtomicBool) -> Result<(), HostError> {
        let mut left = duration;
        while !left.is_zero() {
            if cancel.load(Ordering::Relaxed) {
                return Err(HostError::Cancelled);
            }
            let d = left.min(Duration::from_millis(25));
            std::thread::sleep(d);
            left -= d;
        }
        Ok(())
    }
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

#[test]
fn readonly_scenario_waits_and_asserts_hit_replay_produced_lines() {
    let m = PortManager::new();
    let sink = Arc::new(VecSink::default());
    // speed 1 + max_gap_ms=1000：行 9 在基线后 ~1s 落盘（wait 超时 20s 富余），
    // 尾段 50ms 原始间隔 = 2×轮询分片 → gap 行 9 与尾段行 10 确定性各自成批
    // （批消费语义下的 matched_no 恒定，见 expected_replay_readonly_report 注）
    let (id, _tx, _index) = m
        .start_replay_indexed(
            &fixture_path(),
            ReplayConfig {
                speed: 1.0,
                max_gap_ms: 1_000,
                ..ReplayConfig::default()
            },
            sink,
            PathBuf::new(),
        )
        .expect("start replay");
    wait_until(10_000, || m.replay_view(&id).is_some_and(|(_, w)| w >= 8));

    let scenario: ValidatedScenario =
        validate_scenario(serde_json::from_str(replay_readonly_scenario()).expect("json"))
            .expect("validate");
    let mut host = ManagerHost {
        m: &m,
        id: id.clone(),
    };
    let cancel = AtomicBool::new(false);
    let report = run_scenario(&scenario, &mut host, &cancel);

    if report.status != bytetide_core::automation::ScenarioStatus::Passed {
        panic!(
            "readonly scenario failed: {report:?}\nlast_no={:?}",
            m.bridge_last_no(&id)
        );
    }
    let json: serde_json::Value =
        serde_json::from_str(&report_json(&report).expect("json")).unwrap();
    assert_eq!(normalized_report(&json), expected_replay_readonly_report());

    m.disconnect(&id).expect("disconnect");
}
