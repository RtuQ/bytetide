//! 场景命令层集成测试（Stage 3 Task 3）：真 PortManager + 本机 loopback TCP 回声
//! 会话，走命令层同一实现路径（start_scenario / AutomationRegistry /
//! cancel_and_disconnect——tauri 壳只差 State 提取与 AppHandle emit）。
//! 覆盖 plan 清单：校验失败、缺失/离线/回放会话拒绝、同会话二次运行拒绝
//! （不同会话可并行）、progress 事件顺序、stop 幂等、disconnect 取消、
//! 报告获取（json/junit）、完成后清理（50 上限逐出最旧 completed，running 永不逐出）。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytetide_core::automation::{validate_scenario, Scenario};
use bytetide_core::logfmt::LogConfig;
use bytetide_core::serial::port::PortConfig;
use bytetide_core::serial::PortManager;
use bytetide_core::sink::VecSink;
use serial_tool_lib::commands::{
    cancel_and_disconnect, start_scenario, validate_summary, AutomationRegistry, EmitFn,
};

// ===== 基础设施：临时目录 / 轮询 / 回声会话 / 场景构造 =====

/// 唯一临时目录（进程 id + 纳秒），测试结束 remove_dir_all。
fn temp_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "bytetide-it-automation-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    TempDir(dir)
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 轮询直到条件成立（运行线程异步推进；登记表只能经查询面观测）。
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

/// n 行 TSV 日志（回放会话拒绝测试用）。
fn write_log(dir: &Path, name: &str, n: u64) -> PathBuf {
    let path = dir.join(name);
    let mut w = std::io::BufWriter::new(std::fs::File::create(&path).expect("create log"));
    for i in 1..=n {
        writeln!(w, "00:00:{:02}.000\tRX\tline-{i}", i).expect("write line");
    }
    w.flush().expect("flush");
    path
}

/// 建一个已连接的 loopback TCP 会话 + 回声服务（收到含 PING 的行回 PONG，否则 ACK）。
fn start_echo_session(m: &Arc<PortManager>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
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
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = match stream.try_clone() {
            Ok(r) => BufReader::new(r),
            Err(_) => return,
        };
        let mut writer = stream;
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let reply = if line.contains("PING") {
                        "PONG\n"
                    } else {
                        "ACK\n"
                    };
                    if writer.write_all(reply.as_bytes()).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
            }
        }
    });
    id
}

/// 事件记录回调（进度/完成事件序断言用）。
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

/// 从 JSON 构造场景（贴近前端真实传参路径，走 serde 形状）。
fn scenario_from(v: serde_json::Value) -> Scenario {
    serde_json::from_value(v).expect("scenario json deserializes")
}

/// send PING（带换行）→ wait RX PONG。
fn ping_pong_scenario() -> Scenario {
    scenario_from(serde_json::json!({
        "schema": "bytetide.scenario", "version": 1, "name": "ping-pong",
        "steps": [
            {"kind": "send", "mode": "ascii", "text": "PING", "appendNewline": true},
            {"kind": "wait", "matcher": {"dir": "rx", "literal": "PONG"}, "timeoutMs": 5000}
        ]
    }))
}

/// 单步 delay 场景。
fn delay_scenario(name: &str, ms: u64) -> Scenario {
    scenario_from(serde_json::json!({
        "schema": "bytetide.scenario", "version": 1, "name": name,
        "steps": [{"kind": "delay", "ms": ms}]
    }))
}

// ===== 校验摘要 =====

#[test]
fn validate_summary_reports_step_count_and_stable_error() {
    // 合法：send+wait = 2 步
    let s = validate_summary(ping_pong_scenario());
    assert!(s.ok);
    assert_eq!(s.step_count, 2);
    assert_eq!(s.name, "ping-pong");
    assert!(s.error.is_none());

    // repeat 展开：repeat(3){delay,delay} + send = 7 执行步
    let s = validate_summary(scenario_from(serde_json::json!({
        "schema": "bytetide.scenario", "version": 1, "name": "rep",
        "steps": [
            {"kind": "send", "mode": "ascii", "text": "x", "appendNewline": false},
            {"kind": "repeat", "times": 3, "steps": [
                {"kind": "delay", "ms": 1},
                {"kind": "delay", "ms": 1}
            ]}
        ]
    })));
    assert!(s.ok);
    assert_eq!(s.step_count, 7);

    // 非法：正则编译失败 → 稳定 code + 路径
    let s = validate_summary(scenario_from(serde_json::json!({
        "schema": "bytetide.scenario", "version": 1, "name": "bad",
        "steps": [{"kind": "wait", "matcher": {"regex": "(unclosed"}, "timeoutMs": 10}]
    })));
    assert!(!s.ok);
    let err = s.error.expect("error info");
    assert_eq!(err.code, "invalid_regex");
    assert_eq!(err.path, "steps[0]");

    // 非法：schema 错 → invalid_schema
    let s = validate_summary(scenario_from(serde_json::json!({
        "schema": "bytetide.other", "version": 1, "name": "bad",
        "steps": [{"kind": "delay", "ms": 1}]
    })));
    assert!(!s.ok);
    assert_eq!(s.error.expect("error info").code, "invalid_schema");
}

