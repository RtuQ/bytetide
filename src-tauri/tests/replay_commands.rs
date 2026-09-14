//! 回放命令层集成测试（Stage 3 Task 7）：真 PortManager + 临时回放文件，走命令层
//! 同一实现路径（parse_replay_action / apply_replay_control / build_view——tauri 壳
//! 只差 State 提取与 replay-state 事件 emit）。覆盖 open 索引摘要、pause/resume/
//! seek/speed/loop/stop 全控制面、非法 action/value 稳定文案、close 后 status Err、
//! send 对 replay 拒绝透传、非回放会话拒绝控制。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytetide_core::serial::manager::{SendMode, SendRequest};
use bytetide_core::serial::PortManager;
use serial_tool_lib::commands::{
    apply_replay_control, build_view, parse_replay_action, ReplayMeta, ReplayRegistry,
};

/// n 行、相邻原始间隔 1000ms 的日志（ts 为当日毫秒：首行 1000，末行 n*1000）。
fn timed_log(dir: &Path, name: &str, n: u64) -> PathBuf {
    let path = dir.join(name);
    let f = std::fs::File::create(&path).expect("create log file");
    let mut w = BufWriter::new(f);
    for i in 1..=n {
        let ms = i * 1_000;
        writeln!(
            w,
            "{:02}:{:02}:{:02}.{:03}\tRX\tline-{i}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1_000 % 60,
            ms % 1_000
        )
        .expect("write line");
    }
    w.flush().expect("flush log");
    path
}

/// 唯一临时目录（进程 id + 纳秒），测试结束 remove_dir_all。
fn temp_dir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "bytetide-it-replay-{tag}-{}-{nanos}",
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

