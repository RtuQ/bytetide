//! run 子命令进程级集成测试（Task 5 Step 2）：以子进程方式跑真实二进制
//! （`CARGO_BIN_EXE_bytetide` 标准做法，musl/跨平台可用），断言退出码、stderr
//! 进度、stdout 纪律与报告文件内容。
//!
//! 硬件不可测，两条确定性路径：
//! 1. **fake host 后门**：`BYTETIDE_SCENARIO_FAKE_HOST=<JSON>` 使 run 走内存
//!    FakeHost（见 scenario.rs 头注——仅测试用，不暴露在 help），取消用内置
//!    cancelAfterCalls 开关模拟（避免发真实 SIGINT 的平台差异）；
//! 2. **loopback TCP**：测试内起临时 TCP server 注入响应行，覆盖真实 transport
//!    的 pass 路径（退出码 0 + 报告 passed）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;

/// scenario.rs 头注声明的测试后门环境变量（保持字面量同步）。
const FAKE_ENV: &str = "BYTETIDE_SCENARIO_FAKE_HOST";

/// send→wait→assert 三步全通过场景（与 fake host / TCP server 均配套）。
const PASS_SCENARIO: &str = r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "cli-pass",
  "steps": [
    { "kind": "send", "mode": "ascii", "text": "PING", "appendNewline": true },
    { "kind": "wait", "matcher": { "literal": "PONG" }, "timeoutMs": 3000 },
    { "kind": "assert", "matcher": { "literal": "PONG" }, "withinLast": 10, "message": "pong expected" }
  ]
}"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("bytetide-cli-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("write fixture");
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run_cli(args: &[&str], env: Option<(&str, &str)>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bytetide"));
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((k, v)) = env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn bytetide binary")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 临时 TCP server：收满一行 PING 后回一行 PONG，随后等对端 EOF（CLI 进程退出
/// 自然关连接，不做固定睡眠）。返回端口与线程句柄。
fn spawn_pong_server() -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 256];
        let mut acc = Vec::new();
        // 等 PING 行（LF 或 CRLF 结尾）
        while !acc.windows(4).any(|w| w == b"ING\n") && !acc.windows(5).any(|w| w == b"ING\r\n") {
            let n = sock.read(&mut buf).expect("read ping");
            if n == 0 {
                return; // 对端提前关闭：让断言在退出码/报告处失败
            }
            acc.extend_from_slice(&buf[..n]);
        }
        sock.write_all(b"PONG 4f\n").expect("write pong");
        let _ = sock.flush();
        // 等对端 EOF（场景完成进程退出）
        loop {
            match sock.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    (port, handle)
}

// ---------- 参数面（clap 层拒绝 → exit 2） ----------

#[test]
fn run_without_scenario_fails_with_2() {
    let out = run_cli(&["run", "--tcp", "127.0.0.1:1"], None);
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("--scenario"),
        "{}",
        stderr_of(&out)
    );
}

#[test]
fn run_without_source_fails_with_2() {
    // run 是主动驱动设备的场景工具：无源一律 exit 2（不做 monitor 的交互选择）
    let dir = TempDir::new("nosource");
    let s = dir.write("s.json", PASS_SCENARIO);
    let out = run_cli(&["run", "--scenario", s.to_str().unwrap()], None);
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
    let err = stderr_of(&out);
    assert!(err.contains("--port") || err.contains("--tcp"), "{err}");
}

#[test]
fn run_rejects_bad_report_format_with_2() {
    let dir = TempDir::new("badfmt");
    let s = dir.write("s.json", PASS_SCENARIO);
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report-format",
            "xml",
        ],
        None,
    );
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
}

#[test]
fn run_help_lists_flags() {
    let out = run_cli(&["run", "--help"], None);
    assert_eq!(code(&out), 0, "stderr: {}", stderr_of(&out));
    let help = stdout_of(&out);
    assert!(help.contains("--scenario"), "{help}");
    assert!(help.contains("--report"), "{help}");
    assert!(help.contains("--report-format"), "{help}");
    assert!(help.contains("junit"), "{help}");
}

// ---------- 场景加载面（文件/校验失败 → exit 2，先于连接） ----------

