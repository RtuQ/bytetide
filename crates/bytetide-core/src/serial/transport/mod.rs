//! 数据源链路抽象：`Transport` trait 与 `open_transport` 分发。
//! Stage 2 Task 3 自 manager.rs 迁出：`Link`/`NetLink`/`establish_link` 合并为
//! trait + 三个具体实现（serial/tcp/udp），host/bind/describe/is_net 等 helper 随迁。
//! 网络超时（NET_READ_TIMEOUT=200ms）与 UDP/TCP 语义逐字保留。

use std::io;
use std::time::Duration;

use super::port::PortConfig;

pub mod serial;
pub mod tcp;
pub mod udp;

/// 信号线引脚（输出方向：DTR/RTS；CTS/DSR 等输入引脚读取后续再加）。
#[derive(Clone, Copy, Debug)]
pub enum Pin {
    Dtr,
    Rts,
}

/// 已建立的链路视图（串口或网络）：io 读写 + DTR/RTS 置位 + 人类可读描述。
/// runtime 的读循环只面向此 trait——runtime 不持有/不锁具体 io 句柄类型。
pub trait Transport: std::io::Read + std::io::Write + Send {
    /// DTR/RTS 置位：串口链路直接写引脚；网络源在链路层报“无信号线”。
    fn set_signal(&mut self, pin: Pin, level: bool) -> io::Result<()>;
    /// 链路的人类可读描述（与 `describe_transport` 同一文案体系）。
    fn description(&self) -> String;
}

/// 网络源读超时（空闲期靠 TimedOut/WouldBlock 驱动半行刷出与命令轮询）。
pub(crate) const NET_READ_TIMEOUT: Duration = Duration::from_millis(200);

/// 网络源没有信号线（报错文案与既有实现逐字一致）。
pub(crate) fn net_signal_error() -> io::Error {
    io::Error::other("网络源无信号线")
}

/// 按配置建立链路：串口与 TCP/UDP 源共用同一装配路径，读线程内调用。
pub fn open_transport(config: &PortConfig) -> io::Result<Box<dyn Transport>> {
    if is_net_transport(config) {
        open_net(config)
    } else {
        serial::SerialTransport::open(config).map(|t| Box::new(t) as Box<dyn Transport>)
    }
}

fn open_net(config: &PortConfig) -> io::Result<Box<dyn Transport>> {
    let desc = describe_transport(config);
    match config.transport.as_deref() {
        Some("tcp-client") => {
            let host = config
                .tcp_host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".into());
            let port = config.tcp_port.unwrap_or(23);
            Ok(Box::new(tcp::TcpTransport::connect(host, port, desc)?))
        }
        Some("tcp-server") => Ok(Box::new(tcp::TcpTransport::accept(
            bind_host_of(config),
            config.tcp_port.unwrap_or(9000),
            desc,
        )?)),
        Some("udp") => Ok(Box::new(udp::UdpTransport::bind(
            bind_host_of(config),
            config.udp_local_port.unwrap_or(0),
            desc,
        )?)),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("未知传输类型: {other:?}"),
        )),
    }
}

