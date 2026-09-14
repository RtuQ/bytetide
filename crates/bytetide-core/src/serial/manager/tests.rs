//! `serial::manager` 编排面单测（状态机/ingest/读循环集成在 runtime.rs，
//! 离线分页路由在 offline 模块，录制/捕获/传输契约在各自模块）。

//! manager 侧单测：编排访问器（perf/桥镜像/游标）与会话状态串。
//! （状态机/ingest 单测与读循环 loopback TCP 集成在 serial/runtime.rs，
//! 录制/捕获/传输契约测试在各自模块。）
use super::*;

fn mk_log(
    ts: &str,
    dir: super::super::port::Dir,
    text: &str,
    bytes: Option<Vec<u8>>,
    epoch: u64,
) -> LogLine {
    LogLine {
        ts: ts.into(),
        dir,
        text: text.into(),
        bytes,
        epoch_millis: epoch,
    }
}

#[test]
fn perf_snapshot_skips_stopped_and_offline() {
    let m = PortManager::new();
    let mk_handle = |stopped: bool| {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.ingest(
            &mk_log("t", super::super::port::Dir::Rx, "x", None, 1000),
            IngestOrigin::Transport,
            &crate::sink::NullSink,
        );
        let stop = Arc::new(AtomicBool::new(stopped));
        let (tx, _rx) = mpsc::channel();
        SessionHandle {
            config: PortConfig::default(),
            kind: SessionKind::Live,
            stop,
            write_tx: tx,
            log_path: Arc::new(RwLock::new(PathBuf::from("x.log"))),
            log_base: PathBuf::from("x.log"),
            join: None,
            runtime,
            plot: Arc::new(RwLock::new(PlotConfig::default())),
            bookmarks: Arc::new(RwLock::new(Vec::new())),
            alerts: Arc::new(RwLock::new(Vec::new())),
            annotations: Arc::new(RwLock::new(Vec::new())),
            offline: None,
            replay_tx: None,
        }
    };
    m.sessions.write().insert("s1".into(), mk_handle(false));
    m.sessions.write().insert("s2".into(), mk_handle(true)); // 已停止
    let ids: Vec<String> = m.perf_snapshot().into_iter().map(|(id, ..)| id).collect();
    assert_eq!(ids, vec!["s1".to_string()]);
}

#[test]
fn bridge_bookmarks_alerts_roundtrip_and_unknown_session() {
    let m = PortManager::new();
    let id = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
    assert!(m.bridge_bookmarks(&id).unwrap().is_empty());
    assert!(m.bridge_alerts(&id).unwrap().is_empty());

    let bms = vec![BridgeBookmark {
        no: 3,
        ts: "00:00:01.000".into(),
        text: "ERR line".into(),
    }];
    assert!(m.bridge_set_bookmarks(&id, bms.clone()));
    assert_eq!(m.bridge_bookmarks(&id).unwrap(), bms);

    let alerts = vec![BridgeAlert {
        id: "a1".into(),
        rule_id: "r1".into(),
        pattern: "ERR".into(),
        level: "err".into(),
        no: 3,
        ts: "00:00:01.000".into(),
        text: "ERR line".into(),
        at: 12345,
    }];
    assert!(m.bridge_set_alerts(&id, alerts.clone()));
    assert_eq!(m.bridge_alerts(&id).unwrap(), alerts);

    // 未知会话：写入 false、读取 None
    assert!(!m.bridge_set_bookmarks("nope", vec![]));
    assert!(m.bridge_bookmarks("nope").is_none());
    assert!(!m.bridge_set_alerts("nope", vec![]));
    assert!(m.bridge_alerts("nope").is_none());

    let notes = vec![BridgeAnnotation {
        id: "an1".into(),
        no: 9,
        ts: "00:00:09.000".into(),
        text: "ERR line".into(),
        note: "从这里开始校验失败".into(),
        at: 999,
    }];
    assert!(m.bridge_set_annotations(&id, notes.clone()));
    assert_eq!(m.bridge_annotations(&id).unwrap(), notes);
    assert!(!m.bridge_set_annotations("nope", vec![]));
    assert!(m.bridge_annotations("nope").is_none());
}

