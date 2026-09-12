//! TCP 链路（client 连接 / server 接受首个连入）。
//! 语义与原 `establish_link` 的 Tcp 分支逐字一致：读超时 200ms、nodelay、
//! tcp-server 只服务首个接入连接（该连接断开即会话结束，多并发列为后续增强）。

use std::io;
use std::net::TcpStream;

use super::{net_signal_error, Pin, Transport, NET_READ_TIMEOUT};

/// 已建立的 TCP 连接。
pub struct TcpTransport {
    stream: TcpStream,
    desc: String,
}

impl TcpTransport {
    /// tcp-client：主动连接目标 host:port。
    pub(crate) fn connect(host: String, port: u16, desc: String) -> io::Result<Self> {
        let stream = TcpStream::connect((host.as_str(), port))?;
        stream.set_read_timeout(Some(NET_READ_TIMEOUT))?;
        stream.set_nodelay(true).ok();
        Ok(Self { stream, desc })
    }

    /// tcp-server：绑定监听地址后只接受首个连入连接。
    pub(crate) fn accept(bind_host: String, port: u16, desc: String) -> io::Result<Self> {
        let listener = std::net::TcpListener::bind((bind_host.as_str(), port))?;
        let (stream, _peer) = listener.accept()?;
        stream.set_read_timeout(Some(NET_READ_TIMEOUT))?;
        stream.set_nodelay(true).ok();
        Ok(Self { stream, desc })
    }
}

impl std::io::Read for TcpTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl std::io::Write for TcpTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Transport for TcpTransport {
    fn set_signal(&mut self, _pin: Pin, _level: bool) -> io::Result<()> {
        Err(net_signal_error())
    }

    fn description(&self) -> String {
        self.desc.clone()
    }
}
