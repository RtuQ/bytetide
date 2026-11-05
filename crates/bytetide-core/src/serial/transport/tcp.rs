//! TCP 链路（client 连接 / server 接受首个连入）。
//! 读超时 200ms、nodelay、
//! tcp-server 只服务首个接入连接（该连接断开即会话结束，多并发列为后续增强）。

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
        Self::accept_cancellable(bind_host, port, desc, &AtomicBool::new(false))
    }

    pub(crate) fn accept_cancellable(
        bind_host: String,
        port: u16,
        desc: String,
        stop: &AtomicBool,
    ) -> io::Result<Self> {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "TCP accept cancelled"));
        }
        let listener = TcpListener::bind((bind_host.as_str(), port))?;
        Self::accept_listener(listener, desc, stop)
    }

    fn accept_listener(listener: TcpListener, desc: String, stop: &AtomicBool) -> io::Result<Self> {
        listener.set_nonblocking(true)?;
        loop {
            // Check before accept even when a peer is already waiting.
            if stop.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "TCP accept cancelled"));
            }
            match listener.accept() {
                Ok((stream, _peer)) => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(NET_READ_TIMEOUT))?;
                    stream.set_nodelay(true).ok();
                    return Ok(Self { stream, desc });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error),
            }
        }
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};

    #[test]
    fn cancelled_accept_returns_promptly_and_releases_listen_port() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = TcpTransport::accept_listener(listener, "test".into(), &worker_stop);
            done_tx.send(result.err().map(|error| error.kind())).unwrap();
        });
        assert!(matches!(done_rx.recv_timeout(Duration::from_millis(50)), Err(mpsc::RecvTimeoutError::Timeout)));
        stop.store(true, Ordering::Relaxed);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap(), Some(io::ErrorKind::Interrupted));
        worker.join().unwrap();
        let rebound = TcpListener::bind(address).unwrap();
        assert_eq!(rebound.local_addr().unwrap(), address);
    }

    #[test]
    fn already_cancelled_accept_does_not_accept_waiting_peer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let _peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let stop = AtomicBool::new(true);
        let result = TcpTransport::accept_listener(listener, "test".into(), &stop);
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
    }
}
