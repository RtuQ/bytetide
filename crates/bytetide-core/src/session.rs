use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
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
        let file = OpenOptions::new().create(true).append(true).open(path)?;
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

    pub fn clear(&mut self) -> std::io::Result<()> {
        let _ = self.writer.flush();
        self.writer.get_ref().set_len(0)?;
        self.pending = 0;
        Ok(())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()?;
        self.pending = 0;
        Ok(())
    }
}
