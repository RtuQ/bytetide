//! 离线分页会话集成测试（Stage 2 Task 8，外部 crate 视角只经 bytetide-core pub 项）：
//! 大文件在测试运行时生成于系统临时目录（确定性内容、不提交二进制，结束即删）。
//! 覆盖：200,001 行全量游标翻页、锚点边界（4096 对齐与跨页）、随机回跳、
//! 往前翻页/bounds/stats/snapshot、守卫（发送/分段/信号线）、清屏语义、
//! 会话 log_path，以及 2,000,001 行内存等价探针（结构性断言，方案见 probe 测试注释）。

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use bytetide_core::offline::{open_offline, OfflineReader};
use bytetide_core::serial::manager::{SendMode, SendRequest, RING_CAP};
use bytetide_core::serial::port::PortConfig;
use bytetide_core::serial::PortManager;

/// 第 i 行的确定性 ts（当日毫秒 = i mod 86400000，与 epoch 一致）。
fn ts_of(i: u64) -> String {
    let d = i % 86_400_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        d / 3_600_000,
        d / 60_000 % 60,
        d / 1_000 % 60,
        d % 1_000
    )
}

fn is_tx(i: u64) -> bool {
    i % 3 == 2
}

fn line_text(i: u64) -> String {
    format!("line-{i}")
}

fn line_epoch(i: u64) -> u64 {
    i % 86_400_000
}

/// 唯一临时目录（进程 id + 纳秒），测试结束 remove_dir_all。
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "bytetide-it-offline-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// 流式写 n 行 TSV（每行 `ts\tdir\ttext\n`），避免整文件驻内存。
fn write_log(dir: &Path, name: &str, n: u64, text: impl Fn(u64) -> String) -> PathBuf {
    let path = dir.join(name);
    let f = std::fs::File::create(&path).expect("create log file");
    let mut w = BufWriter::with_capacity(1024 * 1024, f);
    for i in 0..n {
        writeln!(
            w,
            "{}\t{}\t{}",
            ts_of(i),
            if is_tx(i) { "TX" } else { "RX" },
            text(i)
        )
        .expect("write line");
    }
    w.flush().expect("flush log");
    path
}

fn mk_manager() -> PortManager {
    PortManager::new()
}

