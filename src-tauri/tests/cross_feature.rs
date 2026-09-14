//! 跨特性集成测试（Stage 3 Task 8 Step 2/3，桌面命令层）：规范输入对——仓库根
//! `tests/fixtures/replay-scenario.log` + `testdata/scenarios/replay-validation.json`
//! ——经真 PortManager + loopback TCP 对话服务器跑出与 core/CLI 等效的报告；
//! 并覆盖回放只读场景守卫（Task 8 Step 3 放宽：scenario_start 允许无 send/signal
//! 的场景对回放会话运行，wait/assert 命中回放产生的行）。
//!
//! # 三端等效
//! 期望常量与 core（crates/bytetide-core/tests/cross_feature.rs）、CLI
//! （crates/bytetide-cli/tests/cross_feature.rs）各嵌一份（标注「三端同源」），
//! 报告归一化时间戳后逐字段比对。
//!
//! # 对话时序（与 core DialogueHost 镜像）
//! 服务器在「场景基线已取」的放行信号后发首批（fixture 数据行 1..=3，ring
//! no=1..=3）→ 收到 "CAP?" 回 VALUE 行（no=5，其间 send 回显占 no=4）→ 收到
//! "SET …" 回 ACK 行（no=7，回显 no=6）→ 250ms 后发尾段（no=8..=13；250ms ≫
//! 25ms 轮询分片，保证 ACK 与尾段不同批）→ 读到 EOF 退出。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytetide_core::automation::Scenario;
use bytetide_core::logfmt::LogConfig;
use bytetide_core::replay::ReplayConfig;
use bytetide_core::serial::port::PortConfig;
use bytetide_core::serial::PortManager;
use bytetide_core::sink::VecSink;
use serial_tool_lib::commands::{
    cancel_and_disconnect, start_scenario, AutomationRegistry, EmitFn,
};

// ============ 共享装载与期望（三端同源；改 fixture/scenario 必须三处同步） ============

/// 仓库根（src-tauri 上溯一级）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .expect("manifest ancestors")
        .to_path_buf()
}

fn fixture_path() -> PathBuf {
    repo_root().join("tests/fixtures/replay-scenario.log")
}

/// fixture 数据行文本（服务器分批发送的脚本内容；`#`/空行跳过）。
fn load_fixture_texts() -> Vec<String> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read replay-scenario.log");
    let lines: Vec<String> = raw
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            // 取 text 列（第三个 tab 之后；本 fixture 无含 tab 的 text）
            l.splitn(3, '\t').nth(2).expect("text column").to_string()
        })
        .collect();
    assert_eq!(lines.len(), 12, "fixture 数据行数漂移，须同步三端期望常量");
    lines
}

fn canonical_scenario() -> Scenario {
    let raw =
        std::fs::read_to_string(repo_root().join("testdata/scenarios/replay-validation.json"))
            .expect("read replay-validation.json");
    serde_json::from_str(&raw).expect("canonical scenario deserializes")
}

/// 回放只读场景（无 send/signal；core/桌面两处内嵌同源 JSON）。
fn replay_readonly_scenario_json() -> &'static str {
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

/// 报告时间戳归一化（三端同源）：剥 started/finished/duration 与每步
/// started/duration；其余字段逐字节比对。
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

/// 三端同源期望（live 对话模式；matched_no 见对话时序）。
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

/// 三端同源期望（回放只读场景，core/桌面两处内嵌；批消费语义见 core 同名注释）。
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

// ============ 基础设施 ============

