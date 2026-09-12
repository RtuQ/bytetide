//! 离线日志索引：`open_offline` 用 `BufRead` 顺序扫全文件一次，产出
//! [`OfflineIndex`]（行数/首末 epoch）+ 预定位好聚合值的 [`OfflineReader`]。
//! 内存约束：只持稀疏锚点表（≈line_count/4096 个 u64）+ 单行读缓冲，
//! 不把任何整页（遑论全文件）行驻留索引期——全文件进 ring 的旧路径不在此处。

use std::io::{BufRead, BufReader};
use std::path::Path;

use super::reader::OfflineReader;
use super::{classify_line, OfflineError, RawRow, PAGE_LINES};
use crate::serial::port::{Dir, LogLine};

/// 离线日志摘要（`open_offline` / `PortManager::load_offline_indexed` 返回）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfflineIndex {
    /// 数据行数（跳过空行/`#`注释/坏行后按行序连续编号的总数）。
    pub line_count: u64,
    /// 首个数据行 epoch_millis（当日毫秒或行序回退；空文件为 0）。
    pub first_epoch: u64,
    /// 末个数据行 epoch_millis（空文件为 0）。
    pub last_epoch: u64,
}

/// 流式打开离线日志：一次顺序扫描建稀疏索引（每 [`PAGE_LINES`] 个数据行记
/// 一个文件字节偏移锚点）并累计行数/首末 epoch/方向计数。返回索引摘要与
/// 可分页读取的 [`OfflineReader`]。文件假定静态（打开后增长不可见、截断读出
/// 不足页）；索引期 IO 错误直接失败。
pub fn open_offline(path: &Path) -> Result<(OfflineIndex, OfflineReader), OfflineError> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    // 锚点：anchors[k] = 数据行 no=k*PAGE_LINES+1 的字节偏移。在读到「将成为
    // 第 k*4096+1 个数据行」的那一行时记录其起始偏移（跳过行不推进 data_no，
    // 不会误记）；len = ceil(line_count/PAGE_LINES)，空文件为 0（页读不会用到）。
    let mut anchors: Vec<u64> = Vec::new();
    let mut off: u64 = 0;
    let mut data_no: u64 = 0;
    let mut bad_rows: u64 = 0;
    let mut first_epoch: u64 = 0;
    let mut last_epoch: u64 = 0;
    let mut first_ts = String::new();
    let mut last_ts = String::new();
    let mut rx_lines: u64 = 0;
    let mut tx_lines: u64 = 0;
    let mut rx_bytes: u64 = 0;
    let mut tx_bytes: u64 = 0;
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        match classify_line(&buf, data_no) {
            RawRow::Skip => {}
            RawRow::Bad => bad_rows += 1,
            RawRow::Row(LogLine {
                ts,
                dir,
                text,
                bytes,
                epoch_millis,
            }) => {
                // off 仍指向本行起始（尚未累加 n）：本行若成为第 k*4096+1 个
                // 数据行，其起始偏移即第 k 块的锚点
                if data_no.is_multiple_of(PAGE_LINES) {
                    anchors.push(off);
                }
                if data_no == 0 {
                    first_epoch = epoch_millis;
                    first_ts = ts.clone();
                }
                last_epoch = epoch_millis;
                last_ts = ts;
                // 字节计数与 RingBuf::push 同口径：bytes 优先，否则 text 的 UTF-8 字节数
                let n = bytes
                    .as_deref()
                    .map(|b| b.len())
                    .unwrap_or_else(|| text.len()) as u64;
                match dir {
                    Dir::Rx => {
                        rx_lines += 1;
                        rx_bytes += n;
                    }
                    Dir::Tx => {
                        tx_lines += 1;
                        tx_bytes += n;
                    }
                }
                data_no += 1;
            }
        }
        off += n as u64;
    }
    let index = OfflineIndex {
        line_count: data_no,
        first_epoch,
        last_epoch,
    };
    let out = OfflineReader::new(
        path.to_path_buf(),
        anchors,
        index,
        first_ts,
        last_ts,
        rx_lines,
        tx_lines,
        rx_bytes,
        tx_bytes,
        bad_rows,
    );
    Ok((index, out))
}

#[cfg(test)]
mod tests {
    //! 索引单测：混合文件计数、锚点公式、空文件/缺文件、无换行末行。
    use super::*;
    use crate::offline::test_support::{temp_dir, ts_of, write_lines};

