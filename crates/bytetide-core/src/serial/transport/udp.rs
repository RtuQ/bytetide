//! UDP 监听链路：纯接收语义与原 `establish_link` 的 Udp 分支逐字一致。
//! 读超时 200ms；未 connect 的监听 socket 上 send 会报 NotConnected，
//! 符合“UDP 纯接收”设计（对端发送列为后续增强）。

use std::io::{self};
use std::net::UdpSocket;

use super::{net_signal_error, Pin, Transport, NET_READ_TIMEOUT};

/// 已绑定的 UDP 监听 socket。
pub struct UdpTransport {
    sock: UdpSocket,
    desc: String,
}

impl UdpTransport {
    /// 绑定本地监听地址（port=0 由系统分配）。
    pub(crate) fn bind(bind_host: String, port: u16, desc: String) -> io::Result<Self> {
        let sock = UdpSocket::bind((bind_host.as_str(), port))?;
        sock.set_read_timeout(Some(NET_READ_TIMEOUT))?;
        Ok(Self { sock, desc })
    }
}

impl std::io::Read for UdpTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // UDP 无 Read 语义歧义，直接用固有 recv
        self.sock.recv(buf)
    }
}

impl std::io::Write for UdpTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.send(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Transport for UdpTransport {
    fn set_signal(&mut self, _pin: Pin, _level: bool) -> io::Result<()> {
        Err(net_signal_error())
    }

    fn description(&self) -> String {
        self.desc.clone()
    }
}
