//! 宿主事件出口：core 不依赖任何 UI 框架，状态/错误/告警经此 trait 上报。
//! 桌面端实现转发为 Tauri 事件（session-status / session-error / alert-hit），
//! CLI 实现写 stderr 或静默。数据行不走这里——行只进 ring 与落盘文件，
//! 消费者按 `no` 游标拉取（拉模型，防 IPC 洪水）。

use crate::serial::manager::BridgeAlert;
use parking_lot::Mutex;

pub trait EventSink: Send + Sync + 'static {
    /// 会话生命周期状态：connecting / connected / disconnected / error。
    fn status(&self, session_id: &str, status: &str);
    /// 会话级错误（打开失败、读写错误、断开）。低频，可同步处理。
    fn error(&self, session_id: &str, error: &str);
    /// 告警命中（稀疏上报；通知/提示音等表现层行为由宿主决定）。
    fn alert_hits(&self, session_id: &str, hits: Vec<BridgeAlert>);
}

/// 测试/无宿主场景的 EventSink：把事件按序收进 Vec 供断言。
#[derive(Default)]
pub struct VecSink(pub Mutex<Vec<String>>);

impl EventSink for VecSink {
    fn status(&self, session_id: &str, status: &str) {
        self.0.lock().push(format!("status {session_id} {status}"));
    }
    fn error(&self, session_id: &str, error: &str) {
        self.0.lock().push(format!("error {session_id} {error}"));
    }
    fn alert_hits(&self, session_id: &str, hits: Vec<BridgeAlert>) {
        self.0.lock().push(format!(
            "alert-hit {session_id} n={}",
            hits.len()
        ));
    }
}

/// 静默丢弃所有事件（CLI 不需要事件通道时使用）。
pub struct NullSink;

impl EventSink for NullSink {
    fn status(&self, _session_id: &str, _status: &str) {}
    fn error(&self, _session_id: &str, _error: &str) {}
    fn alert_hits(&self, _session_id: &str, _hits: Vec<BridgeAlert>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_sink_records_events_in_order() {
        let sink = VecSink::default();
        sink.status("s1", "connecting");
        sink.status("s1", "connected");
        sink.error("s1", "读取错误");
        sink.alert_hits("s1", vec![BridgeAlert {
            id: "a1".into(),
            rule_id: "r1".into(),
            pattern: "ERR".into(),
            level: "err".into(),
            no: 3,
            ts: "t".into(),
            text: "ERR".into(),
            at: 1,
        }]);
        assert_eq!(
            sink.0.lock().clone(),
            vec![
                "status s1 connecting".to_string(),
                "status s1 connected".to_string(),
                "error s1 读取错误".to_string(),
                "alert-hit s1 n=1".to_string(),
            ]
        );
    }
}
