//! 会话环形缓冲与桥数据类型：`RING_CAP`、`RingBuf`（拉模型唯一数据真相）与
//! `BridgeLine`/`MatchHit`/`RingBounds`/`BridgeStats` DTO。
//! Stage 2 Task 2 自 manager.rs 原样迁出；`serial::manager` 经再导出保持旧路径一个发布周期。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

use super::port::{Dir, LogLine};

/// 桥接环形缓冲容量（带原始字节的近期分析窗口；≈170B/行 × 10 万 ≈ 17MB/会话）。
pub const RING_CAP: usize = 100000;

/// REST 桥单行。`no` 为后端独立序号（与前端 `lineCounter` 无关，环形淘汰后继续递增）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLine {
    pub no: u64,
    pub ts: String,
    pub dir: Dir,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    pub epoch_millis: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#match: Option<MatchHit>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchHit {
    pub offset: u64,
    pub length: u64,
    pub field: String,
}

/// ring 现存行号边界（前端「翻页补旧行」判断还能不能往前翻）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RingBounds {
    pub first_no: u64,
    pub last_no: u64,
    pub size: usize,
    pub ring_cap: usize,
}

/// 会话统计（REST `/sessions/:id/stats`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStats {
    pub rx_lines: u64,
    pub tx_lines: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub first_no: u64,
    pub last_no: u64,
    pub first_ts: String,
    pub last_ts: String,
    pub first_epoch: u64,
    pub last_epoch: u64,
    pub ring_cap: usize,
    pub size: usize,
}

/// 每会话环形缓冲 + 计数器（读线程写入，REST 桥读取）。
/// 不参与 emit/盘写/批；仅在既有 `batch.push(line)` 旁增量写入。
pub struct RingBuf {
    ring: Mutex<VecDeque<BridgeLine>>,
    seq: AtomicU64,
    rx_lines: AtomicU64,
    tx_lines: AtomicU64,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
}

