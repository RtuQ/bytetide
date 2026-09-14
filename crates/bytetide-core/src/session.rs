use std::fs::OpenOptions;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::serial::port::{Dir, LogLine};

/// 单会话数据日志：追加写文件，定期 flush，支持截断（清屏）。
pub struct SessionLog {
    writer: BufWriter<std::fs::File>,
    pending: usize,
}

impl SessionLog {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // 用 write 权限打开并 seek 到末尾，而非 append 标志——Windows 的
        // append 句柄只有 FILE_APPEND_DATA 权限，set_len（清屏截断）会
        // Access Denied（macOS/Linux 的 O_APPEND 无此限制）；write+seek
        // 保留「打开即续写」语义且截断可用。
        let mut file = OpenOptions::new().create(true).write(true).open(path)?;
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            writer: BufWriter::new(file),
            pending: 0,
        })
    }

    pub fn append(&mut self, line: &LogLine) {
        let dir = match line.dir {
            Dir::Rx => "RX",
            Dir::Tx => "TX",
        };
        let _ = writeln!(self.writer, "{}\t{}\t{}", line.ts, dir, line.text);
        self.pending += 1;
        if self.pending >= 64 {
            let _ = self.writer.flush();
            self.pending = 0;
        }
    }

    /// 写入一行原始文本（现场档案头注释用；不经 TSV 三列转义，前端解析按 `#` 跳过）。
    pub fn write_raw_line(&mut self, line: &str) {
        let _ = writeln!(self.writer, "{line}");
        self.pending += 1;
        if self.pending >= 64 {
            let _ = self.writer.flush();
            self.pending = 0;
        }
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        let file = self.writer.get_mut();
        file.set_len(0)?;
        // 游标归零：截断后旧游标越界，续写必须从 0 开始（否则留空洞/语义错位）
        file.seek(SeekFrom::Start(0))?;
        self.pending = 0;
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        self.pending = 0;
        Ok(())
    }
}
