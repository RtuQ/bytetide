//! 落盘录制控制：`SessionLog` writer 的开/写/分段/暂停/午夜轮转收口在
//! [`RecordingController`]。Stage 2 Task 3 自 manager.rs 迁出：
//! `next_segment_path` / `segment_due` / `default_log_path` 与 stream_loop 内的
//! writer 切换逻辑。暂停/恢复/显式分段/同秒冲突 -2/-3/午夜轮转/空路径（不落盘，
//! CLI 缺省）语义逐字保留；writer 的切换只发生在持有它的读线程内。

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDate};
use parking_lot::RwLock;

use super::port::LogLine;
use crate::session::SessionLog;

/// 会话落盘录制控制器：持 writer 与「当前路径」共享单元（manager 的 SessionHandle
/// 也持同一 Arc——「打开日志」/REST 导出指向最新分段）。
pub struct RecordingController {
    writer: Option<SessionLog>,
    /// 分段命名的基准路径（连接时解析；分段始终基于它，避免 stem 越叠越长）。
    base_path: PathBuf,
    current_path: Arc<RwLock<PathBuf>>,
    midnight_rotate: bool,
    /// 当前分段所属的本地日期（午夜自动分段的比对基准）。
    last_date: NaiveDate,
}

/// 午夜轮转失败（携带候选路径与底层 io 错误——错误消息需含路径，与既有文案一致）。
#[derive(Debug)]
pub struct MidnightFailure {
    pub path: PathBuf,
    pub err: io::Error,
}

/// Debug 手工实现（SessionLog 无 Debug 派生；writer 只报在位与否）。
impl std::fmt::Debug for RecordingController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingController")
            .field("active", &self.writer.is_some())
            .field("base_path", &self.base_path)
            .field("current_path", &self.current_path)
            .field("midnight_rotate", &self.midnight_rotate)
            .field("last_date", &self.last_date)
            .finish()
    }
}

impl RecordingController {
    /// 打开录制：`enabled=false`（空路径，不落盘）等价 [`RecordingController::disabled`]；
    /// 否则创建基准路径文件（含父目录）。创建失败返回 Err，由调用方决定告警方式
    /// （连接不受影响，与既有 open_session_log 语义一致）。
    pub fn open(
        base_path: PathBuf,
        current_path: Arc<RwLock<PathBuf>>,
        enabled: bool,
        midnight_rotate: bool,
        now: DateTime<Local>,
    ) -> io::Result<Self> {
        let mut ctrl = Self::disabled(current_path, midnight_rotate, now);
        ctrl.base_path = base_path.clone();
        if !enabled {
            return Ok(ctrl);
        }
        ctrl.writer = Some(SessionLog::create(&base_path)?);
        Ok(ctrl)
    }

    /// 不落盘降级态（CLI 缺省 / 打开失败兜底）：writer 恒 None，写/轮转皆 no-op。
    pub fn disabled(
        current_path: Arc<RwLock<PathBuf>>,
        midnight_rotate: bool,
        now: DateTime<Local>,
    ) -> Self {
        Self {
            writer: None,
            base_path: PathBuf::new(),
            current_path,
            midnight_rotate,
            last_date: now.date_naive(),
        }
    }

    /// writer 是否在位（落盘中）。
    pub fn is_active(&self) -> bool {
        self.writer.is_some()
    }

    /// 追加一行 TSV（writer 不在位为 no-op；写错误按既有行为静默——数据流优先）。
    pub fn write(&mut self, line: &LogLine) -> io::Result<()> {
        if let Some(w) = self.writer.as_mut() {
            w.append(line);
        }
        Ok(())
    }

    /// 清屏截断当前文件（PortCmd::Clear；writer 不在位为 no-op，错误按既有行为静默）。
    pub fn clear(&mut self) -> io::Result<()> {
        if let Some(w) = self.writer.as_mut() {
            let _ = w.clear();
        }
        Ok(())
    }

