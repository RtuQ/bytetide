//! 离线日志分页读取器：[`OfflineReader::read_page`] 是纯分页原语（`no>after`
//! 的前 max 行，升序），跨页经稀疏锚点 seek（最多回扫 PAGE_LINES-1 行），
//! 顺序连读命中游标时零回扫。其上的 `lines_after/lines_before/snapshot/follow/
//! bounds/dir_counters` 是 manager 查询路由用的「虚拟 ring」视图——行 no 语义与
//! `RingBuf::push` 一致（数据行按序连续分配、首行 no=1、清屏不回退游标），
//! 但行不进 ring：每次查询按页直读源文件。
//!
//! 锁形态：SessionHandle 持 `Arc<Mutex<OfflineReader>>`（parking_lot），查询时
//! 短临界区按页读；每次 `read_page` 独立 `File::open+seek`，不跨调用持文件句柄。
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

use super::index::OfflineIndex;
use super::{
    classify_line, is_data_line, parse_ts_ms, strip_eol, Anchor, DayWrap, OfflineError, RawRow,
    PAGE_LINES,
};
use crate::serial::port::LogLine;
use crate::serial::ring::BridgeLine;

/// 离线日志分页读取器（索引产物 + 页读游标）。`no` = 文件内第 N 个数据行
/// （空行/`#`注释/坏行不占号），首行 no=1、末行 no=line_count，与现有离线
/// 会话经 `RingBuf::push` 分配的 no 完全一致。
pub struct OfflineReader {
    path: PathBuf,
    /// 稀疏索引：anchors[k] = 数据行 no=k*PAGE_LINES+1 的字节偏移 + 回卷状态
    /// 快照。len = ceil(line_count/PAGE_LINES)（空文件为 0，页读不会用到）。
    anchors: Vec<Anchor>,
    line_count: u64,
    first_epoch: u64,
    last_epoch: u64,
    first_ts: String,
    last_ts: String,
    rx_lines: u64,
    tx_lines: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    bad_rows: u64,
    /// 清屏水位（对齐 RingBuf::clear：ring 清空、seq 不回退、计数器保留）。
    cleared: bool,
    /// 顺序读游标：cur_next_no=下一个待读数据行 no，cur_off=其字节偏移
    /// （None=无效，需走锚点回退）；回卷快照必须与该字节偏移同步保存。
    /// 仅是连读加速，任何失效都安全回退。
    cur_next_no: u64,
    cur_off: Option<u64>,
    cur_day_off: u64,
    cur_prev_raw: Option<u64>,
}