/// 轮询直到条件成立（回放线程异步推进；命令层只能经查询面观测）。
fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while !f() {
        assert!(Instant::now() < deadline, "wait_until 超时");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// speed 10：相邻 1000ms 原始间隔 → 每段真实睡眠 100ms，既有可观测窗口又不拖慢测试。
fn fast_meta() -> ReplayMeta {
    ReplayMeta {
        speed: 10.0,
        looped: false,
    }
}

#[test]
fn open_then_full_control_flow_pause_resume_seek_speed_loop_stop() {
    let dir = temp_dir("flow");
    let path = timed_log(dir.0.as_path(), "flow.log", 30);
    let m = PortManager::new();
    let replays = ReplayRegistry::default();
    // open：命令层同路径（start_replay_indexed + 镜像登记），30 行 1000ms 间隔
    // → durationMs = 30000 - 1000 = 29000
    let (id, _tx, index) = m
        .start_replay_indexed(
            &path,
            bytetide_core::replay::ReplayConfig::default(),
            std::sync::Arc::new(bytetide_core::sink::VecSink::default()),
            PathBuf::new(),
        )
        .expect("start_replay_indexed");
    assert!(id.starts_with('r'), "回放会话 id 前缀 r: {id}");
    assert_eq!(index.line_count, 30);
    assert_eq!(index.last_epoch.saturating_sub(index.first_epoch), 29_000);
    replays.track(&id, fast_meta());

    // status：起跑即 Running（首行入库紧随其后，中间有起跑竞态窗），等水位 ≥1
    wait_until(2_000, || {
        build_view(&m, &replays, &id)
            .map(|v| v.state == "running" && v.line >= 1)
            .unwrap_or(false)
    });
    let view = build_view(&m, &replays, &id).unwrap();
    assert_eq!(view.session_id, id);
    assert_eq!(view.speed, 10.0);
    assert!(!view.looped);
    assert!(view.line >= 1, "首行已入库（水位推进）");

    // pause → Paused（命令层等待到位）
    let cmd = parse_replay_action("pause", None).expect("pause");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply pause");
    assert_eq!(build_view(&m, &replays, &id).unwrap().state, "paused");

    // 暂停中 seek(5)：水位=目标-1、不入行（确定性断言，无时序竞态）
    let cmd = parse_replay_action("seek", Some(5.0)).expect("seek");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply seek");
    let view = build_view(&m, &replays, &id).unwrap();
    assert_eq!(view.state, "paused", "Paused 下 seek 保持暂停");
    assert_eq!(view.line, 4, "seek 只推水位（=目标-1），恢复后才入库");

    // resume → Running，第 5 行立即入库
    let cmd = parse_replay_action("resume", None).expect("resume");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply resume");
    assert_eq!(build_view(&m, &replays, &id).unwrap().state, "running");
    wait_until(2_000, || {
        build_view(&m, &replays, &id)
            .map(|v| v.line >= 5)
            .unwrap_or(false)
    });

    // speed(2) / loop(on)：镜像登记随命令更新（状态不变）
    let cmd = parse_replay_action("speed", Some(2.0)).expect("speed");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply speed");
    let cmd = parse_replay_action("loop", Some(1.0)).expect("loop");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply loop");
    let view = build_view(&m, &replays, &id).unwrap();
    assert_eq!(view.speed, 2.0);
    assert!(view.looped);
    assert_eq!(view.state, "running", "speed/loop 不改状态");

    // stop → Stopped（幂等：再 stop 不报错）
    let cmd = parse_replay_action("stop", None).expect("stop");
    apply_replay_control(&m, &replays, &id, cmd).expect("apply stop");
    wait_until(2_000, || {
        build_view(&m, &replays, &id)
            .map(|v| v.state == "stopped")
            .unwrap_or(false)
    });
    let cmd = parse_replay_action("stop", None).expect("stop again");
    apply_replay_control(&m, &replays, &id, cmd).expect("stop 幂等");

    // close（disconnect = Stop+join+移除会话）后 status Err
    m.disconnect(&id).expect("disconnect");
    let err = build_view(&m, &replays, &id).unwrap_err();
    assert!(
        err.contains("会话不存在"),
        "close 后 status 稳定报错: {err}"
    );
}

#[test]
fn invalid_action_and_value_yield_stable_errors() {
    // 未知 action
    assert_eq!(
        parse_replay_action("fly", None).unwrap_err(),
        "未知回放控制: fly"
    );
    // seek：缺值 / 非正数 / 小数 / NaN
    assert!(parse_replay_action("seek", None)
        .unwrap_err()
        .contains("seek"));
    assert!(parse_replay_action("seek", Some(0.0))
        .unwrap_err()
        .contains("seek"));
    assert!(parse_replay_action("seek", Some(-3.0))
        .unwrap_err()
        .contains("seek"));
    assert!(parse_replay_action("seek", Some(2.5))
        .unwrap_err()
        .contains("seek"));
    assert!(parse_replay_action("seek", Some(f64::NAN))
        .unwrap_err()
        .contains("seek"));
    // 合法 seek 解析为 u64 行号
    assert_eq!(
        format!("{:?}", parse_replay_action("seek", Some(7.0)).unwrap()),
        "SeekLine(7)"
    );
    // speed：缺值 / 0 / 越界 / 负数 / NaN / inf
    assert!(parse_replay_action("speed", None)
        .unwrap_err()
        .contains("速度"));
    assert!(parse_replay_action("speed", Some(0.0))
        .unwrap_err()
        .contains("速度"));
    assert!(parse_replay_action("speed", Some(100.1))
        .unwrap_err()
        .contains("速度"));
    assert!(parse_replay_action("speed", Some(-1.0))
        .unwrap_err()
        .contains("速度"));
    assert!(parse_replay_action("speed", Some(f64::NAN))
        .unwrap_err()
        .contains("速度"));
    assert!(parse_replay_action("speed", Some(f64::INFINITY))
        .unwrap_err()
        .contains("速度"));
    // 边界值合法
    assert!(parse_replay_action("speed", Some(0.1)).is_ok());
    assert!(parse_replay_action("speed", Some(100.0)).is_ok());
    // loop：仅 0/1
    assert!(parse_replay_action("loop", Some(0.5))
        .unwrap_err()
        .contains("循环"));
    assert_eq!(
        format!("{:?}", parse_replay_action("loop", Some(0.0)).unwrap()),
        "SetLoop(false)"
    );
    assert_eq!(
        format!("{:?}", parse_replay_action("loop", Some(1.0)).unwrap()),
        "SetLoop(true)"
    );
    // pause/resume/stop 不带值
    assert!(parse_replay_action("pause", None).is_ok());
    assert!(parse_replay_action("resume", None).is_ok());
    assert!(parse_replay_action("stop", None).is_ok());
}

#[test]
fn send_and_disk_ops_reject_replay_and_control_rejects_non_replay() {
    let dir = temp_dir("guards");
    let path = timed_log(dir.0.as_path(), "guards.log", 10);
    let m = PortManager::new();
    let replays = ReplayRegistry::default();
    let (id, _tx, _index) = m
        .start_replay_indexed(
            &path,
            bytetide_core::replay::ReplayConfig::default(),
            std::sync::Arc::new(bytetide_core::sink::VecSink::default()),
            PathBuf::new(),
        )
        .expect("start_replay_indexed");
    replays.track(&id, fast_meta());

    // send/落盘对 replay 拒绝（命令层透传 manager 稳定文案）
    assert_eq!(
        m.send(
            &id,
            SendRequest {
                mode: SendMode::Ascii,
                text: "x".into()
            }
        )
        .unwrap_err()
        .to_string(),
        "回放会话不支持发送"
    );
    assert_eq!(
        m.set_recording(&id, true).unwrap_err().to_string(),
        "回放会话不支持落盘"
    );

    // 控制面对非回放会话 / 不存在会话拒绝
    let offline_id = m
        .load_offline_indexed(
            bytetide_core::serial::port::PortConfig::default(),
            path.clone(),
        )
        .expect("load offline")
        .0;
    assert_eq!(
        parse_replay_action("pause", None)
            .and_then(|cmd| apply_replay_control(&m, &replays, &offline_id, cmd))
            .unwrap_err(),
        "非回放会话"
    );
    assert_eq!(
        build_view(&m, &replays, &offline_id).unwrap_err(),
        "会话不存在或非回放会话"
    );
    let err = parse_replay_action("stop", None)
        .and_then(|cmd| apply_replay_control(&m, &replays, "r9999", cmd))
        .unwrap_err();
    assert!(err.contains("会话不存在"), "不存在会话稳定报错: {err}");
    assert_eq!(
        build_view(&m, &replays, "r9999").unwrap_err(),
        "会话不存在或非回放会话"
    );

    m.disconnect(&id).expect("disconnect");
    m.disconnect(&offline_id).expect("disconnect offline");
}
