//! 离线日志流式分页（Stage 2 Task 8）：稀疏索引（每 [`PAGE_LINES`] 个数据行记一个
//! 文件字节偏移锚点）+ 按页读取，替代「前端全量解析 → create_offline_session_cmd
//! 整包灌 ring」的旧路径（旧命令保留一个发布周期，见 `PortManager::load_offline`）。
//!
//! - [`index`]：[`open_offline`] 一次 `BufRead` 顺序扫全文件建索引——内存只持
//!   锚点表（≈行数/4096 个 u64）+ 单行缓冲，不持行、不灌 ring。
//! - [`reader`]：[`OfflineReader`] 按页读（`read_page(after,max)` 返回 `no>after`
//!   的前 max 行）+ manager 查询路由用的虚拟 ring 视图（lines_after/lines_before/
//!   snapshot/follow/bounds/stats，行 no 语义与 `RingBuf::push` 一致：按数据行顺序
//!   连续分配、首行 no=1）。
//!
//! 解析语义与前端 `useLogParser.parseLogFile` 逐条对齐（差异见各函数注释）：
//! TSV `ts\tdir\traw` 三列（text 内可含 tab，只按前两个 tab 切）；`#` 头注释与
//! 空行跳过；无 tab 坏行计错误不占行号；dir 非 `TX`（trim+忽略大小写）一律 rx；
//! epoch 由 ts 解析为当日毫秒，解析失败回退行序（保单调）。

pub mod index;
pub mod reader;

pub use index::{open_offline, OfflineIndex};
pub use reader::OfflineReader;

use crate::serial::port::{Dir, LogLine};

/// 稀疏索引步长：每 4096 个数据行记录一个锚点。页读从锚点 seek 后最多跳扫
/// 4095 行，单页内存上限=调用方 max（manager 钳到 RING_CAP）。
pub(crate) const PAGE_LINES: u64 = 4096;

/// 离线日志打开/读取错误。
#[derive(Debug)]
pub enum OfflineError {
    /// 文件打开 / seek / 读 IO 错误（索引期与页读期共用）。
    Io(std::io::Error),
}

impl std::fmt::Display for OfflineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OfflineError::Io(e) => write!(f, "offline log io error: {e}"),
        }
    }
}

