//! 触发式现场捕获（行车记录仪）：armed/deadline/档案文件行为收口在
//! [`CaptureController`]，消费 `&RingBuf` + `&dyn EventSink`。
//! Stage 2 Task 3 自 manager.rs 迁出：`CaptureRun` / `next_capture_path` /
//! `capture_start` / `cap_maybe_arm` / `cap_on_line` / `cap_finalize`，行为逐字保留——
//! 含「触发行既在 pre 窗口快照中、又经续写再入档一次」的既有双写行为（语义冻结不改）。

use std::path::{Path, PathBuf};

use parking_lot::RwLock;

use super::now_ms;
use super::port::LogLine;
use super::ring::RingBuf;
use super::rules::{clamp_capture_window, CaptureCfg};
use super::runtime::SessionState;
use crate::errors::err_msg;
use crate::session::SessionLog;
use crate::sink::{CaptureInfo, EventSink};

/// ingest 返回的捕获触发决策（由持有 CaptureController 的传输循环执行）。
/// `trigger`: "keyword" | "alert"（disconnect 触发走 [`CaptureController::trigger_on_disconnect`]）。
#[derive(Debug, Clone)]
pub struct CaptureHit {
    pub trigger: &'static str,
    pub rule: String,
}

/// 一次已触发的捕获：档案 writer + 后续窗口截止时刻。
/// writer 用 Option 承载创建失败（降级为不落盘但数据流不受影响）。
struct CaptureRun {
    writer: Option<SessionLog>,
    path: PathBuf,
    deadline_ms: u64,
    lines: u64,
    /// "keyword" | "alert" | "disconnect"
    trigger: &'static str,
    rule: String,
    at: u64,
}

/// 每会话捕获控制器：armed = 已触发、尚在写后续窗口；方法与既有自由函数一一对应。
pub struct CaptureController {
    run: Option<CaptureRun>,
    captures_dir: PathBuf,
}

impl CaptureController {
    /// `captures_dir` 为空路径 = 不落盘（CLI 缺省）：触发直接跳过建档。
    pub fn new(captures_dir: PathBuf) -> Self {
        Self {
            run: None,
            captures_dir,
        }
    }

    /// 是否 armed（已触发、尚未收尾）。
    pub fn is_armed(&self) -> bool {
        self.run.is_some()
    }

    /// 触发一次捕获（= 原 capture_start）：建档案、写头注释、把 ring 里 pre_ms
    /// 窗口的行回溯写入（含触发行本身——触发时它已入 ring）。
    /// 创建失败仅记 last_error（连接保持）不中断数据流。
    #[allow(clippy::too_many_arguments)]
    pub fn trigger(
        &mut self,
        session_id: &str,
        cfg: &CaptureCfg,
        ring: &RingBuf,
        trigger: &'static str,
        rule: &str,
        at_ms: u64,
        sink: &dyn EventSink,
        state: &RwLock<SessionState>,
    ) {
        if self.run.is_some() || self.captures_dir.as_os_str().is_empty() {
            return;
        }
        let pre = clamp_capture_window(cfg.pre_ms);
        let post = clamp_capture_window(cfg.post_ms);
        let now = chrono::Local::now();
        let path = next_capture_path(&self.captures_dir, session_id, &now, |p| p.exists());
        let mut writer = match SessionLog::create(&path) {
            Ok(w) => Some(w),
            Err(e) => {
                state.write().set_error(
                    sink,
                    session_id,
                    &err_msg(
                        "capture_create_failed",
                        format!("{}: {}", path.display(), e),
                    ),
                    None,
                );
                return;
            }
        };
        let mut lines = 0u64;
        if let Some(w) = writer.as_mut() {
            // 头注释行：前端解析按 `#` 跳过，供人肉/工具辨识来源
            w.write_raw_line(&format!(
                "# bytetide-capture v1 trigger={trigger} rule={} at_ms={at_ms} at={}",
                rule.replace(['\n', '\r', '\t'], " "),
                now.format("%Y-%m-%dT%H:%M:%S%.3f%:z"),
            ));
            let (snap, missing) = ring.lines_since_epoch(at_ms.saturating_sub(pre));
            if missing {
                w.write_raw_line("# 注意：更早的行已超出 ring 窗口，部分前置现场缺失");
            }
            for bl in &snap {
                w.append(&LogLine {
                    ts: bl.ts.clone(),
                    dir: bl.dir,
                    text: bl.text.clone(),
                    bytes: bl.bytes.clone(),
                    epoch_millis: bl.epoch_millis,
                });
                lines += 1;
            }
            let _ = w.flush();
        }
        // armed 通知（稀疏）：前端据此点亮「捕获中」呼吸指示；capture_saved 即解除
        sink.capture_active(session_id, rule);
        self.run = Some(CaptureRun {
            writer,
            path,
            deadline_ms: at_ms.saturating_add(post),
            lines,
            trigger,
            rule: rule.to_string(),
            at: at_ms,
        });
    }