/// tcp-server / udp 的监听地址：显式配置非空则用之，否则 0.0.0.0。
pub(crate) fn bind_host_of(config: &PortConfig) -> String {
    match config.tcp_host.as_deref() {
        Some(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => "0.0.0.0".to_string(),
    }
}

/// `%S`（主机地址）实参：网络源 = host:port（tcp-client 目标 / tcp-server、udp 监听地址，
/// 缺省值与 open_transport 一致）；串口源 = 端口名（与 `%H` 相同）。
pub(crate) fn host_of(config: &PortConfig) -> String {
    match config.transport.as_deref() {
        Some("tcp-client") => format!(
            "{}:{}",
            config
                .tcp_host
                .clone()
                .unwrap_or_else(|| "127.0.0.1".into()),
            config.tcp_port.unwrap_or(23)
        ),
        Some("tcp-server") => format!(
            "{}:{}",
            bind_host_of(config),
            config.tcp_port.unwrap_or(9000)
        ),
        Some("udp") => format!(
            "{}:{}",
            bind_host_of(config),
            config.udp_local_port.unwrap_or(0)
        ),
        _ => config.name.clone(),
    }
}

/// 链路的人类可读描述（建链失败错误消息 + 已建链的 `description()`）。
pub(crate) fn describe_transport(config: &PortConfig) -> String {
    match config.transport.as_deref() {
        Some("tcp-client") => format!(
            "TCP 连接 {}:{}",
            config.tcp_host.clone().unwrap_or_default(),
            config.tcp_port.map(|p| p.to_string()).unwrap_or_default()
        ),
        Some("tcp-server") => format!(
            "TCP 服务 {}:{}",
            bind_host_of(config),
            config.tcp_port.map(|p| p.to_string()).unwrap_or_default()
        ),
        Some("udp") => format!(
            "UDP 监听 {}:{}",
            bind_host_of(config),
            config
                .udp_local_port
                .map(|p| p.to_string())
                .unwrap_or_default()
        ),
        _ => "串口".to_string(),
    }
}

/// 是否网络源：transport 显式给出且不是 "serial"（旧 JSON 缺省字段 = 串口）。
pub(crate) fn is_net_transport(config: &PortConfig) -> bool {
    matches!(config.transport.as_deref(), Some(t) if t != "serial")
}

#[cfg(test)]
mod tests {
    //! 契约测试：内存 fake 链路测读写/信号线/描述；OS 串口打开不进单测。
    use std::io::{Read as _, Write as _};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 内存 fake 链路：读自内置缓冲、写进共享缓冲、信号线调用留痕。
    struct FakeTransport {
        inbox: std::io::Cursor<Vec<u8>>,
        written: Arc<Mutex<Vec<u8>>>,
        signals: Arc<Mutex<Vec<String>>>,
    }

    impl Transport for FakeTransport {
        fn set_signal(&mut self, pin: Pin, level: bool) -> io::Result<()> {
            self.signals
                .lock()
                .unwrap()
                .push(format!("{pin:?}={level}"));
            Ok(())
        }
        fn description(&self) -> String {
            "fake-link".into()
        }
    }

    impl std::io::Read for FakeTransport {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.inbox.read(buf)
        }
    }

    impl std::io::Write for FakeTransport {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn fake_transport_read_write_signal_and_description() {
        let written = Arc::new(Mutex::new(Vec::new()));
        let signals = Arc::new(Mutex::new(Vec::new()));
        let mut t = FakeTransport {
            inbox: std::io::Cursor::new(b"pong".to_vec()),
            written: written.clone(),
            signals: signals.clone(),
        };
        // 写：write_all 走 Write trait
        t.write_all(b"ping").expect("write");
        assert_eq!(*written.lock().unwrap(), b"ping".to_vec());
        // 读：缓冲内容逐字读出
        let mut buf = [0u8; 16];
        let n = t.read(&mut buf).expect("read");
        assert_eq!(&buf[..n], b"pong");
        // 信号线：fake 记录调用（串口真实语义由硬件路径验证，这里锁 trait 契约）
        t.set_signal(Pin::Dtr, true).expect("dtr");
        t.set_signal(Pin::Rts, false).expect("rts");
        assert_eq!(
            *signals.lock().unwrap(),
            vec!["Dtr=true".to_string(), "Rts=false".to_string()]
        );
        assert_eq!(t.description(), "fake-link");
    }

    #[test]
    fn open_transport_rejects_unknown_transport_kind() {
        let cfg = PortConfig {
            name: "x".into(),
            transport: Some("carrier-pigeon".into()),
            ..PortConfig::default()
        };
        let err = match open_transport(&cfg) {
            Err(e) => e,
            Ok(_) => panic!("未知传输必须报错"),
        };
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("未知传输类型"));
    }

    #[test]
    fn open_transport_udp_binds_and_reports_no_signal_line() {
        let cfg = PortConfig {
            name: "udp-test".into(),
            transport: Some("udp".into()),
            tcp_host: Some("127.0.0.1".into()),
            udp_local_port: Some(0),
            ..PortConfig::default()
        };
        let mut t = open_transport(&cfg).expect("udp bind");
        assert!(t.description().starts_with("UDP 监听 127.0.0.1:"));
        let err = t.set_signal(Pin::Dtr, true).expect_err("网络源无信号线");
        assert_eq!(err.to_string(), "网络源无信号线");
    }

    #[test]
    fn tcp_transport_loopback_read_write_and_description() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let cfg = PortConfig {
            name: format!("tcp-{port}"),
            transport: Some("tcp-client".into()),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(port),
            ..PortConfig::default()
        };
        let mut client = open_transport(&cfg).expect("tcp-client 连接本机");
        assert_eq!(client.description(), format!("TCP 连接 127.0.0.1:{port}"));
        let (mut server, _) = listener.accept().expect("客户端已连入");
        client.write_all(b"hello").expect("write");
        let mut buf = [0u8; 5];
        server.read_exact(&mut buf).expect("server read");
        assert_eq!(&buf, b"hello");
        let err = client.set_signal(Pin::Rts, true).expect_err("无信号线");
        assert_eq!(err.to_string(), "网络源无信号线");
    }

    // ---------- 纯 helper（自 manager 迁移的既有断言） ----------

    #[test]
    fn host_of_network_and_serial() {
        // tcp-client：目标 host:port
        let tcp = PortConfig {
            transport: Some("tcp-client".into()),
            tcp_host: Some("192.168.1.9".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&tcp), "192.168.1.9:9000");
        // tcp-server：监听地址缺省 0.0.0.0（与 open_transport 一致）
        let srv = PortConfig {
            transport: Some("tcp-server".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&srv), "0.0.0.0:9000");
        // udp：本地监听端口
        let udp = PortConfig {
            transport: Some("udp".into()),
            udp_local_port: Some(5000),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&udp), "0.0.0.0:5000");
        // 串口源：端口名（与 %H 相同）
        let serial = PortConfig {
            name: "COM3".into(),
            ..PortConfig::default()
        };
        assert_eq!(host_of(&serial), "COM3");
    }

    #[test]
    fn describe_transport_covers_all_kinds() {
        let tcp = PortConfig {
            transport: Some("tcp-client".into()),
            tcp_host: Some("1.2.3.4".into()),
            tcp_port: Some(80),
            ..PortConfig::default()
        };
        assert_eq!(describe_transport(&tcp), "TCP 连接 1.2.3.4:80");
        let srv = PortConfig {
            transport: Some("tcp-server".into()),
            tcp_port: Some(9000),
            ..PortConfig::default()
        };
        assert_eq!(describe_transport(&srv), "TCP 服务 0.0.0.0:9000");
        let udp = PortConfig {
            transport: Some("udp".into()),
            udp_local_port: Some(5000),
            ..PortConfig::default()
        };
        assert_eq!(describe_transport(&udp), "UDP 监听 0.0.0.0:5000");
        let serial = PortConfig {
            name: "COM3".into(),
            ..PortConfig::default()
        };
        assert_eq!(describe_transport(&serial), "串口");
        // 未知类型同样落「串口」文案（错误消息体系与既有实现一致）
        let unknown = PortConfig {
            transport: Some("??".into()),
            ..PortConfig::default()
        };
        assert_eq!(describe_transport(&unknown), "串口");
    }

    #[test]
    fn is_net_transport_matches_existing_semantics() {
        assert!(!is_net_transport(&PortConfig::default()));
        assert!(!is_net_transport(&PortConfig {
            transport: Some("serial".into()),
            ..PortConfig::default()
        }));
        assert!(is_net_transport(&PortConfig {
            transport: Some("tcp-client".into()),
            ..PortConfig::default()
        }));
        assert!(is_net_transport(&PortConfig {
            transport: Some("udp".into()),
            ..PortConfig::default()
        }));
    }
}