// ===== start 守卫：校验失败 / 缺失 / 离线 / 回放 =====

#[test]
fn start_rejects_invalid_scenario_and_forbidden_sessions() {
    let m = Arc::new(PortManager::new());
    let registry = AutomationRegistry::default();

    // 校验失败：Err 透传稳定 code
    let bad = scenario_from(serde_json::json!({
        "schema": "bytetide.scenario", "version": 2, "name": "bad",
        "steps": [{"kind": "delay", "ms": 1}]
    }));
    let err = start_scenario(&registry, &m, "s1", bad, noop_emit()).unwrap_err();
    assert!(
        err.contains("场景校验失败") && err.contains("invalid_version"),
        "校验失败须带稳定 code: {err}"
    );

    // 会话不存在
    let err =
        start_scenario(&registry, &m, "s9999", ping_pong_scenario(), noop_emit()).unwrap_err();
    assert_eq!(err, "会话不存在");

    // 离线会话拒绝
    let off = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
    let err = start_scenario(&registry, &m, &off, ping_pong_scenario(), noop_emit()).unwrap_err();
    assert_eq!(err, "离线会话不支持场景");

    // 回放会话拒绝
    let dir = temp_dir("replay-reject");
    let path = write_log(dir.0.as_path(), "r.log", 3);
    let (rid, _tx) = m
        .start_replay(
            &path,
            bytetide_core::replay::ReplayConfig::default(),
            Arc::new(VecSink::default()),
            PathBuf::new(),
        )
        .expect("start replay");
    let err = start_scenario(&registry, &m, &rid, ping_pong_scenario(), noop_emit()).unwrap_err();
    assert_eq!(err, "回放会话不支持场景");

    // live 会话放行
    let live = start_echo_session(&m);
    assert!(start_scenario(&registry, &m, &live, delay_scenario("ok", 0), noop_emit()).is_ok());
    m.disconnect(&live).expect("disconnect");
}

// ===== 并发守卫：同会话一个运行，跨会话并行 =====

#[test]
fn same_session_second_run_rejected_but_other_session_parallel() {
    let m = Arc::new(PortManager::new());
    let registry = AutomationRegistry::default();
    let id_a = start_echo_session(&m);
    let id_b = start_echo_session(&m);
    let long = validate_scenario(delay_scenario("long", 60_000)).expect("validate");

    let run_a = registry
        .start(&id_a, long.clone(), m.clone(), noop_emit())
        .expect("start on A");
    // 同会话第二次 → 稳定错误
    let err = registry
        .start(&id_a, long.clone(), m.clone(), noop_emit())
        .unwrap_err();
    assert!(
        err.contains("同一会话同时只允许运行一个场景"),
        "并发守卫稳定文案: {err}"
    );
    // 不同会话可并行
    let run_b = registry
        .start(&id_b, long, m.clone(), noop_emit())
        .expect("start on B");
    assert_ne!(run_a, run_b);

    // 清理后同会话可再次启动
    registry.stop(&run_a);
    registry.stop(&run_b);
    wait_until(5_000, || {
        registry
            .view(&run_a)
            .map(|v| v.status != "running")
            .unwrap_or(false)
    });
    let again = registry
        .start(
            &id_a,
            validate_scenario(delay_scenario("again", 0)).unwrap(),
            m.clone(),
            noop_emit(),
        )
        .expect("restart after stop");
    wait_until(5_000, || {
        registry
            .view(&again)
            .map(|v| v.status == "passed")
            .unwrap_or(false)
    });

    m.disconnect(&id_a).expect("disconnect A");
    m.disconnect(&id_b).expect("disconnect B");
}