#[test]
fn open_offline_indexed_pages_through_entire_file() {
    let dir = temp_dir("paging");
    const N: u64 = 200_001;
    let path = write_log(&dir, "big.log", N, line_text);
    let m = mk_manager();
    let (id, index) = m
        .load_offline_indexed(PortConfig::default(), path.clone())
        .expect("open offline indexed");
    // 索引摘要：行数与首末 epoch（首行 00:00:00.000=0，末行 200000ms）
    assert_eq!(index.line_count, N);
    assert_eq!(index.first_epoch, 0);
    assert_eq!(index.last_epoch, line_epoch(N - 1));
    // 游标翻页拉全量：no 连续 1..=N，页长 ≤ 5000
    let mut cursor = 0u64;
    let mut total = 0u64;
    loop {
        let page = m.ring_lines_after_no(&id, cursor, 5000).expect("page");
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 5000);
        for (i, l) in page.iter().enumerate() {
            let no = cursor + 1 + i as u64;
            assert_eq!(l.no, no, "no 连续分配");
            let src = no - 1;
            assert_eq!(l.text, line_text(src), "text 与生成序一致");
            assert_eq!(l.epoch_millis, line_epoch(src));
            if is_tx(src) {
                assert!(matches!(l.dir, bytetide_core::serial::port::Dir::Tx));
            }
        }
        cursor += page.len() as u64;
        total += page.len() as u64;
        assert!(total <= N, "游标语义不重不漏");
    }
    assert_eq!(total, N);
    assert_eq!(cursor, N);
    assert_eq!(m.bridge_last_no(&id), Some(N));
    // 锚点边界：no=4096 落在首块末尾（零跳扫不可用——want=4096 从锚点跳扫 4095 行），
    // no=4097 恰从第二个锚点直接落位；跨页不丢行
    let p = m.ring_lines_after_no(&id, 4094, 4).unwrap();
    assert_eq!(
        p.iter().map(|l| l.no).collect::<Vec<_>>(),
        vec![4095, 4096, 4097, 4098]
    );
    assert_eq!(p[2].text, "line-4096");
    // 随机回跳后继续顺序读
    let back = m.ring_lines_after_no(&id, 100, 3).unwrap();
    assert_eq!(back[0].text, "line-100");
    let seq = m.ring_lines_after_no(&id, 103, 2).unwrap();
    assert_eq!(seq[0].text, "line-103");
    // 超前游标安全
    assert!(m.ring_lines_after_no(&id, N, 10).unwrap().is_empty());
    m.disconnect(&id).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn offline_before_no_bounds_and_snapshot_match_ring_semantics() {
    let dir = temp_dir("before");
    const N: u64 = 200_001;
    let path = write_log(&dir, "big.log", N, line_text);
    let m = mk_manager();
    let (id, _) = m
        .load_offline_indexed(PortConfig::default(), path)
        .expect("open");
    // 往前翻页：no<before 的最新 max 行（升序）
    let back = m.ring_lines_before_no(&id, N, 2000).unwrap();
    assert_eq!(back.len(), 2000);
    assert_eq!(back[0].no, N - 2000);
    assert_eq!(back[1999].no, N - 1);
    // 已翻到最早行：返空
    assert!(m.ring_lines_before_no(&id, 1, 10).unwrap().is_empty());
    // before 超前：取最新 max 行
    let head = m.ring_lines_before_no(&id, u64::MAX, 3).unwrap();
    assert_eq!(
        head.iter().map(|l| l.no).collect::<Vec<_>>(),
        vec![N - 2, N - 1, N]
    );
    // bounds：虚拟 ring 全量在场
    let b = m.ring_bounds(&id).unwrap();
    assert_eq!(
        (b.first_no, b.last_no, b.size, b.ring_cap),
        (1, N, N as usize, RING_CAP)
    );
    // 快照=分页走完整个文件
    let snap = m.bridge_snapshot(&id).unwrap();
    assert_eq!(snap.len() as u64, N);
    assert_eq!(snap[0].no, 1);
    assert_eq!(snap[N as usize - 1].text, line_text(N - 1));
    // stats：方向计数与字节量按生成规则核对
    let stats = m.bridge_stats(&id).unwrap();
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut rb = 0u64;
    let mut tb = 0u64;
    for i in 0..N {
        let n = line_text(i).len() as u64;
        if is_tx(i) {
            tx += 1;
            tb += n;
        } else {
            rx += 1;
            rb += n;
        }
    }
    assert_eq!((stats.rx_lines, stats.tx_lines), (rx, tx));
    assert_eq!((stats.rx_bytes, stats.tx_bytes), (rb, tb));
    assert_eq!(
        (stats.first_no, stats.last_no, stats.size),
        (1, N, N as usize)
    );
    assert_eq!(
        (stats.first_epoch, stats.last_epoch),
        (0, line_epoch(N - 1))
    );
    assert_eq!(stats.first_ts, ts_of(0));
    assert_eq!(stats.last_ts, ts_of(N - 1));
    // REST follow 面：no>since 全量 + lastNo
    let (lines, last) = m.bridge_follow(&id, N - 3).unwrap();
    assert_eq!((lines.len(), last), (3, N));
    m.disconnect(&id).unwrap();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn offline_session_guards_clear_and_log_path() {
    let dir = temp_dir("guards");
    let path = write_log(&dir, "g.log", 10, line_text);
    let m = mk_manager();
    let (id, _) = m
        .load_offline_indexed(PortConfig::default(), path.clone())
        .expect("open");
    // log_path 指向源文件（「打开日志」/导出语义）
    assert_eq!(m.session_log_path(&id).unwrap(), path.to_string_lossy());
    // 状态 offline、line_count 报文件行数（ring 恒空不能报 0）
    let snap = m.bridge_list().into_iter().find(|s| s.id == id).unwrap();
    assert_eq!((snap.status.as_str(), snap.line_count), ("offline", 10));
    // 守卫：发送 / 信号线 / 分段 / 录制开关
    assert!(m
        .send(
            &id,
            SendRequest {
                mode: SendMode::Ascii,
                text: "x".into()
            }
        )
        .is_err());
    assert!(m
        .set_signal(&id, bytetide_core::serial::manager::Pin::Dtr, true)
        .is_err());
    assert!(m.rotate_log(&id).is_err());
    assert!(m.set_recording(&id, false).is_err());
    // 清屏=虚拟镜像遗忘：查询全空、no 游标语义不回退、文件事实保留
    m.clear_log(&id).unwrap();
    assert!(m.ring_lines_after_no(&id, 0, 10).unwrap().is_empty());
    assert!(m.ring_lines_before_no(&id, 100, 10).unwrap().is_empty());
    assert!(m.bridge_snapshot(&id).unwrap().is_empty());
    assert_eq!(m.bridge_last_no(&id), Some(0));
    let b = m.ring_bounds(&id).unwrap();
    assert_eq!((b.first_no, b.last_no, b.size), (0, 0, 0));
    let stats = m.bridge_stats(&id).unwrap();
    assert_eq!(stats.size, 0);
    assert_eq!((stats.rx_lines, stats.tx_lines), (7, 3)); // 计数器不重置
                                                          // 断开=停止墓碑（清屏后的镜像仍空，供最终补拉）；释放后彻底不可达。
                                                          // 旧全量路径（create_offline_session_cmd 背后）不受影响
    m.disconnect(&id).unwrap();
    assert!(m.ring_lines_after_no(&id, 0, 10).unwrap().is_empty());
    m.release_dead(&id);
    assert!(m.ring_lines_after_no(&id, 0, 10).is_err());
    let old = m.load_offline(PortConfig::default(), PathBuf::from("x.log"), vec![]);
    assert_eq!(m.ring_lines_after_no(&old, 0, 10).unwrap().len(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn offline_line_by_no_propagates_source_io_failure() {
    let dir = temp_dir("line-by-no-io");
    let path = write_log(&dir, "gone.log", 2, line_text);
    let m = mk_manager();
    let (id, _) = m
        .load_offline_indexed(PortConfig::default(), path.clone())
        .expect("open");
    std::fs::remove_file(&path).expect("remove source after indexing");

    let err = match m.bridge_line_by_no(&id, 1) {
        Err(err) => err,
        Ok(_) => panic!("source IO failure must not masquerade as a missing line"),
    };
    assert!(err.to_string().contains("offline log io error"));
    std::fs::remove_dir_all(&dir).ok();
}

/// 内存等价探针（结构性断言，替代不可移植的 RSS 读数——/proc 仅 Linux 有，
/// macOS/Windows 无同源接口）：
/// 1. 索引期持久内存 ≈ 锚点表：anchors.len() == ceil(line_count/4096)，
///    2,000,001 行 → 489 个 u64 ≈ 3.9KB；叠加 OfflineReader 固定字段仍 < 16KiB。
///    （扫描期另持单行读缓冲，页读期另持单页行 ≤ max——由下方页读断言界定。）
/// 2. 页读有界：read_page(0, 5000) 恰返 5000 行（不因文件大而多读），
///    末页恰返余量 1 行。
#[test]
fn memory_probe_index_is_sparse_and_pages_bounded() {
    let dir = temp_dir("probe");
    const N: u64 = 2_000_001;
    // ~100B/行 → ~214MB 文件，测试运行时生成、结束即删
    let path = write_log(&dir, "huge.log", N, |i| {
        format!("{:<90}", format!("line-{i:07}"))
    });
    let (index, reader) = open_offline(&path).expect("open huge");
    assert_eq!(index.line_count, N);
    assert_eq!(reader.error_count(), 0);
    // 结构性内存断言 1：锚点表长度（≈行数/4096）
    assert_eq!(reader.anchor_count() as u64, N.div_ceil(4096), "锚点=块数");
    let index_bytes =
        reader.anchor_count() * std::mem::size_of::<u64>() + std::mem::size_of::<OfflineReader>();
    assert!(
        index_bytes < 16 * 1024,
        "索引期持久内存应 <16KiB，实际 {index_bytes}B"
    );
    // 结构性内存断言 2：页读有界
    let mut r2 = reopen(&path);
    let first = r2.read_page(0, 5000).expect("first page");
    assert_eq!(first.len(), 5000);
    assert_eq!(first[0].text.trim(), "line-0000000");
    let tail = r2.read_page(N - 1, 5000).expect("tail page");
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].text.trim(), format!("line-{:07}", N - 1).trim());
    // 全量游标走通（5000×400 页）抽查锚点跨页处（no=4096/4097 分别位于两块）
    let cross = r2.read_page(4095, 3).expect("cross anchor");
    assert_eq!(cross[0].text.trim(), "line-0004095");
    assert_eq!(cross[2].text.trim(), "line-0004097");
    std::fs::remove_dir_all(&dir).ok();
}

fn reopen(path: &Path) -> OfflineReader {
    open_offline(path).expect("reopen").1
}