impl OfflineReader {
    /// 索引期装配（[`super::open_offline`] 调用；聚合值一次扫描累计，页读零重扫）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        path: PathBuf,
        anchors: Vec<Anchor>,
        index: OfflineIndex,
        first_ts: String,
        last_ts: String,
        rx_lines: u64,
        tx_lines: u64,
        rx_bytes: u64,
        tx_bytes: u64,
        bad_rows: u64,
    ) -> Self {
        Self {
            path,
            anchors,
            line_count: index.line_count,
            first_epoch: index.first_epoch,
            last_epoch: index.last_epoch,
            first_ts,
            last_ts,
            rx_lines,
            tx_lines,
            rx_bytes,
            tx_bytes,
            bad_rows,
            cleared: false,
            cur_next_no: 0,
            cur_off: None,
            cur_day_off: 0,
            cur_prev_raw: None,
        }
    }

    // ============================ 纯分页原语 ============================

    /// 返回 `no > after` 的前 max 行（升序）。no 连续 ⇒ 第 i 行 no=after+1+i；
    /// `after≥line_count`/`max=0`/已清屏 ⇒ 空。跨页从锚点 seek 后最多跳扫
    /// PAGE_LINES-1 行（非数据行不占号）；顺序连读命中游标则零跳扫。
    pub fn read_page(&mut self, after: u64, max: usize) -> Result<Vec<LogLine>, OfflineError> {
        if self.cleared || max == 0 {
            return Ok(Vec::new());
        }
        let want_start = after.saturating_add(1);
        if want_start > self.line_count {
            return Ok(Vec::new());
        }
        let count = ((self.line_count - want_start + 1) as usize).min(max);
        // 定位：顺序游标命中直接续读，否则锚点回退（k*PAGE_LINES+1 ≤ want_start）
        let (start_no, start_off, day_off, prev_raw) = if self.cur_next_no == want_start {
            match self.cur_off {
                Some(off) => {
                    // 游标连读：文件偏移与回卷状态必须来自同一个页尾快照。
                    (want_start, off, self.cur_day_off, self.cur_prev_raw)
                }
                None => self.anchor_state(want_start),
            }
        } else {
            self.anchor_state(want_start)
        };
        // 页内回卷状态机从锚点快照续接：任意页读顺序产出的调整后 epoch
        // 与索引期顺序扫描逐行一致（评审 P3 跨午夜补偿）
        let mut wrap = DayWrap::resume(day_off, prev_raw);
        let mut file = std::fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(start_off))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        let mut consumed: u64 = 0;
        let mut no = start_no;
        // 跳扫到首个目标行：非数据行不占号，只有数据行推进 no。跳过的数据行
        // 也要喂回卷状态机（ts 列廉价探测，不构造行对象）——否则目标行的回卷
        // 判定丢失锚点行之后的 prev，与索引期顺序扫描结果不一致
        while no < want_start {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                // 文件比索引期短（被截断）：无行可返
                self.cur_next_no = no;
                self.cur_off = None;
                return Ok(Vec::new());
            }
            consumed += n as u64;
            if is_data_line(&buf) {
                if let Some((raw_epoch, is_valid)) = peek_epoch(&buf, no - 1) {
                    wrap.feed(raw_epoch, is_valid);
                }
                no += 1;
            }
        }
        let mut out: Vec<LogLine> = Vec::with_capacity(count);
        // seq = 本行之前的数据行数（epoch 回退值，与索引期口径一致）
        let mut seq = want_start - 1;
        while out.len() < count {
            buf.clear();
            let n = reader.read_until(b'\n', &mut buf)?;
            if n == 0 {
                break; // 截断：返回已收部分
            }
            consumed += n as u64;
            if let RawRow::Row(mut line) = classify_line(&buf, seq) {
                // epoch==seq = 行序回退行（ts 非法）：不参与回卷检测，仅叠加偏移
                let is_valid_ts = line.epoch_millis != seq;
                let day_off = wrap.feed(line.epoch_millis, is_valid_ts);
                line.epoch_millis = line.epoch_millis.saturating_add(day_off);
                out.push(line);
                seq += 1;
            }
        }
        self.cur_next_no = seq + 1;
        self.cur_off = Some(start_off + consumed);
        (self.cur_day_off, self.cur_prev_raw) = wrap.snapshot();
        Ok(out)
    }

    // ===================== manager 查询路由：虚拟 ring 视图 =====================

    /// 游标拉取（镜像 `RingBuf::lines_after_no`）：`no > since_no` 的最旧 max 行。
    pub fn lines_after(
        &mut self,
        since_no: u64,
        max: usize,
    ) -> Result<Vec<BridgeLine>, OfflineError> {
        if self.cleared {
            return Ok(Vec::new());
        }
        let page = self.read_page(since_no, max)?;
        Ok(page
            .into_iter()
            .enumerate()
            .map(|(i, l)| to_bridge(since_no.saturating_add(1 + i as u64), l))
            .collect())
    }

    /// 往前翻页（镜像 `RingBuf::lines_before_no`）：`no < before_no` 的最新 max 行
    /// （升序）。`before_no=0` 或已清屏 ⇒ 空；超前取最新 max 行。
    pub fn lines_before(
        &mut self,
        before_no: u64,
        max: usize,
    ) -> Result<Vec<BridgeLine>, OfflineError> {
        if self.cleared || max == 0 {
            return Ok(Vec::new());
        }
        let last = before_no.saturating_sub(1).min(self.line_count);
        if last == 0 {
            return Ok(Vec::new());
        }
        let count = (last as usize).min(max);
        let start = last - count as u64 + 1;
        let page = self.read_page(start - 1, count)?;
        Ok(page
            .into_iter()
            .enumerate()
            .map(|(i, l)| to_bridge(start.saturating_add(i as u64), l))
            .collect())
    }

    /// 全量快照（镜像 `RingBuf::snapshot`）：分页走完整个文件。仅 REST 显式调用
    /// （前端拉取走 lines_after 游标）；大文件快照量级=文件行数，调用方自负。
    pub fn snapshot(&mut self) -> Result<Vec<BridgeLine>, OfflineError> {
        let mut out: Vec<BridgeLine> = Vec::new();
        if self.cleared {
            return Ok(out);
        }
        let mut cursor: u64 = 0;
        loop {
            let page = self.read_page(cursor, PAGE_LINES as usize)?;
            if page.is_empty() {
                return Ok(out);
            }
            let first_no = cursor.saturating_add(1);
            cursor = cursor.saturating_add(page.len() as u64);
            out.extend(
                page.into_iter()
                    .enumerate()
                    .map(|(i, l)| to_bridge(first_no.saturating_add(i as u64), l)),
            );
        }
    }

    /// 长轮询基线（镜像 `RingBuf::lines_since`+`last_no` 组合）：`no > since` 的
    /// 全部行 + 当前 lastNo。io 失败由调用方退空（Option 签名无错误通道）。
    pub fn follow(&mut self, since: u64) -> Result<(Vec<BridgeLine>, u64), OfflineError> {
        if self.cleared {
            return Ok((Vec::new(), 0));
        }
        let lc = self.line_count;
        if since >= lc {
            return Ok((Vec::new(), lc));
        }
        let count = (lc - since) as usize;
        let page = self.read_page(since, count)?;
        let first_no = since.saturating_add(1);
        let lines = page
            .into_iter()
            .enumerate()
            .map(|(i, l)| to_bridge(first_no.saturating_add(i as u64), l))
            .collect();
        Ok((lines, lc))
    }

    // ============================== 元信息访问器 ==============================

    /// 索引期数据行数（清屏不改变文件事实，查询面用 [`Self::bounds`] 的 size）。
    pub fn line_count(&self) -> u64 {
        self.line_count
    }

    /// 锚点数 = ceil(line_count/PAGE_LINES)：内存等价探针用（索引期持久内存
    /// ≈ anchors.len()×8B + 本结构固定字段，页读另持单页行）。
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }

    /// 坏行数（无 tab 行；对齐前端 parseLogFile 的 errors 计数）。
    pub fn error_count(&self) -> u64 {
        self.bad_rows
    }

    /// 虚拟 ring bounds（镜像 `RingBuf::bounds` 七元组；清屏后全 0）。
    pub fn bounds(&self) -> (u64, u64, String, String, u64, u64, usize) {
        if self.cleared || self.line_count == 0 {
            return (0, 0, String::new(), String::new(), 0, 0, 0);
        }
        (
            1,
            self.line_count,
            self.first_ts.clone(),
            self.last_ts.clone(),
            self.first_epoch,
            self.last_epoch,
            usize::try_from(self.line_count).unwrap_or(usize::MAX),
        )
    }

    /// 方向计数（镜像 `RingBuf` 计数器：清屏不重置）。
    pub fn dir_counters(&self) -> (u64, u64, u64, u64) {
        (self.rx_lines, self.tx_lines, self.rx_bytes, self.tx_bytes)
    }

    /// 当前末行 no（镜像 `RingBuf::last_no`：清屏后 0）。
    pub fn last_no(&self) -> u64 {
        if self.cleared {
            0
        } else {
            self.line_count
        }
    }

    /// 现存行数（镜像 `ring.len()`：清屏后 0）。
    pub fn size(&self) -> usize {
        if self.cleared {
            0
        } else {
            usize::try_from(self.line_count).unwrap_or(usize::MAX)
        }
    }

    /// 首末行时间戳（索引期累计；`bounds` 供 stats 组装，此处供测试）。
    pub fn ts_bounds(&self) -> (String, String) {
        (self.first_ts.clone(), self.last_ts.clone())
    }

    /// 清屏（镜像 `RingBuf::clear`）：后续查询全空、游标/计数语义不变。
    /// 离线源文件是静态快照，「清屏」= 后端镜像遗忘（前端视图自理），不可恢复。
    pub fn clear(&mut self) {
        self.cleared = true;
        self.cur_off = None;
        self.cur_next_no = 0;
        self.cur_day_off = 0;
        self.cur_prev_raw = None;
    }
}

