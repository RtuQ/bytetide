//! 时序回放（Stage 3 Task 6）：离线日志按相邻行原始时间差重放为伪实时会话。
//! 回放是 [`crate::serial::runtime::SessionRuntime::ingest`] 的又一个生产者
//! （`IngestOrigin::Replay`）——ring 拉模型、解析、绘图、告警、自动化全链路天然
//! 复用，无并行前端模拟。
//!
//! - [`model`]：[`ReplayConfig`]（速度有限且 0.1..=100.0、looped、gap 钳制上限）/
//!   [`ReplayState`]（细粒度回放状态）/ [`ReplayCmd`]（控制通道命令）。
//! - [`runner`]：[`spawn_replay`] 调度线程（可注入 ReplayClock，生产=单调墙钟）。
//!
//! 管理面（`PortManager::start_replay`，id 前缀 `r`）：`SessionKind::Replay` 无链路
//! 无落盘（write_tx 断开通道占位，断开语义对齐 Offline），ring 游标/桥查询面复用；
//! 守卫面——send/信号线/落盘（录制/分段）报「回放会话不支持…」，set_live_rules 与
//! clear_log 允许（告警规则经 common ingest 仍评估并稀疏上报，自动回复被 Replay
//! origin 结构性排除；clear 直接清 ring、不回放游标）。会话状态映射见 runner 模块
//! 文档；disconnect = 发 Stop + join。

pub mod model;
pub mod runner;

pub use model::{ReplayCmd, ReplayConfig, ReplayState};
pub use runner::spawn_replay;

#[cfg(test)]
mod tests {
    //! manager 集成端到端：start_replay 建会话 → 拉模型查询 → seek → 守卫 →
    //! disconnect 清理。调度时序细节在 runner 单测（fake clock），此处走生产
    //! 装配路径（WallClock），用 1ms 原始间隔 + 高倍速保证不靠真实等待。

    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::replay::{ReplayCmd, ReplayConfig};
    use crate::serial::manager::{PortManager, SendMode, SendRequest};
    use crate::serial::rules::{AlertCfg, AlertRuleCfg, AutoReplyCfg, CaptureCfg};
    use crate::sink::VecSink;

