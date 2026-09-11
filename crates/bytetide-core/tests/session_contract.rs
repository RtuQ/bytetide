//! 序列化特征化测试（Stage 2 Task 1，docs/superpowers/plans/2026-09-11-stage-2-architecture-modularization.md）：
//! 冻结后端跨边界 DTO 的**线上 JSON 字节形状**——后续大重构（Task 2 ring/runtime 抽取、
//! Task 3 传输/录制/捕获拆分、Task 4 bridge 拆分、Task 8 离线分页）不许改变这些字节。
//! 断言用「固定值 serde_json::to_string ⇔ 完整字面量」的 assert_eq：键序=struct 字段
//! 声明序，rename_all=camelCase，`Option` 是否省略按实际行为冻结（BridgeLine 的
//! bytes/match 带 skip_serializing_if、SessionSnap 的 lastError 与 PortConfig 的
//! transport/tcp* 字段**不带**——None 也序列化为 null）。任何 rename/增删字段都会在
//! 这里红掉，提醒同步前端镜像（src/__tests__/ipc-contract.test.ts 同源字面量；
//! Task 7 起共享 testdata/protocol/ fixtures）。
//!
//! Tauri 事件载荷形状在 src-tauri/src/gui_sink.rs（core 测试不能反向依赖桌面壳），
//! 在此等价记录、由前端契约测试真正断言：
//! - `session-status` → `{"sessionId":"<id>","status":"<connecting|connected|disconnected|error>"}`
//! - `session-error`  → `{"sessionId":"<id>","error":"<消息>"}`

use bytetide_core::serial::manager::{BridgeLine, MatchHit, RingBounds, SessionSnap};
use bytetide_core::serial::port::{Dir, PortConfig};

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("characterization value must serialize")
}

#[test]
fn bridge_line_rx_with_bytes_and_match_freezes_full_shape() {
    let line = BridgeLine {
        no: 7,
        ts: "12:00:00.000".into(),
        dir: Dir::Rx,
        text: "AA 55 01".into(),
        bytes: Some(vec![0xaa, 0x55, 0x01]),
        epoch_millis: 1_727_000_000_000,
        r#match: Some(MatchHit {
            offset: 0,
            length: 2,
            field: "head".into(),
        }),
    };
    assert_eq!(
        to_json(&line),
        "{\"no\":7,\"ts\":\"12:00:00.000\",\"dir\":\"rx\",\"text\":\"AA 55 01\",\
         \"bytes\":[170,85,1],\"epochMillis\":1727000000000,\
         \"match\":{\"offset\":0,\"length\":2,\"field\":\"head\"}}"
    );
}

#[test]
fn bridge_line_tx_without_bytes_and_match_omits_both_keys() {
    let line = BridgeLine {
        no: 8,
        ts: "12:00:00.001".into(),
        dir: Dir::Tx,
        text: "hello".into(),
        bytes: None,
        epoch_millis: 1_727_000_000_001,
        r#match: None,
    };
    // bytes / match 键整体消失（skip_serializing_if）——前端不得假设键恒在
    assert_eq!(
        to_json(&line),
        "{\"no\":8,\"ts\":\"12:00:00.001\",\"dir\":\"tx\",\"text\":\"hello\",\
         \"epochMillis\":1727000000001}"
    );
}

#[test]
fn session_snap_serial_serializes_default_option_fields_as_null() {
    let snap = SessionSnap {
        id: "s1".into(),
        config: PortConfig {
            name: "COM3".into(),
            ..Default::default()
        },
        status: "connected".into(),
        last_error: None,
        line_count: 42,
        ring_cap: 100_000,
    };
    // PortConfig 的 transport/tcpHost/tcpPort/udpLocalPort 无 skip_serializing_if：
    // None 冻结为显式 null（旧预设向后兼容靠 serde(default) 反序列化侧）
    assert_eq!(
        to_json(&snap),
        "{\"id\":\"s1\",\"config\":{\"name\":\"COM3\",\"baudRate\":115200,\
         \"dataBits\":8,\"parity\":\"none\",\"stopBits\":\"1\",\"flowControl\":\"none\",\
         \"transport\":null,\"tcpHost\":null,\"tcpPort\":null,\"udpLocalPort\":null},\
         \"status\":\"connected\",\"lastError\":null,\"lineCount\":42,\"ringCap\":100000}"
    );
}

#[test]
fn session_snap_network_source_freezes_transport_fields() {
    let snap = SessionSnap {
        id: "s2".into(),
        config: PortConfig {
            name: "tcp:127.0.0.1:9000".into(),
            transport: Some("tcp-client".into()),
            tcp_host: Some("127.0.0.1".into()),
            tcp_port: Some(9000),
            ..Default::default()
        },
        status: "error".into(),
        last_error: Some("连接被拒绝".into()),
        line_count: 0,
        ring_cap: 100_000,
    };
    assert_eq!(
        to_json(&snap),
        "{\"id\":\"s2\",\"config\":{\"name\":\"tcp:127.0.0.1:9000\",\"baudRate\":115200,\
         \"dataBits\":8,\"parity\":\"none\",\"stopBits\":\"1\",\"flowControl\":\"none\",\
         \"transport\":\"tcp-client\",\"tcpHost\":\"127.0.0.1\",\"tcpPort\":9000,\
         \"udpLocalPort\":null},\"status\":\"error\",\"lastError\":\"连接被拒绝\",\
         \"lineCount\":0,\"ringCap\":100000}"
    );
}

#[test]
fn ring_bounds_freezes_shape() {
    let bounds = RingBounds {
        first_no: 3,
        last_no: 102,
        size: 100,
        ring_cap: 100_000,
    };
    assert_eq!(
        to_json(&bounds),
        "{\"firstNo\":3,\"lastNo\":102,\"size\":100,\"ringCap\":100000}"
    );
}

#[test]
fn port_config_deserializes_legacy_json_without_transport_fields() {
    // 旧预设/桥配置缺 transport/tcp* 字段：serde(default) 反序列化为 None（向后兼容红线）
    let cfg: PortConfig = serde_json::from_str(
        "{\"name\":\"/dev/ttyUSB0\",\"baudRate\":9600,\"dataBits\":8,\
          \"parity\":\"even\",\"stopBits\":\"2\",\"flowControl\":\"hardware\"}",
    )
    .expect("legacy JSON must deserialize");
    assert_eq!(cfg.name, "/dev/ttyUSB0");
    assert_eq!(cfg.baud_rate, 9600);
    assert_eq!(cfg.parity, "even");
    assert_eq!(cfg.stop_bits, "2");
    assert_eq!(cfg.flow_control, "hardware");
    assert_eq!(cfg.transport, None);
    assert_eq!(cfg.tcp_host, None);
    assert_eq!(cfg.tcp_port, None);
    assert_eq!(cfg.udp_local_port, None);
}

#[test]
fn port_config_roundtrips_through_frozen_json() {
    let cfg = PortConfig {
        name: "COM3".into(),
        ..Default::default()
    };
    let json = to_json(&cfg);
    let back: PortConfig = serde_json::from_str(&json).expect("frozen JSON must roundtrip");
    assert_eq!(back.name, "COM3");
    assert_eq!(back.baud_rate, 115_200);
    assert_eq!(back.transport, None);
}
