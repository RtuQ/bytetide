use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, SerialPortInfo, SerialPortType, StopBits};

/// 串口连接配置，与前端 camelCase 字段对应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortConfig {
    pub name: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: String,
    pub flow_control: String,
    /// 数据源传输类型：None/"serial"=串口；"tcp-client"/"tcp-server"/"udp" 为网络源。
    /// 旧预设/桥配置缺省该字段时反序列化为 None，行为完全向后兼容。
    #[serde(default)]
    pub transport: Option<String>,
    /// tcp-client：目标主机；tcp-server / udp：监听地址（空则 0.0.0.0）
    #[serde(default)]
    pub tcp_host: Option<String>,
    /// tcp-client 目标端口 / tcp-server 监听端口
    #[serde(default)]
    pub tcp_port: Option<u16>,
    /// udp 本地监听端口
    #[serde(default)]
    pub udp_local_port: Option<u16>,
}

impl Default for PortConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            baud_rate: 115_200,
            data_bits: 8,
            parity: "none".into(),
            stop_bits: "1".into(),
            flow_control: "none".into(),
            transport: None,
            tcp_host: None,
            tcp_port: None,
            udp_local_port: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Rx,
    Tx,
}

/// 一条日志行（行号由前端分配，后端不携带，便于“清屏”后重新计数）。
/// epoch_millis 为该行生成的墙钟毫秒，前端用于计算与上一行的时间差（Δ）。
/// bytes 仅在该行含无效 UTF-8 字节时携带原始字节（避免与 text 冗余、零 IPC 开销用于纯 ASCII 行）；
/// 有效 UTF-8 行前端可用 TextEncoder().encode(text) 无损恢复，故省略 bytes。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts: String,
    pub dir: Dir,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
    pub epoch_millis: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub port_type: String,
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

pub fn list_ports() -> Vec<PortInfo> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(serial_info)
        .collect()
}

fn serial_info(p: SerialPortInfo) -> PortInfo {
    let (port_type, vendor, product, serial) = match p.port_type {
        SerialPortType::UsbPort(u) => {
            ("usb".to_string(), u.manufacturer, u.product, u.serial_number)
        }
        SerialPortType::PciPort => ("pci".to_string(), None, None, None),
        SerialPortType::BluetoothPort => ("bluetooth".to_string(), None, None, None),
        _ => ("unknown".to_string(), None, None, None),
    };
    PortInfo {
        name: p.port_name,
        port_type,
        vendor,
        product,
        serial,
    }
}

/// 在调用线程内打开串口（避免 Box<dyn SerialPort> 跨线程的 Send 不确定性）。
pub fn open_port(cfg: &PortConfig) -> Result<Box<dyn serialport::SerialPort>, serialport::Error> {
    let data_bits = match cfg.data_bits {
        5 => DataBits::Five,
        6 => DataBits::Six,
        7 => DataBits::Seven,
        _ => DataBits::Eight,
    };
    let parity = match cfg.parity.as_str() {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    };
    let stop_bits = match cfg.stop_bits.as_str() {
        "2" => StopBits::Two,
        _ => StopBits::One,
    };
    let flow = match cfg.flow_control.as_str() {
        "software" => FlowControl::Software,
        "hardware" => FlowControl::Hardware,
        _ => FlowControl::None,
    };
    serialport::new(&cfg.name, cfg.baud_rate)
        .data_bits(data_bits)
        .parity(parity)
        .stop_bits(stop_bits)
        .flow_control(flow)
        .timeout(std::time::Duration::from_millis(200))
        .open()
}