#[test]
fn run_missing_scenario_file_is_2_not_connection_error() {
    let out = run_cli(
        &[
            "run",
            "--scenario",
            "/nonexistent/bytetide/scenario.json",
            "--tcp",
            "127.0.0.1:1",
        ],
        None,
    );
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("scenario.json"),
        "{}",
        stderr_of(&out)
    );
}

#[test]
fn run_invalid_scenario_fails_validation_with_2() {
    let dir = TempDir::new("badschema");
    let s = dir.write(
        "bad.json",
        r#"{
  "schema": "bytetide.other",
  "version": 1,
  "name": "bad",
  "steps": [ { "kind": "delay", "ms": 1 } ]
}"#,
    );
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
        ],
        None,
    );
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("invalid_schema"),
        "{}",
        stderr_of(&out)
    );
}

// ---------- fake host 后门（确定性 pass/fail/cancel/host 错误） ----------

#[test]
fn fake_host_pass_writes_report_and_keeps_stdout_clean() {
    let dir = TempDir::new("fakepass");
    let s = dir.write("s.json", PASS_SCENARIO);
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
        ],
        Some((FAKE_ENV, r#"{"lines":["PONG 4f"]}"#)),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr_of(&out));
    // 无 --report - 时 stdout 必须干净（数据纪律）
    assert!(stdout_of(&out).is_empty(), "stdout: {}", stdout_of(&out));
    // stderr 有逐行人读进度与摘要
    let err = stderr_of(&out);
    assert!(err.contains("[1/3] send"), "{err}");
    assert!(err.contains("[2/3] wait"), "{err}");
    assert!(err.contains("[3/3] assert"), "{err}");
    assert!(!err.contains("FAILED"), "{err}");
    assert!(err.contains("cli-pass"), "{err}");
    // 报告文件：status passed + wait/assert 命中行号（fake host 在首次交互时先注入
    // PONG=no1、后记 TX 回显=no2，故两步命中均为 no=1；TCP 路径的对照见 tcp 用例）
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "passed""#), "{body}");
    assert!(body.contains(r#""matched_no": 1"#), "{body}");
    assert!(!body.contains(r#""matched_no": 2"#), "{body}");
}

#[test]
fn fake_host_report_to_stdout_with_dash() {
    let dir = TempDir::new("fakestdout");
    let s = dir.write("s.json", PASS_SCENARIO);
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            "-",
        ],
        Some((FAKE_ENV, r#"{"lines":["PONG 4f"]}"#)),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr_of(&out));
    let stdout = stdout_of(&out);
    assert!(stdout.contains(r#""status": "passed""#), "{stdout}");
    assert!(!stdout.contains("[1/3]"), "进度不得混入 stdout: {stdout}");
}

#[test]
fn fake_host_junit_report_file() {
    let dir = TempDir::new("fakejunit");
    let s = dir.write("s.json", PASS_SCENARIO);
    let report = dir.path().join("report.xml");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
            "--report-format",
            "junit",
        ],
        Some((FAKE_ENV, r#"{"lines":["PONG 4f"]}"#)),
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr_of(&out));
    let body = std::fs::read_to_string(&report).expect("junit file written");
    assert!(body.starts_with("<?xml"), "{body}");
    assert!(
        body.contains(r#"<testsuite name="cli-pass""#) && body.contains("failures=\"0\""),
        "{body}"
    );
}

#[test]
fn fake_host_assert_failure_exits_3() {
    let dir = TempDir::new("fakeassert");
    let s = dir.write(
        "s.json",
        r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "cli-assert",
  "steps": [
    { "kind": "send", "mode": "ascii", "text": "PING", "appendNewline": true },
    { "kind": "wait", "matcher": { "literal": "PONG" }, "timeoutMs": 2000 },
    { "kind": "assert", "matcher": { "literal": "NOMATCH" }, "withinLast": 5, "message": "no NOMATCH in tail" }
  ]
}"#,
    );
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
        ],
        Some((FAKE_ENV, r#"{"lines":["PONG 4f"]}"#)),
    );
    assert_eq!(code(&out), 3, "stderr: {}", stderr_of(&out));
    let err = stderr_of(&out);
    assert!(err.contains("FAILED"), "{err}");
    assert!(err.contains("assert_failed"), "{err}");
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "failed""#), "{body}");
    assert!(body.contains("assert_failed"), "{body}");
}

#[test]
fn fake_host_wait_timeout_exits_3() {
    let dir = TempDir::new("fakewait");
    let s = dir.write(
        "s.json",
        r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "cli-wait",
  "steps": [
    { "kind": "wait", "matcher": { "literal": "NEVER" }, "timeoutMs": 150 }
  ]
}"#,
    );
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
        ],
        Some((FAKE_ENV, "{}")),
    );
    assert_eq!(code(&out), 3, "stderr: {}", stderr_of(&out));
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "failed""#), "{body}");
    assert!(body.contains("wait_timeout"), "{body}");
}

#[test]
fn fake_host_cancel_exits_130() {
    // cancelAfterCalls=1：send 完成后置位取消旗标（模拟 Ctrl-C），后续 wait 被中断
    let dir = TempDir::new("fakecancel");
    let s = dir.write(
        "s.json",
        r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "cli-cancel",
  "steps": [
    { "kind": "send", "mode": "ascii", "text": "PING", "appendNewline": true },
    { "kind": "wait", "matcher": { "literal": "PONG" }, "timeoutMs": 60000 }
  ]
}"#,
    );
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
        ],
        Some((FAKE_ENV, r#"{"cancelAfterCalls":1}"#)),
    );
    assert_eq!(code(&out), 130, "stderr: {}", stderr_of(&out));
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "cancelled""#), "{body}");
    let err = stderr_of(&out);
    assert!(err.contains("SKIP"), "{err}");
}

#[test]
fn fake_host_send_failure_exits_1() {
    let dir = TempDir::new("fakesend");
    let s = dir.write(
        "s.json",
        r#"{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "cli-sendfail",
  "steps": [
    { "kind": "send", "mode": "ascii", "text": "PING", "appendNewline": false }
  ]
}"#,
    );
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
            "--report",
            report.to_str().unwrap(),
        ],
        Some((FAKE_ENV, r#"{"failSend":true}"#)),
    );
    assert_eq!(code(&out), 1, "stderr: {}", stderr_of(&out));
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "failed""#), "{body}");
    assert!(body.contains("host_transport"), "{body}");
}

#[test]
fn fake_host_bad_env_json_exits_2() {
    let dir = TempDir::new("fakebadenv");
    let s = dir.write("s.json", PASS_SCENARIO);
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
        ],
        Some((FAKE_ENV, "not-json")),
    );
    assert_eq!(code(&out), 2, "stderr: {}", stderr_of(&out));
    assert!(stderr_of(&out).contains(FAKE_ENV), "{}", stderr_of(&out));
}

// ---------- loopback TCP（真实 transport 的 pass 路径） ----------

#[test]
fn tcp_loopback_roundtrip_passes() {
    let (port, server) = spawn_pong_server();
    let dir = TempDir::new("tcploop");
    let s = dir.write("s.json", PASS_SCENARIO);
    let report = dir.path().join("report.json");
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            &format!("127.0.0.1:{port}"),
            "--report",
            report.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&out), 0, "stderr: {}", stderr_of(&out));
    server.join().expect("pong server thread");
    let body = std::fs::read_to_string(&report).expect("report file written");
    assert!(body.contains(r#""status": "passed""#), "{body}");
    assert!(body.contains("cli-pass"), "{body}");
    // 真实链路：TX 回显先于 PONG 入表，wait 命中的是 PONG 行
    assert!(body.contains(r#""matched_no": 2"#), "{body}");
}

#[test]
fn tcp_connection_failure_exits_1() {
    // 端口 1 上无服务（连接拒绝）：连接失败 → exit 1
    let dir = TempDir::new("tcpfail");
    let s = dir.write("s.json", PASS_SCENARIO);
    let out = run_cli(
        &[
            "run",
            "--scenario",
            s.to_str().unwrap(),
            "--tcp",
            "127.0.0.1:1",
        ],
        None,
    );
    assert_eq!(code(&out), 1, "stderr: {}", stderr_of(&out));
    assert!(stderr_of(&out).contains("连接失败"), "{}", stderr_of(&out));
}