    /// 逐行捕获处理（= 原 cap_maybe_arm + cap_on_line 串联）：命中规则触发/顺延，
    /// 随后本行续写armed 档案（未 armed 为 no-op）。
    #[allow(clippy::too_many_arguments)]
    pub fn commit(
        &mut self,
        hit: Option<CaptureHit>,
        cfg: &CaptureCfg,
        ring: &RingBuf,
        line: &LogLine,
        sink: &dyn EventSink,
        session_id: &str,
        state: &RwLock<SessionState>,
    ) {
        if let Some(hit) = hit.filter(|_| cfg.enabled) {
            match self.run.as_mut() {
                // armed 中再次命中顺延后续窗口（连续事故合并为一个档案）
                Some(run) => {
                    run.deadline_ms = now_ms().saturating_add(clamp_capture_window(cfg.post_ms))
                }
                None => self.trigger(
                    session_id,
                    cfg,
                    ring,
                    hit.trigger,
                    &hit.rule,
                    now_ms(),
                    sink,
                    state,
                ),
            }
        }
        self.on_line(line, sink, session_id);
    }

    /// armed 期间的每行追加 + 到期收尾（= 原 cap_on_line；未 armed 为 no-op）。
    pub fn on_line(&mut self, line: &LogLine, sink: &dyn EventSink, session_id: &str) {
        let Some(run) = self.run.as_mut() else { return };
        if let Some(w) = run.writer.as_mut() {
            w.append(line);
        }
        run.lines += 1;
        if now_ms() >= run.deadline_ms {
            self.finalize(sink, session_id);
        }
    }

    /// 后续窗口到期收尾（窗口内无新行也要收尾，否则档案悬着不发事件）。
    pub fn expire_if_due(&mut self, sink: &dyn EventSink, session_id: &str) {
        if let Some(run) = self.run.as_ref() {
            if now_ms() >= run.deadline_ms {
                self.finalize(sink, session_id);
            }
        }
    }

    /// 断连现场（= stream_loop 尾部的 disconnect 触发块）：未 armed 且开关/断连
    /// 触发启用时，把最后 pre_ms 窗口抓成档案（设备重启/掉线现场最珍贵）。
    pub fn trigger_on_disconnect(
        &mut self,
        cfg: &CaptureCfg,
        ring: &RingBuf,
        sink: &dyn EventSink,
        session_id: &str,
        state: &RwLock<SessionState>,
    ) {
        if self.run.is_none() && cfg.enabled && cfg.on_disconnect {
            let now = now_ms();
            self.trigger(
                session_id,
                cfg,
                ring,
                "disconnect",
                "断连",
                now,
                sink,
                state,
            );
        }
    }

    /// 收尾：flush + capture_saved 稀疏事件（= 原 cap_finalize；未 armed 为 no-op）。
    pub fn finalize(&mut self, sink: &dyn EventSink, session_id: &str) {
        let Some(mut run) = self.run.take() else {
            return;
        };
        if let Some(mut w) = run.writer.take() {
            let _ = w.flush();
        }
        sink.capture_saved(
            session_id,
            CaptureInfo {
                path: run.path.to_string_lossy().into_owned(),
                trigger: run.trigger.to_string(),
                rule: run.rule,
                lines: run.lines,
                at: run.at,
            },
        );
    }
}