#[test]
fn bridge_last_no_reads_ring_cursor_without_full_snapshot() {
    let m = PortManager::new();
    // 未知会话 None
    assert_eq!(m.bridge_last_no("nope"), None);
    let id = m.load_offline(
        PortConfig::default(),
        PathBuf::from("x.log"),
        vec![
            mk_log("01:00:00.000", super::super::port::Dir::Rx, "a", None, 1000),
            mk_log("02:00:00.000", super::super::port::Dir::Tx, "b", None, 2000),
        ],
    );
    // /exchange 基线：仅游标值（与 buf.last_no 一致），不分配快照
    assert_eq!(m.bridge_last_no(&id), Some(2));
}

#[test]
fn session_state_offline_remains_offline() {
    let m = PortManager::new();
    let id = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
    let snap = m
        .bridge_list()
        .into_iter()
        .find(|s| s.id == id)
        .expect("离线会话在列表");
    assert_eq!(snap.status, "offline");
    assert_eq!(snap.last_error, None);
}

#[test]
fn bridge_line_by_no_distinguishes_missing_line_from_io_error() {
    // 复审 R-P2-2：源文件消失（存储故障）必须传播 Err，不得伪装成 Ok(None)
    let m = PortManager::new();
    let dir = crate::offline::test_support::temp_dir("line-by-no");
    let path = crate::offline::test_support::write_lines(&dir, "l.log", 3);
    let (id, _) = m
        .load_offline_indexed(PortConfig::default(), path.clone())
        .unwrap();
    // 行存在 / 行不存在（会话在）两条路径语义不变
    assert!(m.bridge_line_by_no(&id, 2).unwrap().is_some());
    assert!(m.bridge_line_by_no(&id, 99).unwrap().is_none());
    assert!(m.bridge_line_by_no(&id, 0).unwrap().is_none());
    // 存储故障 → Err
    std::fs::remove_file(&path).unwrap();
    assert!(
        m.bridge_line_by_no(&id, 2).is_err(),
        "离线页读 IO 错误必须传播"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn disconnect_parks_readonly_tombstone_for_final_drain() {
    // 评审 P1-2 回归：停止前 ring 里已有、前端尚未拉走的行，停止后仍可经
    // 墓碑补拉（两阶段关闭），显式释放后才彻底不可达
    let m = PortManager::new();
    let id = m.load_offline(
        PortConfig::default(),
        PathBuf::from("x.log"),
        vec![
            mk_log("01:00:00.000", super::super::port::Dir::Rx, "a", None, 1000),
            mk_log("02:00:00.000", super::super::port::Dir::Rx, "b", None, 2000),
        ],
    );
    // 最后一个拉取周期只拉到第 1 行；停止发生
    let page = m.ring_lines_after_no(&id, 0, 1).unwrap();
    assert_eq!(page.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1]);
    m.disconnect(&id).unwrap();
    // 句柄已移除（列表/发送面不可见），但墓碑补拉拿到尾行
    assert!(m.bridge_list().iter().all(|s| s.id != id));
    let tail = m.ring_lines_after_no(&id, 1, 10).unwrap();
    assert_eq!(
        tail.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
        vec!["b"]
    );
    // 第 2 阶段：前端拉空后显式释放 → 彻底消失；重复释放幂等
    m.release_dead(&id);
    assert!(m.ring_lines_after_no(&id, 1, 10).is_err());
    m.release_dead(&id);
    // 墓碑 FIFO 容量上限：超限淘汰最旧（未释放也最多留 DEAD_RING_CAP 份）
    let ids: Vec<String> = (0..DEAD_RING_CAP + 2)
        .map(|_| {
            let i = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
            m.disconnect(&i).unwrap();
            i
        })
        .collect();
    let dead = m.dead_rings.lock();
    assert!(dead.len() <= DEAD_RING_CAP);
    assert!(dead.iter().all(|(did, _)| !ids[..2].contains(did)));
}

// 读循环端到端集成（本机 loopback TCP 跑 session_thread/stream_loop）在
// serial/runtime.rs 测试——被测主循环在该文件；离线分页会话（load_offline_indexed）
// 的虚拟 ring 查询路由端到端用例在 offline 模块单测与 src-tauri/tests/offline_pages.rs。
// 本文件只保留编排面，行数受架构检查器 1000 行限额约束（scripts/check-architecture.mjs）。
