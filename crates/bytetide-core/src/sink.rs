//! 宿主事件出口：core 不依赖任何 UI 框架，状态/错误/告警经此 trait 上报。
//! 桌面端实现转发为 Tauri 事件（session-status / session-error / alert-hit），
//! CLI 实现写 stderr 或静默。数据行不走这里——行只进 ring 与落盘文件，
//! 消费者按 `no` 游标拉取（拉模型，防 IPC 洪水）。

use serde::Serialize;

use crate::serial::manager::BridgeAlert;
use parking_lot::Mutex;

/// 现场捕获档案落成信息（稀疏事件；前端据此刷新档案列表）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureInfo {
    /// 档案文件完整路径
    pub path: String,
    /// 触发方式："keyword" | "alert" | "disconnect"
    pub trigger: String,
    /// 触发规则文本（keyword=规则 pattern；alert=告警 pattern；disconnect=「断连」）
    pub rule: String,
    /// 档案内的数据行数（不含头注释）
    pub lines: u64,
    /// 触发时刻 epoch ms
    pub at: u64,
}

pub trait EventSink: Send + Sync + 'static {
    /// 会话生命周期状态：connecting / connected / disconnected / error。
    fn status(&self, session_id: &str, status: &str);
    /// 会话级错误（打开失败、读写错误、断开）。低频，可同步处理。
    fn error(&self, session_id: &str, error: &str);
    /// 告警命中（稀疏上报；通知/提示音等表现层行为由宿主决定）。
    fn alert_hits(&self, session_id: &str, hits: Vec<BridgeAlert>);
    /// 现场捕获档案落成（极稀疏：一次触发一条）。
    fn capture_saved(&self, session_id: &str, info: CaptureInfo);
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
    fn capture_saved(&self, session_id: &str, info: CaptureInfo) {
        self.0.lock().push(format!(
            "capture {session_id} {} n={}",
            info.path, info.lines
        ));
    }
}

/// 静默丢弃所有事件（CLI 不需要事件通道时使用）。
pub struct NullSink;

impl EventSink for NullSink {
    fn status(&self, _session_id: &str, _status: &str) {}
    fn error(&self, _session_id: &str, _error: &str) {}
    fn alert_hits(&self, _session_id: &str, _hits: Vec<BridgeAlert>) {}
    fn capture_saved(&self, _session_id: &str, _info: CaptureInfo) {}
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
