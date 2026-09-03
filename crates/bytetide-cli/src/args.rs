//! 命令行参数定义与 PortConfig 映射（映射为纯函数，便于单测）。

use bytetide_core::serial::port::PortConfig;
use clap::{ArgGroup, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "bytetide",
    version,
    about = "ByteTide CLI：无 UI 的串口/网络日志监控（数据行走 stdout，提示走 stderr）"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)] // MonitorArgs 仅一个变体持有，装箱无收益
pub enum Command {
    /// 列出可用串口
    List,
    /// 监控数据源（串口 / TCP / UDP），Ctrl-C 或 /quit 退出
    #[command(after_help = AFTER_HELP)]
    Monitor(MonitorArgs),
}

pub const AFTER_HELP: &str = "数据源缺省且 stdin 为终端时进入交互选择；非终端 stdin 未指定数据源直接报错。

交互命令（stdin 为终端时可用）：
  直接输入回车按当前模式发送（ASCII 默认追加换行）
  /hex AA 01 5a     单次十六进制发送
  /mode ascii|hex   切换默认发送模式
  /quit             退出（与 Ctrl-C 相同）

录制模板 token 与桌面端一致：%Y %M %D %H(端口/会话名) %h %m %s %t %%；缺省不落盘。
--retry 重连后新会话行号从头计数，重连期间收到的数据会重新输出。";

#[derive(clap::Args, Debug)]
// 四个数据源参数互斥（缺省合法：进入交互选择或报错，由运行时处理）
#[command(group(ArgGroup::new("source").args(&["port", "tcp", "tcp_listen", "udp"])))]
pub struct MonitorArgs {
    /// 串口路径（如 COM3、/dev/ttyUSB0）
    #[arg(short, long, value_name = "PATH")]
    pub port: Option<String>,
    /// 波特率
    #[arg(long, default_value_t = 115200)]
    pub baud: u32,
    /// 数据位 5-8
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u8).range(5..=8))]
    pub data: u8,
    /// 校验位
    #[arg(long, default_value = "none", value_parser = ["none", "odd", "even"])]
    pub parity: String,
    /// 停止位
    #[arg(long, default_value = "1", value_parser = ["1", "2"])]
    pub stop: String,
    /// 流控
    #[arg(long, default_value = "none", value_parser = ["none", "software", "hardware"])]
    pub flow: String,
    /// TCP 客户端，连接 HOST:PORT
    #[arg(long, value_name = "HOST:PORT")]
    pub tcp: Option<String>,
    /// TCP 服务端监听端口（绑定 0.0.0.0，接受首个接入）
    #[arg(long, value_name = "PORT")]
    pub tcp_listen: Option<u16>,
    /// UDP 监听端口
    #[arg(long, value_name = "PORT")]
    pub udp: Option<u16>,
    /// 录制文件路径/模板（缺省不录制；token 见帮助尾注）
    #[arg(short = 'o', long, value_name = "PATH|TEMPLATE")]
    pub record: Option<String>,
    /// 会话名（网络源用作录制模板 %H 与提示）
    #[arg(long, value_name = "NAME")]
    pub id: Option<String>,
    /// 每行显示时间戳（默认关）
    #[arg(long)]
    pub ts: bool,
    /// 时间戳格式 token（如 %h:%m:%s.%t；隐含 --ts）
    #[arg(long, value_name = "FMT")]
    pub ts_format: Option<String>,
    /// JSON Lines 输出（每行一个 JSON 对象，忽略 --ts）
    #[arg(long)]
    pub json: bool,
    /// 禁用颜色（NO_COLOR 环境变量与非终端 stdout 同样生效）
    #[arg(long)]
    pub no_color: bool,
    /// 意外断开后的重连次数（间隔 1 秒）
    #[arg(long, default_value_t = 0)]
    pub retry: u32,
    /// ASCII 发送追加换行（默认行为，显式写法；与 --no-newline 同给时优先）
    #[arg(long)]
    pub newline: bool,
    /// ASCII 发送不追加换行
    #[arg(long)]
    pub no_newline: bool,
}