/// 跳扫行的廉价 epoch 探测：仅取 ts 列（首个 tab 前）解析当日毫秒，不构造
/// 行对象（跳扫热路径避免 ts/dir/text 三份 String 分配）。返回 (epoch, 是否
/// 合法 ts)；非数据行返回 None（不参与回卷检测）。
fn peek_epoch(raw: &[u8], seq: u64) -> Option<(u64, bool)> {
    let line = strip_eol(raw);
    if line.is_empty() || line[0] == b'#' {
        return None;
    }
    let first = line.iter().position(|&b| b == b'\t')?;
    let ts = std::str::from_utf8(&line[..first]).ok()?;
    match parse_ts_ms(ts) {
        Some(ms) => Some((ms, true)),
        None => Some((seq, false)),
    }
}

/// 锚点定位：数据行 want_start 所在块的锚点状态（k*PAGE_LINES+1 ≤ want_start）。
/// 返回 (锚点行 no, 字节偏移, 日期偏移, 前一合法行原始 epoch)。
fn anchor_state(want_start: u64, anchors: &[Anchor]) -> (u64, u64, u64, Option<u64>) {
    let k = ((want_start - 1) / PAGE_LINES) as usize;
    let a = &anchors[k];
    (k as u64 * PAGE_LINES + 1, a.off, a.day_off, a.prev_raw)
}