/// 轮询直到条件成立。
fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while !f() {
        assert!(Instant::now() < deadline, "wait_until 超时");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// tcp-client 配置指向本机端口。
fn tcp_client_cfg(port: u16) -> PortConfig {
    PortConfig {
        name: format!("tcp-{port}"),
        transport: Some("tcp-client".into()),
        tcp_host: Some("127.0.0.1".into()),
        tcp_port: Some(port),
        ..PortConfig::default()
    }
}

fn write_line(sock: &mut std::net::TcpStream, text: &str) {
    sock.write_all(text.as_bytes()).expect("write line");
    sock.write_all(b"\n").expect("write newline");
    let _ = sock.flush();
}

/// loopback TCP 对话服务器：`gate` 收到放行信号（场景基线已取）后按对话脚本
/// 分批发送 fixture 行；读到 EOF 退出。脚本与 core `DialogueHost` 镜像（同源）。
fn spawn_dialogue_server(lines: Vec<String>, gate: Receiver<()>) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        gate.recv().expect("gate signal");
        // 批 1：fixture 数据行 1..=3（DEVICE READY / ASCII-Hex 帧 / lossy 行）
        for l in &lines[0..3] {
            write_line(&mut sock, l);
        }
        let mut reader = BufReader::new(sock.try_clone().expect("try_clone"));
        let mut line = String::new();
        let mut value_sent = false;
        let mut ack_sent = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if !value_sent && line.starts_with("CAP?") {
                // 批 2：VALUE 行（no=5；send 回显已占 no=4）
                write_line(&mut sock, &lines[3]);
                value_sent = true;
            } else if !ack_sent && line.starts_with("SET ") {
                // 批 3：ACK 行（no=7；回显 no=6）——单独成批
                write_line(&mut sock, &lines[5]);
                ack_sent = true;
                // 尾批延后 ≥10×轮询分片：保证 ACK 与尾段不同批（批消费语义）
                std::thread::sleep(Duration::from_millis(250));
                for l in &lines[6..] {
                    write_line(&mut sock, l);
                }
            }
        }
    });
    (port, handle)
}

/// 事件记录回调（scenario-progress 放行门 + finished 断言用）。
#[derive(Clone, Default)]
struct EventLog(Arc<Mutex<Vec<(String, serde_json::Value)>>>);

fn recording_emit(log: EventLog) -> EmitFn {
    Arc::new(move |event, payload| {
        log.0
            .lock()
            .expect("event log")
            .push((event.to_string(), payload));
    })
}

fn noop_emit() -> EmitFn {
    Arc::new(|_, _| ())
}

// ============ Step 2：live 三端等效（桌面命令层） ============