    /// 分段/恢复录制（PortCmd::RecOn）：flush+关闭旧文件后另起新文件。
    /// 创建失败则停写（writer=None）并返回 Err（连接不受影响，下一条 RecOn 可再试）；
    /// 成功时更新「当前路径」共享单元。manager 侧「分段」命令发送前已同步更新该单元，
    /// 此处再写同值幂等；午夜轮转则依赖此处更新。
    pub fn rotate(&mut self, path: PathBuf) -> io::Result<()> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        match SessionLog::create(&path) {
            Ok(w) => {
                self.writer = Some(w);
                *self.current_path.write() = path;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// 暂停落盘（PortCmd::RecOff）：flush+关闭当前文件；数据仍进 ring/视图。
    pub fn pause(&mut self) -> io::Result<()> {
        if let Some(mut w) = self.writer.take() {
            let _ = w.flush();
        }
        Ok(())
    }

    /// 午夜自动分段（读线程内 1 秒节流调用）：开关启用且本地日期相对上次检查变更
    /// （跨天/时钟跳变）才成立；日期无条件推进，writer 不在位（录制关闭）不建文件。
    /// 成功返回新分段路径（「打开日志」/REST 导出随之指向最新分段）。
    pub fn tick_midnight(
        &mut self,
        now: DateTime<Local>,
    ) -> Result<Option<PathBuf>, MidnightFailure> {
        if !segment_due(self.midnight_rotate, self.last_date, now.date_naive()) {
            return Ok(None);
        }
        self.last_date = now.date_naive();
        if self.writer.is_none() {
            return Ok(None);
        }
        let np = next_segment_path(&self.base_path, &now, |p| p.exists());
        match self.rotate(np.clone()) {
            Ok(()) => Ok(Some(np)),
            Err(err) => Err(MidnightFailure { path: np, err }),
        }
    }

    /// 会话收尾 flush（finish_loop；错误按既有行为静默）。writer 保留（会话可能
    /// 尚未结束的其他 flush 场景也复用）。
    pub fn flush(&mut self) -> io::Result<()> {
        if let Some(w) = self.writer.as_mut() {
            let _ = w.flush();
        }
        Ok(())
    }
}

/// 默认日志路径：`sessions_dir/{id}.log`（桌面端传 app_data_dir()/sessions）。
pub fn default_log_path(sessions_dir: &Path, id: &str) -> PathBuf {
    sessions_dir.join(format!("{}.log", id))
}

/// 分段日志路径：在基准路径扩展名前插入 `-YYYYMMDD-HHMMSS`；同秒内再次分段
/// 用 `-2`/`-3` 递增去重。`exists` 由调用方注入（真实 fs / 测试桩），保持纯函数可测。
pub fn next_segment_path(
    base: &Path,
    now: &chrono::DateTime<Local>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let stamp = now.format("%Y%m%d-%H%M%S").to_string();
    let dir = base.parent().unwrap_or(Path::new(""));
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into());
    let ext = base.extension().map(|s| s.to_string_lossy().into_owned());
    let name = |n: Option<u32>| match (&ext, n) {
        (Some(e), Some(n)) => format!("{stem}-{stamp}-{n}.{e}"),
        (Some(e), None) => format!("{stem}-{stamp}.{e}"),
        (None, Some(n)) => format!("{stem}-{stamp}-{n}"),
        (None, None) => format!("{stem}-{stamp}"),
    };
    let mut cand = dir.join(name(None));
    let mut n = 2;
    while exists(&cand) {
        cand = dir.join(name(Some(n)));
        n += 1;
    }
    cand
}

/// 午夜分段判定：开关启用且本地日期相对上次检查已变更（跨天 / 时钟跳变）才成立。
fn segment_due(enabled: bool, last_date: NaiveDate, now_date: NaiveDate) -> bool {
    enabled && last_date != now_date
}

#[cfg(test)]
mod tests {
    //! 路径命名纯函数（自 manager 迁移的既有断言）+ 控制器行为（临时目录，不触业务目录）。
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bytetide-rec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("create temp dir");
        d
    }

    fn mk_line(text: &str) -> LogLine {
        LogLine {
            ts: "00:00:00.000".into(),
            dir: super::super::port::Dir::Rx,
            text: text.into(),
            bytes: None,
            epoch_millis: 0,
        }
    }

    fn read_file(p: &Path) -> String {
        std::fs::read_to_string(p).expect("read recorded file")
    }

    // ---------- 路径命名（原 manager 断言原样迁移） ----------

    #[test]
    fn segment_path_inserts_stamp_before_extension() {
        use chrono::TimeZone;
        let dt = chrono::Local
            .with_ymd_and_hms(2026, 9, 4, 15, 30, 12)
            .unwrap();
        // 默认命名 {id}.log：时间戳插在扩展名前
        let p = next_segment_path(Path::new("/data/sessions/s1.log"), &dt, |_| false);
        assert_eq!(p, PathBuf::from("/data/sessions/s1-20260904-153012.log"));
        // 无扩展名路径同样成立
        let p = next_segment_path(Path::new("/logs/COM3"), &dt, |_| false);
        assert_eq!(p, PathBuf::from("/logs/COM3-20260904-153012"));
    }