impl OfflineReader {
    fn anchor_state(&self, want_start: u64) -> (u64, u64, u64, Option<u64>) {
        anchor_state(want_start, &self.anchors)
    }
}

fn to_bridge(no: u64, l: LogLine) -> BridgeLine {
    BridgeLine {
        no,
        ts: l.ts,
        dir: l.dir,
        text: l.text,
        bytes: l.bytes,
        epoch_millis: l.epoch_millis,
        r#match: None,
    }
}

#[cfg(test)]
mod tests {
    //! 页读单测：游标连读、锚点跨页、随机回跳、非数据行不占号、
    //! before/snapshot/follow 的 RingBuf 语义镜像、清屏、非法 UTF-8。
    use crate::offline::test_support::{temp_dir, ts_of, write_lines};
    use crate::serial::port::Dir;

    use super::*;

    fn open(n: u64) -> (TempDirGuard, OfflineReader, u64) {
        let dir = temp_dir("reader");
        let path = write_lines(&dir, "lines.log", n);
        let (_, r) = super::super::open_offline(&path).unwrap();
        (TempDirGuard(dir), r, n)
    }

    /// 临时目录守卫：drop 时尽力清理（测试失败也会清，不残留大临时文件）。
    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn nos(lines: &[BridgeLine]) -> Vec<u64> {
        lines.iter().map(|l| l.no).collect()
    }

    #[test]
    fn read_page_sequential_cursor_and_eof() {
        let (_g, mut r, n) = open(10);
        let mut cursor = 0u64;
        let mut all = Vec::new();
        loop {
            let page = r.read_page(cursor, 4).unwrap();
            if page.is_empty() {
                break;
            }
            cursor += page.len() as u64;
            all.extend(page.iter().map(|l| l.text.clone()));
        }
        assert_eq!(all.len(), n as usize);
        assert_eq!(all[0], "line-0");
        assert_eq!(all[9], "line-9");
        // 游标推进后重复读上一页之外的范围：超前/回跳都安全
        assert!(r.read_page(10, 4).unwrap().is_empty());
        assert!(r.read_page(u64::MAX, 4).unwrap().is_empty());
        assert!(r.read_page(0, 0).unwrap().is_empty());
    }

    #[test]
    fn read_page_crosses_anchor_boundaries() {
        let (_g, mut r, _) = open(4097 + 4096); // 8193 行 → 3 个锚点
                                                // 锚点前最后 2 行 + 跨锚点：4095,4096 | 4097,4098
        let p = r.read_page(4094, 2).unwrap();
        assert_eq!(p[0].text, "line-4094");
        assert_eq!(p[1].text, "line-4095");
        let p = r.read_page(4096, 2).unwrap();
        // no=4097 起恰是第二个锚点：零跳扫直接落位
        assert_eq!(p[0].text, "line-4096");
        assert_eq!(p[1].text, "line-4097");
    }