/// 现场档案路径：{dir}/{id}-cap-YYYYMMDD-HHMMSS[-N].log（同秒冲突 -2/-3 递增，与分段规则一致）。
pub fn next_capture_path(
    dir: &Path,
    id: &str,
    now: &chrono::DateTime<chrono::Local>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let mut n = 1u32;
    loop {
        let name = if n == 1 {
            format!("{id}-cap-{stamp}.log")
        } else {
            format!("{id}-cap-{stamp}-{n}.log")
        };
        let p = dir.join(name);
        if !exists(&p) {
            return p;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    //! 捕获行为契约：pre/post 窗口、再触发顺延、缺失标记、事件顺序、禁用态、断连触发。
    use std::sync::Arc;

    use parking_lot::RwLock;

    use super::super::port::Dir;
    use super::super::ring::RING_CAP;
    use super::super::runtime::SessionStatus;
    use super::*;
    use crate::sink::VecSink;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bytetide-cap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create temp dir");
        d
    }

    fn mk_line(text: &str, epoch: u64) -> LogLine {
        LogLine {
            ts: "00:00:00.000".into(),
            dir: Dir::Rx,
            text: text.into(),
            bytes: None,
            epoch_millis: epoch,
        }
    }

    fn enabled_cfg() -> CaptureCfg {
        CaptureCfg {
            enabled: true,
            on_disconnect: false,
            pre_ms: 60_000,
            post_ms: 60_000,
            rules: Vec::new(),
        }
    }

    fn state() -> Arc<RwLock<SessionState>> {
        Arc::new(RwLock::new(SessionState::default()))
    }

    fn data_lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("read archive")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect()
    }

    #[test]
    fn next_capture_path_avoids_conflicts() {
        let dir = Path::new("/tmp");
        let now = chrono::Local::now();
        let taken: Vec<PathBuf> = vec![];
        let p1 = next_capture_path(dir, "s1", &now, |p| taken.contains(&p.to_path_buf()));
        assert_eq!(
            p1.file_name().unwrap().to_string_lossy(),
            format!("s1-cap-{}.log", now.format("%Y%m%d-%H%M%S"))
        );
        let stamp = p1.clone();
        let p2 = next_capture_path(dir, "s1", &now, |p| *p == stamp);
        assert_eq!(
            p2.file_name().unwrap().to_string_lossy(),
            format!("s1-cap-{}-2.log", now.format("%Y%m%d-%H%M%S"))
        );
        let stamp2 = p2.clone();
        let p3 = next_capture_path(dir, "s1", &now, |p| *p == stamp || *p == stamp2);
        assert!(p3
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-3.log"));
    }

    #[test]
    fn keyword_trigger_writes_header_prewindow_and_keeps_double_trigger_line() {
        let dir = temp_dir("keyword");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        // 行 epoch 用真实墙钟基准（pre 窗口回溯按 epoch_millis 比较）
        let t0 = now_ms();
        ring.push(&mk_line("pre-1", t0 - 2000));
        ring.push(&mk_line("pre-2", t0 - 1000));
        let trigger_line = mk_line("ERROR overtemp", t0);
        ring.push(&trigger_line);

        let mut cap = CaptureController::new(dir.clone());
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &enabled_cfg(),
            &ring,
            &trigger_line,
            &sink,
            "s1",
            &st,
        );
        assert!(cap.is_armed());
        // 事件：armed 通知；错误状态未被触碰
        assert_eq!(
            sink.0.lock().clone(),
            vec!["capture-active s1 ERROR".to_string()]
        );
        assert_eq!(st.read().status, SessionStatus::Connecting);

        // 档案：头注释 + pre 窗口两行 + 触发行（快照一次 + 续写一次——既有双写行为冻结）
        let saved_path = {
            // 用 finalize 前的事件无法拿路径，直接扫目录唯一档案
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            entries.sort();
            entries[0].clone()
        };
        let raw = std::fs::read_to_string(&saved_path).expect("read archive");
        assert!(raw.starts_with("# bytetide-capture v1 trigger=keyword rule=ERROR at_ms="));

        // 后续窗口内的行继续入档（BufWriter 批量刷写：快照段在建档时已 flush，
        // 续写段随 64 行批/收尾 flush 落盘——与既有行为一致，这里以 finalize 后读取为准）
        cap.on_line(&mk_line("post-1", t0 + 1), &sink, "s1");
        assert!(cap.is_armed());
        cap.finalize(&sink, "s1");
        assert!(!cap.is_armed());

        // 档案内容：pre-1, pre-2, 触发行(快照), 触发行(续写——既有双写行为冻结), post-1
        let lines = data_lines(&saved_path);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "00:00:00.000\tRX\tpre-1");
        assert_eq!(lines[2], lines[3]);

        // 事件序：active -> saved；行数=快照 3 + 双写 1 + post 1
        let events = sink.0.lock().clone();
        assert_eq!(events.len(), 2);
        assert!(events[0].starts_with("capture-active s1 "));
        assert!(events[1].starts_with("capture s1 "));
        assert!(events[1].contains("n=5"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retrigger_extends_deadline_without_new_archive() {
        let dir = temp_dir("retrigger");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        let cfg = enabled_cfg();
        let line = mk_line("ERROR a", 1000);
        ring.push(&line);

        let mut cap = CaptureController::new(dir.clone());
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &cfg,
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        assert!(cap.is_armed());

        // 把截止时刻拨到过去：再次命中必须顺延（重新钳 post 窗口），而不是立刻收尾
        if let Some(r) = cap.run.as_mut() {
            r.deadline_ms = now_ms() - 1;
        }
        let cfg2 = CaptureCfg {
            post_ms: 60_000,
            ..cfg.clone()
        };
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &cfg2,
            &ring,
            &mk_line("ERROR b", 2000),
            &sink,
            "s1",
            &st,
        );
        assert!(cap.is_armed(), "再触发应顺延而非收尾");
        assert!(cap.run.as_ref().expect("armed").deadline_ms >= now_ms());
        // 仅一个档案、一次 armed 事件
        assert_eq!(sink.0.lock().len(), 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expire_if_due_finalizes_without_new_lines() {
        let dir = temp_dir("expire");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        let line = mk_line("ERROR x", 1000);
        ring.push(&line);
        let mut cap = CaptureController::new(dir.clone());
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &CaptureCfg {
                post_ms: 1_000,
                ..enabled_cfg()
            },
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        // at_ms=真实墙钟：post 最小 1s，手动把 deadline 拨到过去模拟窗口走完
        if let Some(r) = cap.run.as_mut() {
            r.deadline_ms = now_ms() - 1;
        }
        cap.expire_if_due(&sink, "s1");
        assert!(!cap.is_armed());
        let events = sink.0.lock().clone();
        assert_eq!(events.len(), 2, "active -> saved 顺序");
        assert!(events[0].starts_with("capture-active "));
        assert!(events[1].starts_with("capture s1 "));
        // 重复收尾幂等（未 armed 为 no-op）
        cap.finalize(&sink, "s1");
        assert_eq!(sink.0.lock().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_marker_written_when_ring_evicted_earlier_lines() {
        let dir = temp_dir("missing");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        let n = (RING_CAP + 5) as u64;
        for i in 0..n {
            ring.push(&mk_line("e", 1000 + i));
        }
        // at_ms=1500 < 现存首行 epoch(1005)：回溯窗口起点早于 ring 最早行 → 标缺失
        let line = mk_line("ERROR", 1500);
        ring.push(&line);
        let mut cap = CaptureController::new(dir.clone());
        cap.trigger(
            "s1",
            &enabled_cfg(),
            &ring,
            "keyword",
            "ERROR",
            1500,
            &sink,
            &st,
        );
        assert!(cap.is_armed());
        let archive = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let raw = std::fs::read_to_string(&archive).expect("read archive");
        assert!(
            raw.contains("# 注意：更早的行已超出 ring 窗口，部分前置现场缺失"),
            "淘汰后必须写缺失标记: {raw}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_capture_and_alert_linkage_paths() {
        let dir = temp_dir("disabled");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        let line = mk_line("ERROR", 1000);
        ring.push(&line);
        // 禁用态：即使传了 hit 也不触发（决策侧 capture_eval 对禁用返回空，这里兜底验证）
        let mut cap = CaptureController::new(dir.clone());
        let off = CaptureCfg {
            enabled: false,
            ..enabled_cfg()
        };
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &off,
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        assert!(!cap.is_armed());
        assert!(sink.0.lock().is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        // 告警联动触发（trigger="alert"）
        cap.commit(
            Some(CaptureHit {
                trigger: "alert",
                rule: "ERR".into(),
            }),
            &enabled_cfg(),
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        assert!(cap.is_armed());
        assert_eq!(
            sink.0.lock().first().map(String::as_str),
            Some("capture-active s1 ERR")
        );
        let archive = std::fs::read_to_string(
            std::fs::read_dir(&dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path(),
        )
        .unwrap();
        assert!(archive.starts_with("# bytetide-capture v1 trigger=alert rule=ERR at_ms="));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_captures_dir_skips_archiving_silently() {
        // CLI 缺省：空路径 = 不落盘，触发直接跳过、无事件
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        let line = mk_line("ERROR", 1000);
        ring.push(&line);
        let mut cap = CaptureController::new(PathBuf::new());
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &enabled_cfg(),
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        assert!(!cap.is_armed());
        assert!(sink.0.lock().is_empty());
    }

    #[test]
    fn create_failure_records_error_and_keeps_connection_state() {
        let dir = temp_dir("fail");
        // captures_dir 指向一个文件：建档必败
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("write blocker");
        let sink = VecSink::default();
        let st = state();
        {
            let mut s = st.write();
            s.status = SessionStatus::Connected;
        }
        let ring = RingBuf::new();
        let line = mk_line("ERROR", 1000);
        ring.push(&line);
        let mut cap = CaptureController::new(blocker.clone());
        cap.commit(
            Some(CaptureHit {
                trigger: "keyword",
                rule: "ERROR".into(),
            }),
            &enabled_cfg(),
            &ring,
            &line,
            &sink,
            "s1",
            &st,
        );
        assert!(!cap.is_armed());
        // 仅记 last_error（连接保持），不发 armed 事件、不改状态
        let events = sink.0.lock().clone();
        assert_eq!(events.len(), 1);
        assert!(
            events[0].starts_with("error s1 capture_create_failed|"),
            "建档失败只发 error 事件: {events:?}"
        );
        let st = st.read();
        assert!(st
            .last_error
            .as_deref()
            .unwrap_or_default()
            .starts_with("capture_create_failed|"));
        assert_eq!(st.status, SessionStatus::Connected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disconnect_trigger_requires_enabled_and_not_armed() {
        let dir = temp_dir("disconnect");
        let sink = VecSink::default();
        let st = state();
        let ring = RingBuf::new();
        // 行 epoch 用真实墙钟（pre 窗口回溯按 epoch_millis 与 at_ms-pre 比较）
        ring.push(&mk_line("boom before crash", now_ms()));
        let mut cap = CaptureController::new(dir.clone());
        // 未启用断连捕获：不触发
        cap.trigger_on_disconnect(
            &CaptureCfg {
                enabled: true,
                on_disconnect: false,
                ..enabled_cfg()
            },
            &ring,
            &sink,
            "s1",
            &st,
        );
        assert!(!cap.is_armed());
        // 启用：触发并写头注释 trigger=disconnect
        cap.trigger_on_disconnect(
            &CaptureCfg {
                enabled: true,
                on_disconnect: true,
                ..enabled_cfg()
            },
            &ring,
            &sink,
            "s1",
            &st,
        );
        assert!(cap.is_armed());
        let archive_path = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let archive = std::fs::read_to_string(&archive_path).unwrap();
        assert!(archive.starts_with("# bytetide-capture v1 trigger=disconnect rule=断连 at_ms="));
        assert_eq!(
            data_lines(&archive_path),
            vec!["00:00:00.000\tRX\tboom before crash".to_string()]
        );
        // 已 armed 时断连不另起档案
        cap.trigger_on_disconnect(
            &CaptureCfg {
                enabled: true,
                on_disconnect: true,
                ..enabled_cfg()
            },
            &ring,
            &sink,
            "s1",
            &st,
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