impl std::error::Error for OfflineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OfflineError::Io(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for OfflineError {
    fn from(e: std::io::Error) -> Self {
        OfflineError::Io(e)
    }
}

/// 行分类：与前端 parseLogFile 的「跳过 / errors 计数 / 数据行」三分一致。
pub(crate) enum RawRow {
    /// 空行或 `#` 头注释（现场档案元信息）：跳过、不计错误、不占行号。
    Skip,
    /// 无 tab 的坏行：不计行号、不产出（前端计入 errors）。
    Bad,
    /// 数据行（结构上必然可切出 ts/dir/text——ts 非法时 epoch 走行序回退）。
    Row(LogLine),
}

/// 行尾剥离：只剥 `\n` 及其紧邻的 `\r`（等价前端 `split(/\r?\n/)`；
/// 文件末尾孤立的 `\r` 不剥，与前端切分结果一致）。
pub(crate) fn strip_eol(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    if end > 0 && raw[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && raw[end - 1] == b'\r' {
            end -= 1;
        }
    }
    &raw[..end]
}

/// 廉价预筛（跳扫用）：是否数据行（非空、非 `#`、含 tab）——与
/// `matches!(classify_line(..), RawRow::Row(_))` 严格等价，但不做解析分配。
pub(crate) fn is_data_line(raw: &[u8]) -> bool {
    let line = strip_eol(raw);
    !line.is_empty() && line[0] != b'#' && line.contains(&b'\t')
}

/// 解析一行原始字节。`seq` = 本行之前的累计数据行数（epoch 回退值，对齐前端
/// `epochMillis = ms >= 0 ? ms : seq`）。
///
/// 与前端的唯一语义差异（报告项）：非法 UTF-8 时前端对**整文件** lossy 后无法
/// 区分列，这里按**text 列原始字节**判定——仅 text 列非法时 `bytes=Some(text 列
/// 原始字节)`（保证 `lossy(bytes)==text` 的既有不变式，HEX 直发语义与实时行
/// `LogLine.bytes` 一致）；非法字节仅在 ts/dir 列时 text 列仍无损，bytes=None。
pub(crate) fn classify_line(raw: &[u8], seq: u64) -> RawRow {
    let line = strip_eol(raw);
    if line.is_empty() || line[0] == b'#' {
        return RawRow::Skip;
    }
    let Some(first) = line.iter().position(|&b| b == b'\t') else {
        return RawRow::Bad;
    };
    let (ts_b, rest) = line.split_at(first);
    let rest = &rest[1..];
    // 只按前两个 tab 切：text 内可含 tab（对齐前端 indexOf 两段切分）
    let (dir_b, text_b) = match rest.iter().position(|&b| b == b'\t') {
        Some(second) => {
            let (d, t) = rest.split_at(second);
            (d, &t[1..])
        }
        // 两列行（ts\tdir）：text 为空（前端 slice 到行尾得 ''，一致）
        None => (rest, &[][..]),
    };
    let ts = String::from_utf8_lossy(ts_b).into_owned();
    // dir：trim + 忽略 ASCII 大小写后等于 TX 才是 tx，其余一律 rx（对齐前端
    // dirStr.trim().toUpperCase() === 'TX'；Unicode 大小写边角差异不涉及 RX/TX）
    let dir = {
        let d = String::from_utf8_lossy(dir_b);
        if d.trim().eq_ignore_ascii_case("tx") {
            Dir::Tx
        } else {
            Dir::Rx
        }
    };
    let (text, bytes) = match std::str::from_utf8(text_b) {
        Ok(s) => (s.to_owned(), None),
        Err(_) => (
            String::from_utf8_lossy(text_b).into_owned(),
            Some(text_b.to_vec()),
        ),
    };
    let epoch_millis = parse_ts_ms(&ts).unwrap_or(seq);
    RawRow::Row(LogLine {
        ts,
        dir,
        text,
        bytes,
        epoch_millis,
    })
}

/// 行时间戳 → 当日毫秒（对齐前端 `parseTsToMs`）：`^(\d{1,2}):(\d{2}):(\d{2})
/// (?:\.(\d{1,3}))?$`（trim 后匹配，ASCII 数字），h≤23、mi≤59、s≤59；
/// 小数按 `padEnd(3,'0')` 右补零（`.5`=500ms、`.05`=50ms）。
pub(crate) fn parse_ts_ms(ts: &str) -> Option<u64> {
    let b = ts.trim().as_bytes();
    // 取一段 ASCII 数字（min..=max 位），返回 (值, 消耗字节数)
    fn digits(seg: &[u8], min: usize, max: usize) -> Option<(u32, usize)> {
        let mut n = 0;
        while n < seg.len() && n < max && seg[n].is_ascii_digit() {
            n += 1;
        }
        if n < min {
            return None;
        }
        Some((std::str::from_utf8(&seg[..n]).ok()?.parse().ok()?, n))
    }
    let (h, n1) = digits(b, 1, 2)?;
    if b.get(n1) != Some(&b':') {
        return None;
    }
    let (mi, n2) = digits(&b[n1 + 1..], 2, 2)?;
    let after_mi = n1 + 1 + n2;
    if b.get(after_mi) != Some(&b':') {
        return None;
    }
    let (s, n3) = digits(&b[after_mi + 1..], 2, 2)?;
    let after_s = after_mi + 1 + n3;
    let ms = if after_s == b.len() {
        0
    } else {
        if b[after_s] != b'.' {
            return None;
        }
        let (v, n4) = digits(&b[after_s + 1..], 1, 3)?;
        if after_s + 1 + n4 != b.len() {
            return None;
        }
        v * 10u32.pow(3 - n4 as u32)
    };
    if h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(u64::from(h) * 3_600_000 + u64::from(mi) * 60_000 + u64::from(s) * 1_000 + u64::from(ms))
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 单测共享：临时目录 + 确定性日志文件生成（测试运行时落盘，不提交二进制）。
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 共享黄金样本（仓库 `testdata/protocol/tsv-v1.log`，TS 侧
    /// `useLogParser.test.ts` 同源消费）：`#` 注释头、坏行、非法 dir、tab text、
    /// epoch 回退、lossy U+FFFD。仓库 `.gitattributes eol=lf` 规范化，拿到的是
    /// LF 版本（CRLF 语义由消费方运行时转换验证）。
    pub(crate) const TSV_FIXTURE: &str = include_str!("../../../../testdata/protocol/tsv-v1.log");

    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// 第 i 行的确定性 ts/epoch（当日毫秒 = i mod 86400000）。
    pub fn ts_of(i: u64) -> String {
        let d = i % 86_400_000;
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            d / 3_600_000,
            d / 60_000 % 60,
            d / 1_000 % 60,
            d % 1_000
        )
    }

    /// 唯一临时目录（进程 id + 纳秒 + 计数）。
    pub fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "bytetide-offline-{tag}-{}-{}-{}",
            std::process::id(),
            nanos,
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 写一个纯 LF 文件：每行 `{ts_of(i)}\t{dir}\t{text}`。
    pub fn write_lines(dir: &std::path::Path, name: &str, n: u64) -> PathBuf {
        let path = dir.join(name);
        let mut body = String::with_capacity(n as usize * 32);
        for i in 0..n {
            let dir_s = if i % 3 == 2 { "TX" } else { "RX" };
            body.push_str(&format!("{}\t{}\tline-{}\n", ts_of(i), dir_s, i));
        }
        std::fs::write(&path, body).expect("write log");
        path
    }
}