#[test]
fn canonical_pair_report_matches_shared_expectations_on_desktop() {
    let m = Arc::new(PortManager::new());
    let lines = load_fixture_texts();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let (port, server) = spawn_dialogue_server(lines, gate_rx);
    let id = m
        .connect(
            tcp_client_cfg(port),
            LogConfig::default(),
            Arc::new(VecSink::default()),
            PathBuf::new(),
        )
        .expect("connect");
    wait_until(5_000, || {
        m.bridge_list()
            .iter()
            .any(|s| s.id == id && s.status == "connected")
    });

    let registry = AutomationRegistry::default();
    let events = EventLog::default();
    let run = start_scenario(
        &registry,
        &m,
        &id,
        canonical_scenario(),
        recording_emit(events.clone()),
    )
    .expect("start scenario");
    // 放行门：首个 scenario-progress 事件意味着 runner 已取基线（baseline 先于
    // 首步执行）——首批行此后才入 ring，wait[0] 必然可见（零竞态）
    wait_until(5_000, || {
        events
            .0
            .lock()
            .expect("event log")
            .iter()
            .any(|(e, _)| e == "scenario-progress")
    });
    gate_tx.send(()).expect("gate");

    wait_until(30_000, || {
        registry
            .view(&run)
            .map(|v| v.status != "running")
            .unwrap_or(false)
    });
    let view = registry.view(&run).expect("view");
    assert_eq!(view.status, "passed", "run view: {view:?}");

    // 报告与三端同源期望归一化后逐字段一致
    let json = registry.report(&run, "json").expect("json report");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("report json parses");
    assert_eq!(normalized_report(&parsed), expected_live_report());
    // JUnit：7 用例全过、无 failure
    let junit = registry.report(&run, "junit").expect("junit report");
    assert!(
        junit.contains(r#"<testsuite name="replay-validation""#),
        "{junit}"
    );
    assert_eq!(junit.matches("<testcase").count(), 7);
    assert!(!junit.contains("<failure"), "{junit}");
    // 完成事件恰一次
    assert_eq!(
        events
            .0
            .lock()
            .expect("event log")
            .iter()
            .filter(|(e, _)| e == "scenario-finished")
            .count(),
        1
    );

    cancel_and_disconnect(&registry, &m, &id).expect("disconnect");
    server.join().expect("server thread");
}

// ============ Step 3：回放只读场景（T3 守卫放宽后的行为） ============

#[test]
fn readonly_scenario_runs_against_replay_session_and_hits_replayed_lines() {
    let m = Arc::new(PortManager::new());
    let registry = AutomationRegistry::default();
    // 与 core 同款时序：speed 1 + max_gap_ms=1000（gap 行 9 在基线后 ~1s、尾段
    // 50ms = 2×轮询分片各自成批），水位=8 时启动场景 → 基线恒为 8
    let (id, _tx, index) = m
        .start_replay_indexed(
            &fixture_path(),
            ReplayConfig {
                speed: 1.0,
                max_gap_ms: 1_000,
                ..ReplayConfig::default()
            },
            Arc::new(VecSink::default()),
            PathBuf::new(),
        )
        .expect("start replay");
    assert_eq!(index.line_count, 12);
    wait_until(10_000, || m.replay_view(&id).is_some_and(|(_, w)| w >= 8));

    let scenario: Scenario =
        serde_json::from_str(replay_readonly_scenario_json()).expect("readonly scenario");
    let run = start_scenario(&registry, &m, &id, scenario, noop_emit())
        .expect("readonly scenario must be allowed on replay session (Task 8 Step 3)");
    wait_until(30_000, || {
        registry
            .view(&run)
            .map(|v| v.status != "running")
            .unwrap_or(false)
    });
    let view = registry.view(&run).expect("view");
    assert_eq!(view.status, "passed", "run view: {view:?}");

    let json = registry.report(&run, "json").expect("json report");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("report json parses");
    assert_eq!(
        normalized_report(&parsed),
        expected_replay_readonly_report()
    );

    m.disconnect(&id).expect("disconnect");
}

#[test]
fn scenario_guards_replay_send_steps_and_offline_unchanged() {
    let m = Arc::new(PortManager::new());
    let registry = AutomationRegistry::default();

    // 回放会话 + 含 send 步的场景 → 稳定文案拒绝（Task 8 Step 3 的新语义）
    let dir = std::env::temp_dir().join(format!(
        "bytetide-xf-guard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let log = dir.join("g.log");
    std::fs::write(&log, "00:00:01.000\tRX\thello\n").expect("write log");
    let (rid, _tx, _index) = m
        .start_replay_indexed(
            &log,
            ReplayConfig::default(),
            Arc::new(VecSink::default()),
            PathBuf::new(),
        )
        .expect("start replay");
    let with_send: Scenario = serde_json::from_str(
        r#"{
        "schema": "bytetide.scenario", "version": 1, "name": "has-send",
        "steps": [
            {"kind": "send", "mode": "ascii", "text": "x", "appendNewline": true},
            {"kind": "wait", "matcher": {"literal": "y"}, "timeoutMs": 100}
        ]
    }"#,
    )
    .expect("scenario");
    let err = start_scenario(&registry, &m, &rid, with_send, noop_emit()).unwrap_err();
    assert_eq!(err, "回放会话不支持发送步骤");
    // 含 signal 步同样拒绝
    let with_signal: Scenario = serde_json::from_str(
        r#"{
        "schema": "bytetide.scenario", "version": 1, "name": "has-signal",
        "steps": [{"kind": "signal", "pin": "dtr", "level": true}]
    }"#,
    )
    .expect("scenario");
    let err = start_scenario(&registry, &m, &rid, with_signal, noop_emit()).unwrap_err();
    assert_eq!(err, "回放会话不支持发送步骤");
    m.disconnect(&rid).expect("disconnect replay");

    // 离线会话维持无条件拒绝（T3 语义不变）
    let off = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
    let readonly: Scenario =
        serde_json::from_str(replay_readonly_scenario_json()).expect("scenario");
    let err = start_scenario(&registry, &m, &off, readonly.clone(), noop_emit()).unwrap_err();
    assert_eq!(err, "离线会话不支持场景");
    // 会话不存在维持稳定文案
    let err = start_scenario(&registry, &m, "s9999", readonly, noop_emit()).unwrap_err();
    assert_eq!(err, "会话不存在");

    std::fs::remove_dir_all(&dir).ok();
}