impl Default for RingBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl RingBuf {
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(VecDeque::new()),
            seq: AtomicU64::new(0),
            rx_lines: AtomicU64::new(0),
            tx_lines: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
        }
    }

    /// 环内现存行数是否为 0（clippy::len_without_is_empty：len 公开须配套）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 推入一行（分配单调 `no`、更新计数器、超容淘汰最旧）。不改 emit/盘写/批。
    pub fn push(&self, line: &LogLine) -> u64 {
        let no = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let bl = BridgeLine {
            no,
            ts: line.ts.clone(),
            dir: line.dir,
            text: line.text.clone(),
            bytes: line.bytes.clone(),
            epoch_millis: line.epoch_millis,
            r#match: None,
        };
        let n = line
            .bytes
            .as_deref()
            .map(|b| b.len())
            .unwrap_or_else(|| line.text.len()) as u64;
        match line.dir {
            Dir::Rx => {
                self.rx_lines.fetch_add(1, Ordering::Relaxed);
                self.rx_bytes.fetch_add(n, Ordering::Relaxed);
            }
            Dir::Tx => {
                self.tx_lines.fetch_add(1, Ordering::Relaxed);
                self.tx_bytes.fetch_add(n, Ordering::Relaxed);
            }
        }
        let mut r = self.ring.lock();
        r.push_back(bl);
        while r.len() > RING_CAP {
            r.pop_front();
        }
        no
    }

    /// 清屏：清空环形（不重置 `seq`，保持 `no` 单调，避免 REST 引用碰撞）。
    pub fn clear(&self) {
        self.ring.lock().clear();
    }

    pub fn snapshot(&self) -> Vec<BridgeLine> {
        self.ring.lock().iter().cloned().collect()
    }

    /// 仅返回 `no > since` 的行（长轮询 `/follow` 用，避免全 ring 拷贝）。
    pub fn lines_since(&self, since: u64) -> Vec<BridgeLine> {
        self.ring
            .lock()
            .iter()
            .filter(|l| l.no > since)
            .cloned()
            .collect()
    }

    /// 游标拉取：`no > since_no` 的最旧 max 行（no 单调递增，二分定位）。
    pub fn lines_after_no(&self, since_no: u64, max: usize) -> Vec<BridgeLine> {
        let ring = self.ring.lock();
        let from = ring.partition_point(|l| l.no <= since_no);
        ring.iter().skip(from).take(max).cloned().collect()
    }

    /// 往前翻页：`no < before_no` 的最新 max 行（视图缓冲裁掉旧行后从 ring 回补用，
    /// 仍按 no 升序返回；ring 已翻到最早行时返回不足 max 或空）。
    pub fn lines_before_no(&self, before_no: u64, max: usize) -> Vec<BridgeLine> {
        let ring = self.ring.lock();
        let end = ring.partition_point(|l| l.no < before_no);
        let start = end.saturating_sub(max);
        ring.iter().skip(start).take(end - start).cloned().collect()
    }

    /// 当前末行 `no`（空环返回 0）。
    pub fn last_no(&self) -> u64 {
        self.ring.lock().back().map(|l| l.no).unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.ring.lock().len()
    }

    /// 首/末行元信息（空环返回全 0）。
    pub fn bounds(&self) -> (u64, u64, String, String, u64, u64, usize) {
        let r = self.ring.lock();
        if r.is_empty() {
            return (0, 0, String::new(), String::new(), 0, 0, 0);
        }
        let f = r.front().expect("non-empty");
        let l = r.back().expect("non-empty");
        (
            f.no,
            l.no,
            f.ts.clone(),
            l.ts.clone(),
            f.epoch_millis,
            l.epoch_millis,
            r.len(),
        )
    }

    pub fn rx_lines(&self) -> u64 {
        self.rx_lines.load(Ordering::Relaxed)
    }
    pub fn tx_lines(&self) -> u64 {
        self.tx_lines.load(Ordering::Relaxed)
    }
    pub fn rx_bytes(&self) -> u64 {
        self.rx_bytes.load(Ordering::Relaxed)
    }
    pub fn tx_bytes(&self) -> u64 {
        self.tx_bytes.load(Ordering::Relaxed)
    }

    /// 取 epoch_ms >= since 的全部行（升序）。第二返回值=更早的行已被 ring
    /// 淘汰（发生过淘汰且现存首行晚于 since）——档案据此标注「前置现场可能缺失」；
    /// 会话刚开始、数据天然不足窗口不算缺失。
    pub(crate) fn lines_since_epoch(&self, since_epoch_ms: u64) -> (Vec<BridgeLine>, bool) {
        let ring = self.ring.lock();
        let missing_earlier = self.seq.load(Ordering::Relaxed) as usize > ring.len()
            && ring
                .front()
                .is_some_and(|l| l.epoch_millis > since_epoch_ms);
        let (mut lo, mut hi) = (0usize, ring.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if ring
                .get(mid)
                .is_some_and(|l| l.epoch_millis < since_epoch_ms)
            {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let out: Vec<BridgeLine> = ring.iter().skip(lo).cloned().collect();
        (out, missing_earlier)
    }
}

#[cfg(test)]
mod tests {
    //! RingBuf 纯逻辑单测：单调 no、快照顺序、游标、边界、驱逐、计数器。
    use super::{RingBuf, RING_CAP};
    use crate::serial::port::{Dir, LogLine};

    fn mk_log(ts: &str, dir: Dir, text: &str, bytes: Option<Vec<u8>>, epoch: u64) -> LogLine {
        LogLine {
            ts: ts.into(),
            dir,
            text: text.into(),
            bytes,
            epoch_millis: epoch,
        }
    }

    #[test]
    fn push_assigns_monotonic_no_and_snapshot_order() {
        let buf = RingBuf::new();
        for i in 0..3 {
            buf.push(&mk_log(
                "00:00:00.001",
                Dir::Rx,
                &format!("l{i}"),
                None,
                1000 + i,
            ));
        }
        let nos: Vec<u64> = buf.snapshot().iter().map(|l| l.no).collect();
        assert_eq!(nos, vec![1, 2, 3]);
        assert_eq!(buf.last_no(), 3);
        assert_eq!(buf.len(), 3);
    }

    #[test]
    fn lines_since_epoch_slices_and_reports_missing() {
        let buf = RingBuf::new();
        // 空 ring：空快照且不标缺失
        assert!(buf.lines_since_epoch(1).0.is_empty());
        for i in 0..5 {
            buf.push(&mk_log(
                "00:00:00.001",
                Dir::Rx,
                &format!("l{i}"),
                None,
                1000 + i * 10,
            ));
        }
        // since 落在首行之前：全量且无缺失
        let (snap, missing) = buf.lines_since_epoch(500);
        assert_eq!(snap.len(), 5);
        assert!(!missing);
        // since 命中第 3 行（epoch=1020）：取 1020..=1040 三行
        let (snap, missing) = buf.lines_since_epoch(1020);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].text, "l2");
        assert!(!missing);
        // since 早于全部行且 ring 未淘汰：全量、不标缺失
        let small = RingBuf::new();
        for i in 0..5 {
            small.push(&mk_log(
                "00:00:00.001",
                Dir::Rx,
                &format!("s{i}"),
                None,
                1000 + i * 10,
            ));
        }
        let (snap, missing) = small.lines_since_epoch(1000);
        assert_eq!(snap.len(), 5);
        assert!(!missing);
    }

    #[test]
    fn lines_since_epoch_flags_evicted_head() {
        // 真实淘汰：灌满 ring 溢出 5 行后，现存首行(=6)已晚于 since=1000
        // → 更早行被覆盖，标缺失
        let buf = RingBuf::new();
        let n = (RING_CAP + 5) as u64;
        for i in 0..n {
            buf.push(&mk_log("t", Dir::Rx, &format!("e{i}"), None, 1000 + i));
        }
        let (snap, missing) = buf.lines_since_epoch(1000);
        assert_eq!(snap.len(), RING_CAP);
        assert_eq!(snap[0].text, "e5");
        assert!(missing);
        // since 在现存首行之前：虽然发生过淘汰，但请求窗口内数据完整
        let (_, missing) = buf.lines_since_epoch(1000 + RING_CAP as u64);
        assert!(!missing);
    }

    #[test]
    fn lines_since_cursor() {
        let buf = RingBuf::new();
        for i in 0..5 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let s1: Vec<u64> = buf.lines_since(1).iter().map(|l| l.no).collect();
        assert_eq!(s1, vec![2, 3, 4, 5]);
        let s0: Vec<u64> = buf.lines_since(0).iter().map(|l| l.no).collect();
        assert_eq!(s0, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn lines_after_no_cursor_pages() {
        // 拉模型游标：no>since 的最旧 max 行；游标推进不重不漏；翻页到拉空
        let buf = RingBuf::new();
        for i in 0..10 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let p1 = buf.lines_after_no(0, 4);
        assert_eq!(
            p1.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        let p2 = buf.lines_after_no(4, 4);
        assert_eq!(
            p2.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![5, 6, 7, 8]
        );
        let p3 = buf.lines_after_no(8, 4);
        assert_eq!(p3.iter().map(|l| l.no).collect::<Vec<_>>(), vec![9, 10]);
        // 拉空：游标已到最新
        assert!(buf.lines_after_no(10, 4).is_empty());
        // 游标超前（ring 淘汰/新会话）也安全
        assert!(buf.lines_after_no(999, 4).is_empty());
    }

    #[test]
    fn lines_after_no_survives_clear() {
        // 清屏 seq 单调不回退：游标保持原位，只拉新行
        let buf = RingBuf::new();
        for i in 0..5 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        buf.clear();
        assert!(buf.lines_after_no(5, 4).is_empty());
        buf.push(&mk_log("t", Dir::Rx, "new", None, 100));
        let after = buf.lines_after_no(5, 4);
        assert_eq!(after.iter().map(|l| l.no).collect::<Vec<_>>(), vec![6]);
        assert_eq!(after[0].text, "new");
    }

    #[test]
    fn lines_before_no_pages_backwards() {
        // 往前翻页：no<before 的最新 max 行，升序返回；翻到 ring 最早行返空
        let buf = RingBuf::new();
        for i in 0..10 {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        let p1 = buf.lines_before_no(10, 4);
        assert_eq!(
            p1.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![6, 7, 8, 9]
        );
        let p2 = buf.lines_before_no(6, 4);
        assert_eq!(
            p2.iter().map(|l| l.no).collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
        let p3 = buf.lines_before_no(2, 4);
        assert_eq!(p3.iter().map(|l| l.no).collect::<Vec<_>>(), vec![1]);
        // 已翻到最早：安全返空；before_no 超前（比最新还大）取最新 max 行
        assert!(buf.lines_before_no(1, 4).is_empty());
        let p4 = buf.lines_before_no(999, 3);
        assert_eq!(p4.iter().map(|l| l.no).collect::<Vec<_>>(), vec![8, 9, 10]);
        assert!(buf.lines_before_no(0, 4).is_empty());
    }

    #[test]
    fn lines_before_no_empty_ring() {
        let buf = RingBuf::new();
        assert!(buf.lines_before_no(100, 4).is_empty());
        buf.push(&mk_log("t", Dir::Rx, "x", None, 0));
        assert!(buf.lines_before_no(1, 4).is_empty());
        assert_eq!(buf.lines_before_no(2, 4)[0].no, 1);
    }

    #[test]
    fn bounds_and_clear_keeps_seq_monotonic() {
        let buf = RingBuf::new();
        buf.push(&mk_log("01:00:00.000", Dir::Rx, "a", None, 3600000));
        buf.push(&mk_log("02:00:00.000", Dir::Tx, "b", None, 7200000));
        let (fno, lno, fts, lts, fep, lep, len) = buf.bounds();
        assert_eq!((fno, lno), (1, 2));
        assert_eq!(fts, "01:00:00.000");
        assert_eq!(lts, "02:00:00.000");
        assert_eq!((fep, lep), (3600000, 7200000));
        assert_eq!(len, 2);
        // 清屏不重置 seq，避免 no 引用碰撞
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.last_no(), 0);
        buf.push(&mk_log("t", Dir::Rx, "c", None, 0));
        assert_eq!(buf.snapshot()[0].no, 3);
    }

    #[test]
    fn evicts_oldest_beyond_cap() {
        let buf = RingBuf::new();
        let n = (RING_CAP + 5) as u64;
        for i in 0..n {
            buf.push(&mk_log("t", Dir::Rx, "x", None, i));
        }
        assert_eq!(buf.len(), RING_CAP);
        let snap = buf.snapshot();
        assert_eq!(snap.first().unwrap().no, 6); // 先 5 行被驱逐
        assert_eq!(snap.last().unwrap().no, n);
        assert_eq!(buf.last_no(), n);
    }

    #[test]
    fn counters_by_dir_and_bytes() {
        let buf = RingBuf::new();
        // rx 携带字节(2B) + rx 纯文本(len 3) + tx 纯文本(len 1)
        buf.push(&mk_log("t", Dir::Rx, "x", Some(vec![0xAA, 0x55]), 0));
        buf.push(&mk_log("t", Dir::Rx, "abc", None, 0));
        buf.push(&mk_log("t", Dir::Tx, "y", None, 0));
        assert_eq!(buf.rx_lines(), 2);
        assert_eq!(buf.tx_lines(), 1);
        assert_eq!(buf.rx_bytes(), 5); // 2 + 3
        assert_eq!(buf.tx_bytes(), 1);
    }
}