    #[test]
    fn segment_path_avoids_collision_with_counter_suffix() {
        use chrono::TimeZone;
        let dt = chrono::Local
            .with_ymd_and_hms(2026, 9, 4, 15, 30, 12)
            .unwrap();
        // 首个候选已存在（同秒内再次分段）：-2 递增去重
        let seen = std::cell::Cell::new(0);
        let p = next_segment_path(Path::new("/data/s1.log"), &dt, |_| {
            seen.set(seen.get() + 1);
            seen.get() == 1
        });
        assert_eq!(p, PathBuf::from("/data/s1-20260904-153012-2.log"));
    }

    #[test]
    fn segment_due_only_when_enabled_and_date_changed() {
        let d7 = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
        let d8 = NaiveDate::from_ymd_opt(2026, 9, 8).unwrap();
        assert!(!segment_due(false, d7, d8)); // 开关关闭永不轮转
        assert!(!segment_due(true, d7, d7)); // 同日不轮转
        assert!(segment_due(true, d7, d8)); // 跨天轮转
        assert!(segment_due(true, d8, d7)); // 时钟回拨视为变更（一次性轮转）
    }

    #[test]
    fn default_log_path_uses_session_id() {
        assert_eq!(
            default_log_path(Path::new("/data/sessions"), "s7"),
            PathBuf::from("/data/sessions/s7.log")
        );
    }

    // ---------- 控制器行为 ----------