/// 参数 → 数据源配置；Ok(None) 表示未指定（终端进入选择器，非终端报错）。
/// 四源互斥由 clap group 保证；此处按串口 > tcp > tcp-listen > udp 的顺序取第一个。
pub fn to_port_config(args: &MonitorArgs) -> Result<Option<PortConfig>, String> {
    if let Some(port) = args.port.as_deref() {
        return Ok(Some(serial_config(args, port)));
    }
    if let Some(endpoint) = args.tcp.as_deref() {
        let (host, port) = parse_endpoint(endpoint)?;
        return Ok(Some(PortConfig {
            name: args.id.clone().unwrap_or_else(|| format!("{host}:{port}")),
            transport: Some("tcp-client".into()),
            tcp_host: Some(host),
            tcp_port: Some(port),
            ..PortConfig::default()
        }));
    }
    if let Some(port) = args.tcp_listen {
        return Ok(Some(PortConfig {
            name: args.id.clone().unwrap_or_else(|| format!("tcp-listen:{port}")),
            transport: Some("tcp-server".into()),
            tcp_port: Some(port), // tcp_host 留空 -> core 绑 0.0.0.0
            ..PortConfig::default()
        }));
    }
    if let Some(port) = args.udp {
        return Ok(Some(PortConfig {
            name: args.id.clone().unwrap_or_else(|| format!("udp:{port}")),
            transport: Some("udp".into()),
            udp_local_port: Some(port),
            ..PortConfig::default()
        }));
    }
    Ok(None)
}

/// 串口配置：name 固定为端口路径（录制模板 %H 即端口名），其余套 CLI 参数。
pub fn serial_config(args: &MonitorArgs, port: &str) -> PortConfig {
    PortConfig {
        name: port.to_string(),
        baud_rate: args.baud,
        data_bits: args.data,
        parity: args.parity.clone(),
        stop_bits: args.stop.clone(),
        flow_control: args.flow.clone(),
        ..PortConfig::default()
    }
}

/// "HOST:PORT" → (host, port)；按最后一个冒号切分，兼容 IPv6 字面量（[] 可省）。
fn parse_endpoint(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    let Some((host, port)) = s.rsplit_once(':') else {
        return Err(format!("--tcp 需要 HOST:PORT 格式，收到 \"{s}\""));
    };
    if host.is_empty() {
        return Err(format!("--tcp 主机名为空: {s}"));
    }
    let port: u16 = port
        .parse()
        .map_err(|_| format!("--tcp 端口无效: \"{port}\""))?;
    Ok((host.trim_matches(['[', ']']).to_string(), port))
}

