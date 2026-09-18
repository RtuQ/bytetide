//! 串口链路实现：包装 `Box<dyn SerialPort>`。
//! serialport 依赖在 core 恒编译（仅 udev feature 由下游取舍——桌面开、CLI 关，
//! musl 静态构建依赖此纪律）；OS 串口打开收口在 [`SerialTransport::open`] 小构造函数，
//! 单测不触硬件（trait 契约用内存 fake 验证，见 transport/mod.rs 测试）。

use std::io;

use super::{Pin, Transport};
use crate::serial::port::{open_port, PortConfig};

/// 已打开的串口链路：io 读写与 DTR/RTS 置位都唯一借用同一个 `Box<dyn SerialPort>`。
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    /// 在调用线程内打开串口（读超时 200ms 由 `open_port` 设定）。
    /// 打开失败映射为 io::Error，错误文本与既有 `serialport::Error` Display 一致。
    pub(crate) fn open(config: &PortConfig) -> io::Result<Self> {
        let port = open_port(config).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Self { port })
    }
}

impl std::io::Read for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.port.as_mut().read(buf)
    }
}

impl std::io::Write for SerialTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.port.as_mut().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.port.as_mut().flush()
    }
}

impl Transport for SerialTransport {
    fn set_signal(&mut self, pin: Pin, level: bool) -> io::Result<()> {
        // serialport 的引脚方法返回其自有 Result<T, serialport::Error> 别名，映射成 io::Error
        let map = |e: serialport::Error| io::Error::other(e.to_string());
        let p = self.port.as_mut();
        match pin {
            Pin::Dtr => p.write_data_terminal_ready(level).map_err(map),
            Pin::Rts => p.write_request_to_send(level).map_err(map),
        }
    }

    fn description(&self) -> String {
        // 与 describe_transport 同一技术记法体系（英文、不翻译）
        "serial".to_string()
    }
}