    #[test]
    fn open_offline_counts_mixed_file() {
        let dir = temp_dir("idx-mixed");
        let path = dir.join("mixed.log");
        std::fs::write(
            &path,
            concat!(
                "# capture header meta\n",
                "# second comment\n",
                "\n",
                "00:00:01.000\tRX\thello\n",
                "00:00:02.000\ttx\tworld\r\n",
                "bad line no tab\n",
                "  \n",
                "00:00:03.500\tRX\tspaced\r\n",
                "00:00:04.000\tTX\ttail-no-newline",
            ),
        )
        .unwrap();
        let (index, reader) = open_offline(&path).unwrap();
        assert_eq!(index.line_count, 4);
        assert_eq!(index.first_epoch, 1_000);
        assert_eq!(index.last_epoch, 4_000);
        assert_eq!(reader.error_count(), 2); // "bad line no tab" + "  "
        let (rx, tx, rb, tb) = reader.dir_counters();
        assert_eq!((rx, tx), (2, 2)); // world/tail 是 tx（大小写不敏感）
                                      // 字节数 = text 的 UTF-8 字节长（hello=5, spaced=6 | world=5, tail=15）
        assert_eq!(rb, 5 + 6);
        assert_eq!(tb, 5 + 15);
        assert_eq!(
            reader.ts_bounds(),
            ("00:00:01.000".to_string(), "00:00:04.000".to_string())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn anchor_count_follows_page_formula() {
        // 锚点数 = ceil(line_count/PAGE_LINES)，空文件为 0（只在读到数据行时记录）
        for (n, want) in [
            (0u64, 0usize),
            (1, 1),
            (4096, 1),
            (4097, 2),
            (8192, 2),
            (8193, 3),
        ] {
            let dir = temp_dir("idx-anchor");
            let path = if n == 0 {
                let p = dir.join("empty.log");
                std::fs::write(&p, "# only comments\n\nno-tab-bad\n").unwrap();
                p
            } else {
                write_lines(&dir, "lines.log", n)
            };
            let (index, reader) = open_offline(&path).unwrap();
            assert_eq!(reader.anchor_count(), want, "n={n}");
            assert_eq!(index.line_count, if n == 0 { 0 } else { n });
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn empty_and_comment_only_files_index_to_zero_lines() {
        let dir = temp_dir("idx-empty");
        for content in ["", "# c1\n# c2\n"] {
            let path = dir.join("e.log");
            std::fs::write(&path, content).unwrap();
            let (index, mut reader) = open_offline(&path).unwrap();
            assert_eq!(index.line_count, 0);
            assert_eq!((index.first_epoch, index.last_epoch), (0, 0));
            assert!(reader.read_page(0, 100).unwrap().is_empty());
            assert_eq!(
                reader.bounds(),
                (0, 0, String::new(), String::new(), 0, 0, 0)
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_io_error() {
        let dir = temp_dir("idx-missing");
        let err = match open_offline(&dir.join("nope.log")) {
            Err(e) => e,
            Ok(_) => panic!("missing file should fail"),
        };
        assert!(matches!(err, OfflineError::Io(_)));
        assert!(err.to_string().contains("offline log io error"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn epochs_come_from_first_and_last_data_line() {
        let dir = temp_dir("idx-epochs");
        let path = dir.join("e.log");
        let body = format!(
            "00:00:00.000\tRX\ta\n{}\tRX\tb\nbad\n{}\tTX\tc\n",
            ts_of(5),
            ts_of(9),
        );
        std::fs::write(&path, body).unwrap();
        let (index, _) = open_offline(&path).unwrap();
        assert_eq!(index.line_count, 3);
        assert_eq!(index.first_epoch, 0); // 首行 ts 合法值为 0
        assert_eq!(index.last_epoch, 9);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_utf8_rows_keep_bytes_and_count_lines() {
        let dir = temp_dir("idx-utf8");
        let path = dir.join("u.log");
        std::fs::write(&path, b"00:00:01.000\tRX\tok\n00:00:02.000\tRX\t\xff\xfe\n").unwrap();
        let (index, mut reader) = open_offline(&path).unwrap();
        assert_eq!(index.line_count, 2);
        assert_eq!(reader.error_count(), 0);
        let page = reader.read_page(0, 10).unwrap();
        assert_eq!(page.len(), 2);
        assert!(page[0].bytes.is_none());
        assert_eq!(page[1].bytes.as_deref(), Some(&b"\xff\xfe"[..]));
        assert!(page[1].text.contains('\u{FFFD}'));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn open_offline_consumes_golden_tsv_fixture() {
        // 共享黄金样本 testdata/protocol/tsv-v1.log（TS useLogParser.test.ts 同源）：
        // 流式索引/分页全链路，含 # 注释头、坏行、epoch 回退、lossy U+FFFD。
        use crate::offline::test_support::TSV_FIXTURE;
        let dir = temp_dir("idx-fixture");
        // LF 原样（仓库 .gitattributes eol=lf，include_str! 拿到 LF 版本）
        let path = dir.join("golden.log");
        std::fs::write(&path, TSV_FIXTURE).unwrap();
        let (index, mut reader) = open_offline(&path).unwrap();
        assert_eq!(index.line_count, 9);
        assert_eq!((index.first_epoch, index.last_epoch), (1000, 8250));
        assert_eq!(reader.error_count(), 1);
        let page = reader.read_page(0, 100).unwrap();
        assert_eq!(page.len(), 9);
        assert_eq!(page[5].epoch_millis, 5); // garbage-ts → 行序回退
        assert_eq!(page[4].text, "text may contain\ta tab here");
        assert!(page[6].text.contains('\u{FFFD}'));
        assert!(page[6].bytes.is_none()); // 有损 TSV 落盘无原始字节
                                          // CRLF 变体（运行时 LF→CRLF 全文转换，对齐 TS 黄金 CRLF 用例）：逐行一致
        let crlf_path = dir.join("golden-crlf.log");
        std::fs::write(&crlf_path, TSV_FIXTURE.replace('\n', "\r\n")).unwrap();
        let (crlf_index, mut crlf_reader) = open_offline(&crlf_path).unwrap();
        assert_eq!(crlf_index.line_count, 9);
        assert_eq!(
            (crlf_index.first_epoch, crlf_index.last_epoch),
            (1000, 8250)
        );
        assert_eq!(crlf_reader.error_count(), 1);
        let crlf_page = crlf_reader.read_page(0, 100).unwrap();
        assert_eq!(crlf_page.len(), page.len());
        for (a, b) in page.iter().zip(&crlf_page) {
            assert_eq!(
                (
                    a.ts.as_str(),
                    a.dir,
                    a.text.as_str(),
                    a.bytes.clone(),
                    a.epoch_millis
                ),
                (
                    b.ts.as_str(),
                    b.dir,
                    b.text.as_str(),
                    b.bytes.clone(),
                    b.epoch_millis
                )
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