/// ASCII 发送是否追加换行：默认追加；--no-newline 关闭，--newline 显式优先
pub fn append_newline(args: &MonitorArgs) -> bool {
    args.newline || !args.no_newline
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn base() -> MonitorArgs {
        MonitorArgs {
            port: None,
            baud: 115200,
            data: 8,
            parity: "none".into(),
            stop: "1".into(),
            flow: "none".into(),
            tcp: None,
            tcp_listen: None,
            udp: None,
            record: None,
            id: None,
            ts: false,
            ts_format: None,
            json: false,
            no_color: false,
            retry: 0,
            newline: false,
            no_newline: false,
        }
    }

    #[test]
    fn serial_defaults() {
        let mut a = base();
        a.port = Some("COM3".into());
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!(c.name, "COM3");
        assert_eq!(c.baud_rate, 115200);
        assert_eq!(c.data_bits, 8);
        assert_eq!(c.parity, "none");
        assert_eq!(c.stop_bits, "1");
        assert_eq!(c.flow_control, "none");
        assert_eq!(c.transport, None);
    }

    #[test]
    fn serial_custom_params() {
        let mut a = base();
        a.port = Some("/dev/ttyUSB0".into());
        a.baud = 9600;
        a.data = 7;
        a.parity = "odd".into();
        a.stop = "2".into();
        a.flow = "hardware".into();
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!((c.baud_rate, c.data_bits), (9600, 7));
        assert_eq!(
            (c.parity.as_str(), c.stop_bits.as_str(), c.flow_control.as_str()),
            ("odd", "2", "hardware")
        );
        assert_eq!(c.name, "/dev/ttyUSB0");
    }

    #[test]
    fn tcp_client_mapping() {
        let mut a = base();
        a.tcp = Some("192.168.1.10:9000".into());
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!(c.transport.as_deref(), Some("tcp-client"));
        assert_eq!(c.tcp_host.as_deref(), Some("192.168.1.10"));
        assert_eq!(c.tcp_port, Some(9000));
        assert_eq!(c.name, "192.168.1.10:9000"); // 无 --id 时端点名即会话名
        a.id = Some("probe".into());
        assert_eq!(to_port_config(&a).unwrap().unwrap().name, "probe");
    }

    #[test]
    fn tcp_client_ipv6_and_invalid() {
        let mut a = base();
        a.tcp = Some("[::1]:7000".into());
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!(c.tcp_host.as_deref(), Some("::1"));
        assert_eq!(c.tcp_port, Some(7000));
        a.tcp = Some("host-no-port".into());
        assert!(to_port_config(&a).is_err());
        a.tcp = Some("host:notaport".into());
        assert!(to_port_config(&a).is_err());
    }

    #[test]
    fn tcp_server_and_udp_mapping() {
        let mut a = base();
        a.tcp_listen = Some(9000);
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!(c.transport.as_deref(), Some("tcp-server"));
        assert_eq!(c.tcp_port, Some(9000));
        assert_eq!(c.tcp_host, None); // 空 host -> core 绑 0.0.0.0
        a = base();
        a.udp = Some(5000);
        let c = to_port_config(&a).unwrap().unwrap();
        assert_eq!(c.transport.as_deref(), Some("udp"));
        assert_eq!(c.udp_local_port, Some(5000));
    }

    #[test]
    fn no_source_returns_none() {
        assert!(to_port_config(&base()).unwrap().is_none());
    }

    #[test]
    fn append_newline_rules() {
        assert!(append_newline(&base()));
        let mut a = base();
        a.no_newline = true;
        assert!(!append_newline(&a));
        a.newline = true; // --newline 显式优先
        assert!(append_newline(&a));
    }

    #[test]
    fn clap_parses_monitor_flags() {
        let cli = Cli::try_parse_from([
            "bytetide", "monitor", "-p", "COM3", "--baud", "9600", "--ts", "--retry", "3",
        ])
        .unwrap();
        let Command::Monitor(a) = cli.command else {
            panic!("应为 monitor 子命令");
        };
        assert_eq!(a.port.as_deref(), Some("COM3"));
        assert_eq!(a.baud, 9600);
        assert!(a.ts);
        assert_eq!(a.retry, 3);
    }

    #[test]
    fn clap_sources_mutually_exclusive() {
        assert!(Cli::try_parse_from(["bytetide", "monitor", "-p", "COM3", "--tcp", "h:1"]).is_err());
        assert!(Cli::try_parse_from(["bytetide", "monitor", "--tcp", "h:1", "--udp", "5"]).is_err());
    }

    #[test]
    fn clap_parses_list() {
        assert!(matches!(
            Cli::try_parse_from(["bytetide", "list"]).unwrap().command,
            Command::List
        ));
    }

    #[test]
    fn clap_rejects_bad_enums() {
        assert!(Cli::try_parse_from(["bytetide", "monitor", "-p", "COM3", "--parity", "x"]).is_err());
        assert!(Cli::try_parse_from(["bytetide", "monitor", "-p", "COM3", "--data", "9"]).is_err());
    }
}