    #[test]
    fn sequential_page_keeps_midnight_wrap_after_cursor() {
        let dir = temp_dir("reader-midnight-cursor");
        let path = dir.join("midnight.log");
        let mut body = String::new();
        for i in 0..4_000 {
            let ts = if i < 2_500 {
                "23:59:59.000"
            } else {
                "00:00:00.000"
            };
            body.push_str(&format!("{ts}\tRX\tline-{i}\n"));
        }
        std::fs::write(&path, body).unwrap();
        let (_, mut r) = super::super::open_offline(&path).unwrap();

        let first = r.read_page(0, 3_000).unwrap();
        assert_eq!(first[2_500].epoch_millis, 86_400_000);
        let next = r.read_page(3_000, 1).unwrap();
        assert_eq!(next[0].epoch_millis, 86_400_000);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_page_random_seek_backward_then_forward() {
        let (_g, mut r, n) = open(6000);
        // 先读尾部（游标在尾部），再回跳中部：锚点回退路径
        let tail = r.read_page(5999, 1).unwrap();
        assert_eq!(tail[0].text, "line-5999");
        let mid = r.read_page(100, 3).unwrap();
        assert_eq!(mid[0].text, "line-100");
        assert_eq!(mid[2].text, "line-102");
        // 从回跳处继续顺序读（游标连读）
        let seq = r.read_page(103, 2).unwrap();
        assert_eq!(seq[0].text, "line-103");
        assert_eq!(seq[1].text, "line-104");
        assert_eq!(n, 6000);
    }

    #[test]
    fn non_data_lines_do_not_consume_no() {
        let dir = temp_dir("reader-mixed");
        let path = dir.join("m.log");
        std::fs::write(
            &path,
            "# h\n\n00:00:00.001\tRX\ta\nbad\n# c\n00:00:00.002\tTX\tb\n\n00:00:00.003\tRX\tc\nx\n00:00:00.004\tRX\td\n",
        )
        .unwrap();
        let (_, mut r) = super::super::open_offline(&path).unwrap();
        let page = r.read_page(0, 10).unwrap();
        assert_eq!(page.len(), 4);
        assert_eq!(page[0].text, "a");
        assert_eq!(page[3].text, "d");
        // "00:00:00.004" → 小数段 004 = 4ms
        assert_eq!(page[3].epoch_millis, 4);
        // 回跳页：坏行/注释不占号，no 映射稳定
        let back = r.read_page(1, 2).unwrap();
        assert_eq!(back[0].text, "b");
        assert_eq!(back[1].text, "c");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lines_after_maps_no_and_matches_ring_semantics() {
        let (_g, mut r, _) = open(10);
        let p = r.lines_after(0, 4).unwrap();
        assert_eq!(nos(&p), vec![1, 2, 3, 4]);
        assert_eq!(p[0].dir, Dir::Rx);
        let p = r.lines_after(4, 4).unwrap();
        assert_eq!(nos(&p), vec![5, 6, 7, 8]);
        let p = r.lines_after(8, 4).unwrap();
        assert_eq!(nos(&p), vec![9, 10]);
        assert!(r.lines_after(10, 4).unwrap().is_empty());
        assert!(r.lines_after(999, 4).unwrap().is_empty());
        // 游标回退（重拉同一页）结果稳定
        let again = r.lines_after(0, 4).unwrap();
        assert_eq!(nos(&again), vec![1, 2, 3, 4]);
        assert_eq!(again[0].text, "line-0");
        // epoch：ts_of(i) 的当日毫秒 = i mod 86400000（write_lines 的确定性构造）
        let p = r.lines_after(0, 1).unwrap();
        assert_eq!(p[0].epoch_millis, 0);
    }

    #[test]
    fn lines_before_mirrors_ring_semantics() {
        let (_g, mut r, _) = open(10);
        assert_eq!(nos(&r.lines_before(10, 4).unwrap()), vec![6, 7, 8, 9]);
        assert_eq!(nos(&r.lines_before(6, 4).unwrap()), vec![2, 3, 4, 5]);
        assert_eq!(nos(&r.lines_before(2, 4).unwrap()), vec![1]);
        assert!(r.lines_before(1, 4).unwrap().is_empty());
        assert_eq!(nos(&r.lines_before(999, 3).unwrap()), vec![8, 9, 10]);
        assert!(r.lines_before(0, 4).unwrap().is_empty());
    }

    #[test]
    fn snapshot_walks_all_pages_in_order() {
        let (_g, mut r, n) = open(10_000);
        let all = r.snapshot().unwrap();
        assert_eq!(all.len(), n as usize);
        assert_eq!(nos(&all), (1..=10_000).collect::<Vec<_>>());
        assert_eq!(all[4096].text, "line-4096");
        assert_eq!(all[4096].no, 4097);
        // 再刷一遍（游标已到尾）结果一致
        assert_eq!(r.snapshot().unwrap().len(), 10_000);
    }

    #[test]
    fn follow_returns_everything_after_cursor() {
        let (_g, mut r, n) = open(10);
        let (lines, last) = r.follow(0).unwrap();
        assert_eq!((lines.len(), last), (10, 10));
        assert_eq!(nos(&lines)[0], 1);
        let (lines, last) = r.follow(3).unwrap();
        assert_eq!((lines.len(), last), (7, 10));
        assert_eq!(lines[0].no, 4);
        let (lines, last) = r.follow(u64::MAX).unwrap();
        assert!(lines.is_empty());
        assert_eq!(last, n);
    }

    #[test]
    fn clear_masks_reads_but_keeps_counters() {
        let (_g, mut r, _) = open(10);
        let counters = r.dir_counters();
        assert_eq!((counters.0, counters.1), (7, 3)); // i%3==2 → TX：3 行 TX、7 行 RX
        r.clear();
        assert!(r.read_page(0, 10).unwrap().is_empty());
        assert!(r.lines_after(0, 10).unwrap().is_empty());
        assert!(r.lines_before(100, 10).unwrap().is_empty());
        assert!(r.snapshot().unwrap().is_empty());
        let (lines, last) = r.follow(0).unwrap();
        assert!(lines.is_empty());
        assert_eq!(last, 0);
        assert_eq!(r.last_no(), 0);
        assert_eq!(r.size(), 0);
        assert_eq!(r.bounds(), (0, 0, String::new(), String::new(), 0, 0, 0));
        // 计数器保留（与 RingBuf::clear 一致）
        assert_eq!(r.dir_counters(), counters);
        assert_eq!(r.line_count(), 10);
    }

    #[test]
    fn bounds_and_accessors_match_virtual_ring() {
        let (_g, r, _) = open(3);
        assert_eq!(
            r.bounds(),
            (1u64, 3u64, ts_of(0), ts_of(2), 0u64, 2u64, 3usize)
        );
        assert_eq!(r.last_no(), 3);
        assert_eq!(r.size(), 3);
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn invalid_utf8_page_carries_raw_bytes() {
        let dir = temp_dir("reader-utf8");
        let path = dir.join("u.log");
        std::fs::write(
            &path,
            b"00:00:01.000\tRX\tgood\n00:00:02.000\tRX\tbi\xffnary\n",
        )
        .unwrap();
        let (_, mut r) = super::super::open_offline(&path).unwrap();
        let page = r.lines_after(0, 10).unwrap();
        assert_eq!(page.len(), 2);
        assert!(page[0].bytes.is_none());
        let bl = &page[1];
        assert_eq!(bl.no, 2);
        assert_eq!(bl.bytes.as_deref(), Some(&b"bi\xffnary"[..]));
        assert!(bl.text.contains('\u{FFFD}'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn empty_file_reader_all_reads_empty() {
        let dir = temp_dir("reader-empty");
        let path = dir.join("e.log");
        std::fs::write(&path, "").unwrap();
        let (_, mut r) = super::super::open_offline(&path).unwrap();
        assert!(r.read_page(0, 10).unwrap().is_empty());
        assert!(r.lines_after(0, 10).unwrap().is_empty());
        assert!(r.snapshot().unwrap().is_empty());
        let (lines, last) = r.follow(0).unwrap();
        assert!(lines.is_empty());
        assert_eq!(last, 0);
        assert_eq!(r.bounds(), (0, 0, String::new(), String::new(), 0, 0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }
}
