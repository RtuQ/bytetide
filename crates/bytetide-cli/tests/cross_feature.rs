//! 跨特性集成测试（Stage 3 Task 8 Step 2，CLI 侧）：规范输入对——仓库根
//! `tests/fixtures/replay-scenario.log` + `testdata/scenarios/replay-validation.json`
//! ——以子进程跑真实 `bytetide run`（loopback TCP 对话服务器按对话脚本应答），
//! 断言 JSON 报告（--report -）与 core/桌面等效（时间戳归一化后逐字段一致）。
//!
//! # 三端等效
//! 期望常量与 core（crates/bytetide-core/tests/cross_feature.rs 的 DialogueHost）、
//! 桌面（src-tauri/tests/cross_feature.rs）各嵌一份（标注「三端同源」）。对话
//! 服务器脚本与桌面同款（放行门=accept 后 300ms——CLI 进程 connect 后随即取
//! 基线，300ms ≫ 起跑延迟；失败模式为 wait 超时，可见不静默）。
//!
//! # 已知慢路径（预先存在，非本 fixture 引入）
//! CliHost 的读线程持 io 互斥做 200ms 读循环，send 只能在锁竞争间歇完成——
//! 真 TCP 路径的 send 普遍有秒级延迟（既有 cli_scenario::tcp_loopback 同象，
//! ~12s/步）。正确性不受影响（wait 超时只约束本步起点之后；send 延迟只推移
//! 进度），本用例整体 ~1 分钟内稳定通过。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

// ============ 共享装载与期望（三端同源；改 fixture/scenario 必须三处同步） ============

/// 仓库根（crates/bytetide-cli 上溯两级）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("manifest ancestors")
        .to_path_buf()
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

/// 三端同源期望（live 对话模式；matched_no 语义见 core 同名测试注释）。
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

// ============ 基础设施 ============

/// loopback TCP 对话服务器（与桌面 cross_feature.rs 同源脚本）：accept 后 300ms
/// 发首批（基线已在 connect 后即刻取定，300ms ≫ 起跑延迟），按收到的探测行
/// 分批应答，读 EOF 退出。300ms 放行门若被极端 CI 迟滞击穿，失败模式为
/// wait 超时（exit 3），可见不静默。
fn spawn_dialogue_server(lines: Vec<String>) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        std::thread::sleep(Duration::from_millis(300));
        let send = |sock: &mut std::net::TcpStream, text: &str| {
            sock.write_all(text.as_bytes()).expect("write line");
            sock.write_all(b"\n").expect("write newline");
            let _ = sock.flush();
        };
        for l in &lines[0..3] {
            send(&mut sock, l);
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
                send(&mut sock, &lines[3]);
                value_sent = true;
            } else if !ack_sent && line.starts_with("SET ") {
                send(&mut sock, &lines[5]);
                ack_sent = true;
                // 尾批延后 ≥10×Wait 轮询分片：保证 ACK 与尾段不同批
                std::thread::sleep(Duration::from_millis(250));
                for l in &lines[6..] {
                    send(&mut sock, l);
                }
            }
        }
    });
    (port, handle)
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bytetide"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn bytetide binary")
}

// ============ 规范输入对 × 真实 run 子进程 ============

#[test]
fn canonical_pair_report_matches_shared_expectations_via_cli() {
    let raw = std::fs::read_to_string(repo_root().join("tests/fixtures/replay-scenario.log"))
        .expect("read replay-scenario.log");
    let lines: Vec<String> = raw
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.splitn(3, '\t').nth(2).expect("text column").to_string())
        .collect();
    assert_eq!(lines.len(), 12, "fixture 数据行数漂移，须同步三端期望常量");

    let (port, server) = spawn_dialogue_server(lines);
    let scenario = repo_root().join("testdata/scenarios/replay-validation.json");
    let out = run_cli(&[
        "run",
        "--scenario",
        scenario.to_str().unwrap(),
        "--tcp",
        &format!("127.0.0.1:{port}"),
        "--report",
        "-",
    ]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // stdout = 报告（--report -）；进度只在 stderr
    let stdout = String::from_utf8(out.stdout).expect("stdout utf8");
    let stderr = String::from_utf8(out.stderr).expect("stderr utf8");
    assert!(!stderr.contains("FAILED"), "stderr: {stderr}");
    assert!(stderr.contains("replay-validation"), "stderr 摘要: {stderr}");
    assert!(stderr.contains("[7/7] assert"), "stderr 进度: {stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is report json");
    assert_eq!(
        normalized_report(&parsed),
        expected_live_report(),
        "CLI 报告须与 core/桌面等效: {parsed}"
    );

    server.join().expect("server thread");
}