    /// 轮询直到条件成立（回放线程异步推进；manager 侧只能经查询面观测）。
    fn wait_until(timeout_ms: u64, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while !f() {
            assert!(Instant::now() < deadline, "wait_until 超时");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// 临时目录守卫：drop 时尽力清理。
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let dir = std::env::temp_dir().join(format!(
                "bytetide-replay-mgr-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    /// 5 行、相邻原始间隔 1000ms 的日志（speed 10 → 每段真实睡眠 100ms）。
    fn timed_log(tag: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(tag);
        let mut body = String::new();
        for i in 0..5u64 {
            let ms = 1_000 + i * 1_000;
            body.push_str(&format!(
                "{:02}:{:02}:{:02}.{:03}\tRX\tline-{i}\n",
                ms / 3_600_000,
                ms / 60_000 % 60,
                ms / 1_000 % 60,
                ms % 1_000
            ));
        }
        let path = dir.path().join("t.log");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    fn fast_cfg() -> ReplayConfig {
        // 原始间隔 1000ms @speed 10 → 每段真实睡眠 100ms：既有可观测窗口又不拖慢测试
        ReplayConfig {
            speed: 10.0,
            ..ReplayConfig::default()
        }
    }

    #[test]
    fn start_replay_streams_lines_queries_seek_and_cleans_up() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let (_g, path) = timed_log("lifecycle");
        let (id, tx) = m
            .start_replay(&path, fast_cfg(), sink.clone(), PathBuf::new())
            .expect("start_replay");
        assert!(id.starts_with('r'), "回放会话 id 前缀 r: {id}");
        // 会话状态映射：运行中 connected（首行立即入库，随后 100ms 级 gap 睡眠）
        wait_until(2_000, || {
            m.bridge_list()
                .iter()
                .any(|s| s.id == id && s.status == "connected")
        });
        // 拉模型查询面天然工作（ring 在 runtime；offline=None → ring 路由）
        wait_until(5_000, || m.bridge_last_no(&id) == Some(5));
        let lines = m.ring_lines_after_no(&id, 0, 10).unwrap();
        assert_eq!(
            lines.iter().map(|l| l.no).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(lines[0].text, "line-0");
        assert_eq!(m.session_log_path(&id).unwrap(), path.to_string_lossy());
        // 细粒度状态查询面：非回放会话 None、回放会话有值
        assert!(m.replay_state(&id).is_some());
        // seek：ring 清屏、seq 单调续（5+5=10）
        tx.send(ReplayCmd::SeekLine(1)).unwrap();
        wait_until(5_000, || m.bridge_last_no(&id) == Some(10));
        assert_eq!(m.ring_bounds(&id).unwrap().first_no, 6);
        // 断开 = Stop + join + 移除会话
        m.disconnect(&id).unwrap();
        assert!(m.ring_lines_after_no(&id, 0, 10).is_err(), "会话已移除");
        assert_eq!(m.replay_state(&id), None);
        assert!(!m.bridge_list().iter().any(|s| s.id == id));
    }

    #[test]
    fn replay_guards_reject_disabled_ops_but_allow_rules_and_clear() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let (_g, path) = timed_log("guards");
        let (id, tx) = m
            .start_replay(&path, fast_cfg(), sink.clone(), PathBuf::new())
            .unwrap();
        // 禁用操作：稳定错误「回放会话不支持…」
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
            "回放会话不支持发送"
        );
        assert_eq!(
            err(m
                .set_signal(&id, crate::serial::transport::Pin::Dtr, true)
                .unwrap_err()),
            "回放会话不支持信号线"
        );
        assert_eq!(err(m.rotate_log(&id).unwrap_err()), "回放会话不支持落盘");
        assert_eq!(
            err(m.set_recording(&id, false).unwrap_err()),
            "回放会话不支持落盘"
        );
        assert_eq!(
            err(m.set_recording(&id, true).unwrap_err()),
            "回放会话不支持落盘"
        );
        // 首轮播完（线程驻留 Finished）
        wait_until(5_000, || m.bridge_last_no(&id) == Some(5));
        // 允许：set_live_rules（告警要工作）与 clear_log（清 ring 不动回放游标）
        m.set_live_rules(
            &id,
            AutoReplyCfg::default(),
            AlertCfg {
                enabled: true,
                rules: vec![AlertRuleCfg {
                    id: "a1".into(),
                    pattern: "line-2".into(),
                    min_count: 1,
                    level: "warn".into(),
                    enabled: true,
                    ..AlertRuleCfg::default()
                }],
            },
            CaptureCfg::default(),
        )
        .unwrap();
        m.clear_log(&id).unwrap();
        // seek 重播：规则生效后 line-2 再入库 → 告警经 common ingest 仍评估，
        // sink 收到稀疏 alert-hit（auto-reply 结构性不回写）
        tx.send(ReplayCmd::SeekLine(1)).unwrap();
        wait_until(5_000, || m.bridge_last_no(&id) == Some(10));
        wait_until(2_000, || {
            sink.0
                .lock()
                .iter()
                .any(|e| e.starts_with(&format!("alert-hit {id} ")))
        });
        m.disconnect(&id).unwrap();
    }

    #[test]
    fn start_replay_validates_config_and_missing_file_before_session() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let (_g, path) = timed_log("validate");
        // 速度非法：执行前失败，不建会话
        let bad = ReplayConfig {
            speed: 0.0,
            ..ReplayConfig::default()
        };
        assert!(m
            .start_replay(&path, bad, sink.clone(), PathBuf::new())
            .unwrap_err()
            .to_string()
            .contains("回放配置非法"));
        // 源文件缺失：open_offline 失败，不建会话
        let missing = _g.path().join("nope.log");
        assert!(m
            .start_replay(&missing, fast_cfg(), sink, PathBuf::new())
            .is_err());
        assert!(m.bridge_list().is_empty(), "失败路径不留会话");
    }

    #[test]
    fn disconnect_mid_replay_stops_thread_promptly() {
        let m = PortManager::new();
        let sink = Arc::new(VecSink::default());
        let dir = TempDir::new("midrun");
        // 长回放（500 行 × 1s 原始间隔 @speed 1）：运行中断开须立即回收
        let mut body = String::new();
        for i in 0..500u64 {
            let ms = i * 1_000 % 86_400_000;
            body.push_str(&format!(
                "{:02}:{:02}:{:02}.{:03}\tRX\tline-{i}\n",
                ms / 3_600_000,
                ms / 60_000 % 60,
                ms / 1_000 % 60,
                ms % 1_000
            ));
        }
        let path = dir.path().join("long.log");
        std::fs::write(&path, body).unwrap();
        let (id, _tx) = m
            .start_replay(&path, ReplayConfig::default(), sink, PathBuf::new())
            .unwrap();
        wait_until(2_000, || m.bridge_last_no(&id) == Some(1));
        let started = Instant::now();
        m.disconnect(&id).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "disconnect 不等回放播完"
        );
        assert!(m.ring_lines_after_no(&id, 0, 10).is_err());
        assert!(m.bridge_list().is_empty());
    }
}
