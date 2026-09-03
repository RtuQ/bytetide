//! EventSink → mpsc 通道：读线程的状态/错误/告警事件转发给主循环统一处理。
//! 数据行不走这里——行只进 ring，由主循环按 no 游标拉取（与桌面端同一拉模型）。

use std::sync::{mpsc, Mutex};

use bytetide_core::serial::manager::BridgeAlert;
use bytetide_core::sink::EventSink;

/// 转发给主循环的宿主事件
#[derive(Debug)]
pub enum SinkEvent {
    Status(String),
    Error(String),
    Alerts(Vec<BridgeAlert>),
}

/// CLI 侧 EventSink：把事件塞进通道（mpsc::Sender 非 Sync，用 Mutex 包一层满足 trait 约束）
pub struct CliSink(Mutex<mpsc::Sender<SinkEvent>>);

impl CliSink {
    pub fn new(tx: mpsc::Sender<SinkEvent>) -> Self {
        Self(Mutex::new(tx))
    }
}

impl EventSink for CliSink {
    fn status(&self, _session_id: &str, status: &str) {
        if let Ok(tx) = self.0.lock() {
            let _ = tx.send(SinkEvent::Status(status.to_string()));
        }
    }
    fn error(&self, _session_id: &str, error: &str) {
        if let Ok(tx) = self.0.lock() {
            let _ = tx.send(SinkEvent::Error(error.to_string()));
        }
    }
    fn alert_hits(&self, _session_id: &str, hits: Vec<BridgeAlert>) {
        if let Ok(tx) = self.0.lock() {
            let _ = tx.send(SinkEvent::Alerts(hits));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_events_in_order() {
        let (tx, rx) = mpsc::channel();
        let sink = CliSink::new(tx);
        sink.status("s1", "connecting");
        sink.error("s1", "boom");
        sink.alert_hits(
            "s1",
            vec![BridgeAlert {
                id: "a1".into(),
                rule_id: "r1".into(),
                pattern: "ERR".into(),
                level: "err".into(),
                no: 2,
                ts: "t".into(),
                text: "ERR x".into(),
                at: 9,
            }],
        );
        drop(sink);
        assert!(matches!(rx.recv(), Ok(SinkEvent::Status(s)) if s == "connecting"));
        assert!(matches!(rx.recv(), Ok(SinkEvent::Error(m)) if m == "boom"));
        match rx.recv() {
            Ok(SinkEvent::Alerts(h)) => assert_eq!(h.len(), 1),
            other => panic!("预期 Alerts，收到 {other:?}"),
        }
        assert!(rx.recv().is_err()); // 通道应已排空
    }
}