#[cfg(test)]
mod tests {
    //! 行解析单测：与前端 parseLogFile/parseTsToMs 语义逐条对齐。
    use super::*;

    fn row(raw: &[u8]) -> LogLine {
        match classify_line(raw, 0) {
            RawRow::Row(l) => l,
            _ => panic!("expected data row"),
        }
    }

    #[test]
    fn classify_skips_empty_comment_and_flags_bad() {
        for skip in ["\n", "\r\n", "", "# capture header", "#ts\tdir\ttext"] {
            assert!(
                matches!(classify_line(skip.as_bytes(), 0), RawRow::Skip),
                "{skip:?}"
            );
        }
        assert!(matches!(classify_line(b"no tab here", 0), RawRow::Bad));
        // 前导空格的注释不算注释 → 坏行（与前端 startsWith('#') 一致）
        assert!(matches!(classify_line(b"  # x", 0), RawRow::Bad));
    }

    #[test]
    fn classify_two_and_three_columns() {
        // 两列行：text 为空
        let l = row(b"00:00:01.000\tRX");
        assert_eq!(
            (l.ts.as_str(), l.dir, l.text.as_str()),
            ("00:00:01.000", Dir::Rx, "")
        );
        // text 内含 tab：只按前两个 tab 切
        let l = row(b"01:02:03.004\tTX\ta\tb\tc");
        assert_eq!(l.text, "a\tb\tc");
        assert_eq!(l.dir, Dir::Tx);
        assert_eq!(l.epoch_millis, 3_723_004);
    }

    #[test]
    fn classify_dir_trim_and_case() {
        assert_eq!(row(b"t\ttx").dir, Dir::Tx);
        assert_eq!(row(b"t\tTx").dir, Dir::Tx);
        assert_eq!(row(b"t\t TX ").dir, Dir::Tx);
        assert_eq!(row(b"t\trx").dir, Dir::Rx);
        assert_eq!(row(b"t\tgarbage").dir, Dir::Rx);
        assert_eq!(row(b"t\t").dir, Dir::Rx);
    }