// ===== live 回声运行：progress 事件序 + 报告 =====

#[test]
fn live_echo_run_passes_with_progress_order_and_reports() {
    let m = Arc::new(PortManager::new());
    let id = start_echo_session(&m);
    let registry = AutomationRegistry::default();
    let events = EventLog::default();
    let validated = validate_scenario(ping_pong_scenario()).expect("validate");
    let run = registry
        .start(&id, validated, m.clone(), recording_emit(events.clone()))
        .expect("start");
    wait_until(5_000, || {
        registry
            .view(&run)
            .map(|v| v.status != "running")
            .unwrap_or(false)
    });

    // 终态与视图
    let v = registry.view(&run).expect("view");
    assert_eq!(v.status, "passed");
    assert_eq!(v.session_id, id);
    assert_eq!(v.run_id, run);
    assert_eq!(
        v.started_epoch_ms,
        registry.view(&run).unwrap().started_epoch_ms
    );
    assert!(v.duration_ms.unwrap_or(u64::MAX) < 5_000, "回声应快速完成");

    // 事件序：progress(send) → progress(wait) → finished（唯一一次，且无其他事件）
    let evs = events.0.lock().expect("event log").clone();
    let progress_kinds: Vec<&str> = evs
        .iter()
        .filter(|(e, _)| e == "scenario-progress")
        .map(|(_, p)| p["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(progress_kinds, vec!["send", "wait"], "事件序: {evs:?}");
    assert!(
        evs.iter()
            .all(|(e, _)| e == "scenario-progress" || e == "scenario-finished"),
        "只允许 progress/finished 事件: {evs:?}"
    );
    let (fin_event, fin_payload) = evs.last().expect("finished event");
    assert_eq!(fin_event, "scenario-finished");
    assert_eq!(fin_payload["runId"].as_str(), Some(run.as_str()));
    assert_eq!(fin_payload["sessionId"].as_str(), Some(id.as_str()));
    assert_eq!(fin_payload["status"].as_str(), Some("passed"));
    // progress payload：currentStep 递增、totalSteps=2
    let progresses: Vec<&serde_json::Value> = evs
        .iter()
        .filter(|(e, _)| e == "scenario-progress")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(progresses[0]["currentStep"], serde_json::json!(1));
    assert_eq!(progresses[1]["currentStep"], serde_json::json!(2));
    assert!(progresses
        .iter()
        .all(|p| p["totalSteps"] == serde_json::json!(2)
            && p["runId"].as_str() == Some(run.as_str())
            && p["sessionId"].as_str() == Some(id.as_str())));

    // 报告：JSON 解析、步数与命中行号；JUnit testsuite
    let json = registry.report(&run, "json").expect("json report");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("report json parses");
    assert_eq!(parsed["status"], serde_json::json!("passed"));
    assert_eq!(parsed["steps"].as_array().expect("steps").len(), 2);
    assert_eq!(parsed["steps"][0]["status"], serde_json::json!("passed"));
    assert_eq!(parsed["steps"][1]["status"], serde_json::json!("passed"));
    let matched = parsed["steps"][1]["matchedNo"]
        .as_u64()
        .or(parsed["steps"][1]["matched_no"].as_u64())
        .expect("wait matched_no");
    assert!(matched >= 1, "wait 命中行号: {parsed}");
    let junit = registry.report(&run, "junit").expect("junit report");
    assert!(junit.contains(r#"<testsuite name="ping-pong""#), "{junit}");
    assert_eq!(junit.matches("<testcase").count(), 2);
    assert!(!junit.contains("<failure"), "全过的场景无 failure: {junit}");

    m.disconnect(&id).expect("disconnect");
}

// ===== stop：取消 + 幂等 =====

#[test]
fn stop_cancels_running_scenario_and_is_idempotent() {
    let m = Arc::new(PortManager::new());
    let id = start_echo_session(&m);
    let registry = AutomationRegistry::default();
    let run = registry
        .start(
            &id,
            validate_scenario(delay_scenario("long", 60_000)).unwrap(),
            m.clone(),
            noop_emit(),
        )
        .expect("start");

    registry.stop(&run);
    assert_eq!(
        registry.view(&run).expect("view").status,
        "cancelled",
        "stop 后运行必须为 cancelled"
    );
    // 报告已生成且为 cancelled
    let json = registry.report(&run, "json").expect("report after stop");
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["status"], serde_json::json!("cancelled"));

    // 幂等：已完成 / 未知 runId 的 stop 不报错（no-op）
    registry.stop(&run);
    registry.stop("run9999");

    m.disconnect(&id).expect("disconnect");
}

// ===== disconnect 取消 =====

#[test]
fn disconnect_cancels_running_scenario() {
    let m = Arc::new(PortManager::new());
    let id = start_echo_session(&m);
    let registry = AutomationRegistry::default();
    let events = EventLog::default();
    let run = registry
        .start(
            &id,
            validate_scenario(delay_scenario("long", 60_000)).unwrap(),
            m.clone(),
            recording_emit(events.clone()),
        )
        .expect("start");

    // 命令层同一断开路径：先取消该会话运行（runner ≤25ms 分片内退出）再 disconnect
    cancel_and_disconnect(&registry, &m, &id).expect("cancel + disconnect");

    let v = registry.view(&run).expect("run 保留可查");
    assert_eq!(v.status, "cancelled", "断开须取消运行中的场景");
    // 完成事件恰一次
    let evs = events.0.lock().expect("event log").clone();
    assert_eq!(
        evs.iter().filter(|(e, _)| e == "scenario-finished").count(),
        1,
        "finished 恰一次: {evs:?}"
    );
    // 会话已移除
    assert!(!m.bridge_list().iter().any(|s| s.id == id));
}

// ===== 完成后清理：保留最近 50 份 completed，逐出最旧；running 永不逐出 =====

#[test]
fn completed_runs_evicted_after_fifty_while_running_survives() {
    let m = Arc::new(PortManager::new());
    let registry = AutomationRegistry::default();
    let id_a = start_echo_session(&m);
    let id_b = start_echo_session(&m);

    // B 会话的长跑运行（整个测试期间 running，不入完成队列）
    let long = registry
        .start(
            &id_b,
            validate_scenario(delay_scenario("long", 60_000)).unwrap(),
            m.clone(),
            noop_emit(),
        )
        .expect("start long");

    // A 会话串行跑 51 个即时场景（同会话守卫要求等上一个完成）
    let mut runs: Vec<String> = Vec::new();
    for i in 0..51 {
        let r = registry
            .start(
                &id_a,
                validate_scenario(delay_scenario("quick", 0)).unwrap(),
                m.clone(),
                noop_emit(),
            )
            .unwrap_or_else(|e| panic!("quick run {i} start: {e}"));
        wait_until(5_000, || {
            registry
                .view(&r)
                .map(|v| v.status == "passed")
                .unwrap_or(false)
        });
        runs.push(r);
    }

    // running 永不逐出
    assert!(registry.view(&long).is_ok(), "运行中条目不得被逐出");
    // 最旧 completed 被逐出，最近 50 份保留
    let err = registry.view(&runs[0]).unwrap_err();
    assert!(err.contains("场景运行不存在"), "最旧 completed 逐出: {err}");
    assert!(registry.view(&runs[1]).is_ok(), "最近 50 份保留");
    assert!(registry.view(runs.last().unwrap()).is_ok());

    // 报告逐出同步：被逐出 run 查不到报告
    assert!(registry.report(&runs[0], "json").is_err());

    registry.stop(&long);
    m.disconnect(&id_a).expect("disconnect A");
    m.disconnect(&id_b).expect("disconnect B");
}

// ===== 报告查询错误路径 =====

#[test]
fn report_errors_for_running_unknown_run_and_bad_format() {
    let m = Arc::new(PortManager::new());
    let id = start_echo_session(&m);
    let registry = AutomationRegistry::default();
    let run = registry
        .start(
            &id,
            validate_scenario(delay_scenario("long", 60_000)).unwrap(),
            m.clone(),
            noop_emit(),
        )
        .expect("start");

    // 未完成 → 报告未生成
    let err = registry.report(&run, "json").unwrap_err();
    assert!(err.contains("尚未完成"), "运行中报告错误: {err}");

    // 未知格式
    let err = registry.report(&run, "xml").unwrap_err();
    assert!(err.contains("未知报告格式"), "格式错误: {err}");

    // 未知 run
    let err = registry.report("run9999", "json").unwrap_err();
    assert!(err.contains("场景运行不存在"), "未知 run: {err}");
    assert!(registry
        .view("run9999")
        .unwrap_err()
        .contains("场景运行不存在"));

    registry.stop(&run);
    m.disconnect(&id).expect("disconnect");
}