    #[test]
    fn open_disabled_writes_nothing_and_creates_no_file() {
        let dir = temp_dir("disabled");
        let shared = Arc::new(RwLock::new(PathBuf::new()));
        let mut rec = RecordingController::open(
            dir.join("x.log"),
            shared.clone(),
            false,
            false,
            Local::now(),
        )
        .expect("disabled open");
        assert!(!rec.is_active());
        rec.write(&mk_line("hello")).expect("write no-op");
        assert_eq!(shared.read().clone(), PathBuf::new());
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert!(entries.is_empty(), "不落盘不得创建文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_creates_file_and_write_appends_tsv() {
        let dir = temp_dir("write");
        let path = dir.join("s1.log");
        let shared = Arc::new(RwLock::new(path.clone()));
        let mut rec =
            RecordingController::open(path.clone(), shared.clone(), true, false, Local::now())
                .expect("open");
        assert!(rec.is_active());
        rec.write(&mk_line("first")).expect("write");
        rec.write(&mk_line("second")).expect("write");
        rec.flush().expect("flush");
        let content = read_file(&path);
        assert_eq!(
            content,
            "00:00:00.000\tRX\tfirst\n00:00:00.000\tRX\tsecond\n"
        );
        assert_eq!(shared.read().clone(), path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_failure_propagates_io_error() {
        // 父“目录”实为一个文件：create_dir_all 必败
        let dir = temp_dir("fail");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("write blocker");
        let shared = Arc::new(RwLock::new(PathBuf::new()));
        let err = RecordingController::open(
            blocker.join("sub").join("s1.log"),
            shared,
            true,
            false,
            Local::now(),
        )
        .expect_err("父路径是文件时创建必须失败");
        // 错误文本与既有 open_session_log 透传的 io::Error 一致（仅透传，不加工）
        assert!(!err.to_string().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pause_stops_writing_and_rotate_resumes_into_new_file() {
        let dir = temp_dir("pause-rotate");
        let first = dir.join("a.log");
        let second = dir.join("b.log");
        let shared = Arc::new(RwLock::new(first.clone()));
        let mut rec =
            RecordingController::open(first.clone(), shared.clone(), true, false, Local::now())
                .expect("open");
        rec.write(&mk_line("before")).expect("write");
        // 暂停：文件关闭，后续写 no-op
        rec.pause().expect("pause");
        assert!(!rec.is_active());
        rec.write(&mk_line("dropped")).expect("write no-op");
        rec.flush().expect("flush");
        assert_eq!(read_file(&first), "00:00:00.000\tRX\tbefore\n");
        // 恢复/分段：另起新文件，当前路径单元随更新
        rec.rotate(second.clone()).expect("rotate");
        assert!(rec.is_active());
        assert_eq!(shared.read().clone(), second);
        rec.write(&mk_line("after")).expect("write");
        rec.flush().expect("flush");
        assert_eq!(read_file(&second), "00:00:00.000\tRX\tafter\n");
        // 旧文件不再增长
        assert_eq!(read_file(&first), "00:00:00.000\tRX\tbefore\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_truncates_current_file() {
        let dir = temp_dir("clear");
        let path = dir.join("s1.log");
        let mut rec = RecordingController::open(
            path.clone(),
            Arc::new(RwLock::new(path.clone())),
            true,
            false,
            Local::now(),
        )
        .expect("open");
        rec.write(&mk_line("x")).expect("write");
        rec.flush().expect("flush");
        assert!(!read_file(&path).is_empty());
        rec.clear().expect("clear");
        rec.flush().expect("flush");
        assert_eq!(read_file(&path), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_midnight_rotates_only_on_date_change_with_active_writer() {
        use chrono::TimeZone;
        let dir = temp_dir("midnight");
        let base = dir.join("s1.log");
        let shared = Arc::new(RwLock::new(base.clone()));
        let day1 = Local.with_ymd_and_hms(2026, 9, 7, 23, 59, 0).unwrap();
        let day2 = Local.with_ymd_and_hms(2026, 9, 8, 0, 0, 30).unwrap();
        let mut rec = RecordingController::open(base.clone(), shared.clone(), true, true, day1)
            .expect("open");
        rec.write(&mk_line("day1")).expect("write");

        // 同日 tick：不轮转
        assert_eq!(rec.tick_midnight(day1).expect("tick"), None);
        assert!(rec.is_active());

        // 跨天 tick：另起新分段并更新当前路径
        let np = rec.tick_midnight(day2).expect("tick").expect("应轮转");
        assert!(np
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("s1-20260908-000030"));
        assert_eq!(shared.read().clone(), np);
        assert!(rec.is_active());
        rec.write(&mk_line("day2")).expect("write");
        rec.flush().expect("flush");
        assert_eq!(read_file(&np), "00:00:00.000\tRX\tday2\n");

        // 跨天后同日再 tick：不重复轮转
        assert_eq!(rec.tick_midnight(day2).expect("tick"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_midnight_disabled_or_paused_never_creates_files() {
        use chrono::TimeZone;
        let dir = temp_dir("midnight-off");
        let base = dir.join("s1.log");
        let day1 = Local.with_ymd_and_hms(2026, 9, 7, 23, 0, 0).unwrap();
        let day2 = Local.with_ymd_and_hms(2026, 9, 8, 0, 0, 30).unwrap();
        // 开关关闭：日期变更也不轮转（open 建立的 s1.log 是目录里唯一文件）
        let shared = Arc::new(RwLock::new(base.clone()));
        let mut off = RecordingController::open(base.clone(), shared.clone(), true, false, day1)
            .expect("open");
        assert_eq!(off.tick_midnight(day2).expect("tick"), None);
        assert!(off.is_active());
        assert_eq!(shared.read().clone(), base, "未轮转时当前路径不变");
        // 开关开启但录制暂停（writer 不在位）：不建新文件
        let shared2 = Arc::new(RwLock::new(PathBuf::new()));
        let mut paused = RecordingController::disabled(shared2, true, day1);
        assert_eq!(paused.tick_midnight(day2).expect("tick"), None);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["s1.log".to_string()], "跨天不得新建分段文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tick_midnight_failure_carries_path_and_stops_recording() {
        use chrono::TimeZone;
        let dir = temp_dir("midnight-fail");
        let base = dir.join("s1.log");
        let day1 = Local.with_ymd_and_hms(2026, 9, 7, 23, 0, 0).unwrap();
        let day2 = Local.with_ymd_and_hms(2026, 9, 8, 0, 0, 30).unwrap();
        let mut rec = RecordingController::open(
            base.clone(),
            Arc::new(RwLock::new(base.clone())),
            true,
            true,
            day1,
        )
        .expect("open");
        // 让基准路径的父“目录”变成文件：新分段创建必败
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("write blocker");
        // 基准路径换成 blocker 下的子路径（controller 已持 writer，base 仅用于命名）
        rec.base_path = blocker.join("s2.log");
        let fail = rec.tick_midnight(day2).expect_err("应失败");
        assert!(fail
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("s2-20260908-"));
        assert!(!fail.err.to_string().is_empty());
        // 创建失败后停写（与既有 RecOn 失败语义一致）
        assert!(!rec.is_active());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