    #[test]
    fn parse_ts_ms_matches_frontend() {
        assert_eq!(parse_ts_ms("00:00:00.001"), Some(1));
        assert_eq!(parse_ts_ms("12:34:56.789"), Some(45_296_789));
        assert_eq!(parse_ts_ms("23:59:59.999"), Some(86_399_999));
        // 小数右补零（padEnd(3,'0')）：.5=500ms、.05=50ms、.055=55ms
        assert_eq!(parse_ts_ms("00:00:00.5"), Some(500));
        assert_eq!(parse_ts_ms("00:00:00.05"), Some(50));
        assert_eq!(parse_ts_ms("00:00:00.055"), Some(55));
        // 小时 1-2 位；trim 后匹配
        assert_eq!(parse_ts_ms("1:02:03"), Some(3_723_000));
        assert_eq!(parse_ts_ms("  01:02:03.004  "), Some(3_723_004));
        assert_eq!(parse_ts_ms("00:00:00.000"), Some(0));
        // 非法：越界/缺位/超长小数/杂字符
        for bad in [
            "24:00:00",
            "12:60:00",
            "12:00:60",
            "1:2:3",
            "12:34:5",
            "12:34:56.0555",
            "12:34:56.",
            "aa:bb:cc",
            "12-34-56",
            "123:00:00",
            "",
        ] {
            assert_eq!(parse_ts_ms(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn classify_epoch_falls_back_to_seq() {
        // ts 非法 → epoch = seq（本行之前的数据行数），保持单调（对齐前端）
        assert!(matches!(
            classify_line(b"bad\tRX\tx", 7),
            RawRow::Row(LogLine {
                epoch_millis: 7,
                ..
            })
        ));
        // ts 合法但为 0 也走解析值而非回退
        assert!(matches!(
            classify_line(b"00:00:00.000\tRX\tx", 9),
            RawRow::Row(LogLine {
                epoch_millis: 0,
                ..
            })
        ));
    }

    #[test]
    fn classify_invalid_utf8_carries_text_column_bytes() {
        // text 列非法：lossy 文本 + 原始字节（LogLine.bytes 仅非法时携带的既有语义）
        let l = row(b"00:00:01.000\tRX\tok\xe4\xbd\xa0\xff");
        assert!(l.text.contains('\u{FFFD}'));
        assert_eq!(l.bytes.as_deref(), Some(&b"ok\xe4\xbd\xa0\xff"[..]));
        // ts 列非法：text 列仍无损 → bytes=None（差异项：按 text 列判定，见 classify_line 注释）
        let l = row(b"\xff\xfe\tRX\tclean");
        assert_eq!(l.text, "clean");
        assert!(l.bytes.is_none());
        // 纯 ASCII：bytes=None
        assert!(row(b"00:00:01.000\tRX\thello").bytes.is_none());
    }

    #[test]
    fn strip_eol_only_drops_cr_before_lf() {
        assert_eq!(strip_eol(b"a\n"), b"a");
        assert_eq!(strip_eol(b"a\r\n"), b"a");
        assert_eq!(strip_eol(b"a\r"), b"a\r"); // 末尾孤立 \r 不剥（对齐前端切分）
        assert_eq!(strip_eol(b"a\r\r\n"), b"a\r");
        assert_eq!(strip_eol(b"a"), b"a");
        assert_eq!(strip_eol(b""), b"");
    }

    #[test]
    fn is_data_line_matches_row_classification() {
        for (raw, want) in [
            (&b"ts\trx\tx\n"[..], true),
            (&b"ts\trx"[..], true),
            (&b"\n"[..], false),
            (&b"# c\n"[..], false),
            (&b"no tab\n"[..], false),
            (&b"\r\n"[..], false),
        ] {
            assert_eq!(
                is_data_line(raw),
                want,
                "{:?} should be {want}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    // ===== 共享黄金样本（testdata/protocol/tsv-v1.log，TS useLogParser.test.ts 同源）=====

    #[test]
    fn golden_fixture_tsv_v1_classifies_like_frontend() {
        let mut rows: Vec<LogLine> = Vec::new();
        let mut bad = 0usize;
        for raw in test_support::TSV_FIXTURE.lines() {
            match classify_line(raw.as_bytes(), rows.len() as u64) {
                RawRow::Skip => {}
                RawRow::Bad => bad += 1,
                RawRow::Row(l) => rows.push(l),
            }
        }
        // 与前端 golden 断言同源：9 数据行、仅无 tab 行计坏行（# 注释/空行跳过）
        assert_eq!(rows.len(), 9, "fixture data rows: {rows:?}");
        assert_eq!(bad, 1, "only the no-tab line is bad");
        // dir 归一：rx/TX/小写 rx/「 TX 」/其余一律按前端 trim+大小写不敏感规则
        let dirs: Vec<Dir> = rows.iter().map(|l| l.dir).collect();
        assert_eq!(
            dirs,
            [
                Dir::Rx,
                Dir::Tx,
                Dir::Rx,
                Dir::Tx,
                Dir::Rx,
                Dir::Rx,
                Dir::Rx,
                Dir::Rx,
                Dir::Rx
            ],
            "dirs in fixture order"
        );
        // epoch：合法 ts 逐行解析成当日毫秒；garbage-ts 回退行序 seq=5（此前 5 个数据行）
        let epochs: Vec<u64> = rows.iter().map(|l| l.epoch_millis).collect();
        assert_eq!(epochs, [1000, 2000, 3500, 4000, 5123, 5, 7000, 8000, 8250]);
        // ts 字段保留原文（回退而非丢弃）
        assert_eq!(rows[5].ts, "garbage-ts");
        // 仅按前两个 tab 切：text 可含 tab
        assert_eq!(rows[4].text, "text may contain\ta tab here");
        // 二进制 lossy 行：有损 TSV 落盘即 U+FFFD 文本（fixture 内是合法 UTF-8 的
        // 替换符），原始字节不落盘 → bytes=None；Rust「仅 text 列原始非法时
        // bytes=Some」语义由 classify_invalid_utf8_carries_text_column_bytes 覆盖
        assert_eq!(rows[6].text, "binary lossy: \u{FFFD}\u{FFFD} OK\u{FFFD}");
        assert!(rows[6].bytes.is_none());
    }

    // ===== manager 接入端到端（load_offline_indexed 的虚拟 ring 查询路由）=====

    use std::path::PathBuf;

    use super::test_support::{temp_dir, write_lines};
    use crate::serial::manager::{PortManager, SendMode, SendRequest};
    use crate::serial::port::PortConfig;

    #[test]
    fn load_offline_indexed_pages_as_virtual_ring() {
        let m = PortManager::new();
        let dir = temp_dir("mgr-indexed");
        let path = write_lines(&dir, "lines.log", 12);
        let (id, index) = m
            .load_offline_indexed(PortConfig::default(), path.clone())
            .expect("open indexed");
        assert_eq!(index.line_count, 12);
        // ring 恒空：查询走页读；no 连续 1..=12
        let p1 = m.ring_lines_after_no(&id, 0, 5).unwrap();
        assert_eq!(
            p1.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(p1[0].text, "line-0");
        let p2 = m.ring_lines_after_no(&id, 5, 5).unwrap();
        assert_eq!(
            p2.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![6, 7, 8, 9, 10]
        );
        // 往前翻页（升序、最新窗口）
        let back = m.ring_lines_before_no(&id, 5, 3).unwrap();
        assert_eq!(back.iter().map(|l| l.no).collect::<Vec<_>>(), vec![2, 3, 4]);
        // 边界/统计/列表
        let b = m.ring_bounds(&id).unwrap();
        assert_eq!((b.first_no, b.last_no, b.size), (1, 12, 12));
        assert_eq!(m.bridge_last_no(&id), Some(12));
        let stats = m.bridge_stats(&id).unwrap();
        assert_eq!((stats.rx_lines, stats.tx_lines, stats.size), (8, 4, 12));
        assert_eq!((stats.first_no, stats.last_no), (1, 12));
        let snap = m.bridge_list().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!((snap.status.as_str(), snap.line_count), ("offline", 12));
        // log_path 指向源文件（「打开日志」/导出语义）
        assert_eq!(m.session_log_path(&id).unwrap(), path.to_string_lossy());
        // 全量快照=分页走完
        assert_eq!(m.bridge_snapshot(&id).unwrap().len(), 12);
        // 守卫：发送/分段拒绝（离线会话无链路）
        assert!(m
            .send(
                &id,
                SendRequest {
                    mode: SendMode::Ascii,
                    text: "x".into()
                }
            )
            .is_err());
        assert!(m.rotate_log(&id).is_err());
        // 清屏=虚拟镜像遗忘（计数器与文件事实保留）
        m.clear_log(&id).unwrap();
        assert!(m.ring_lines_after_no(&id, 0, 10).unwrap().is_empty());
        assert_eq!(m.bridge_last_no(&id), Some(0));
        let b = m.ring_bounds(&id).unwrap();
        assert_eq!((b.first_no, b.last_no, b.size), (0, 0, 0));
        // 断开即清理
        m.disconnect(&id).unwrap();
        assert!(m.ring_lines_after_no(&id, 0, 10).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_offline_indexed_missing_file_errors_and_old_path_intact() {
        let m = PortManager::new();
        let dir = temp_dir("mgr-missing");
        assert!(m
            .load_offline_indexed(PortConfig::default(), dir.join("nope.log"))
            .is_err());
        // 旧全量路径不受影响：行为照旧（ring 有行、游标可用）
        let id = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
        assert!(m.ring_lines_after_no(&id, 0, 10).unwrap().is_empty());
        m.clear_log(&id).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
