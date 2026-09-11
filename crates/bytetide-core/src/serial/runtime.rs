//! 会话运行态：状态机事实源（`SessionStatus`/`SessionState`）与每会话共享单元
//! `SessionRuntime`（ring + 运行状态 + 前端推送规则配置五件套）。
//! Stage 2 Task 2 自 manager.rs 原样迁出；`serial::manager` 经再导出保持旧路径一个发布周期。
//! 带 sink 事件的状态上报薄壳（`set_status`/`set_error` + 事件）仍留在 manager 侧——
//! 本任务不把 sink 事件逻辑搬进 runtime（T3 的 `ingest(origin, sink)` 统一收敛）。

use std::sync::Arc;

use parking_lot::RwLock;
use serde::Serialize;

use super::port::LogLine;
use super::ring::RingBuf;
use super::rules::{AlertCfg, AutoReplyCfg, CaptureCfg};

/// 会话运行状态：REST 快照（bridge_list）与 sink 事件的共同事实源（serde 小写，
/// 与既有 REST status 字符串一致）。读线程写、REST 读，挂在 SessionRuntime.state。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// 已创建、读线程建链中（开串口/建链完成前）
    #[default]
    Connecting,
    /// 链路就绪，读循环运行中
    Connected,
    /// 正常收尾（用户停止/对端关闭/停止标志退出）
    Disconnected,
    /// 终止性错误（开串口失败/建链失败/读硬错误）
    Error,
    /// 离线加载的日志会话（无链路）
    Offline,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionStatus::Connecting => "connecting",
            SessionStatus::Connected => "connected",
            SessionStatus::Disconnected => "disconnected",
            SessionStatus::Error => "error",
            SessionStatus::Offline => "offline",
        }
    }
}

/// 每会话共享运行状态（读线程写，REST 读）。
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub status: SessionStatus,
    pub last_error: Option<String>,
}

/// 每会话运行态共享单元：ring（拉模型数据真相）+ 运行状态 + 前端推送的规则配置。
/// SessionHandle 持 `Arc<SessionRuntime>`，读线程与 REST 桥经它共享同一批 Arc。
pub struct SessionRuntime {
    pub ring: Arc<RingBuf>,
    pub state: Arc<RwLock<SessionState>>,
    /// 自动回复规则（前端推送；读线程内评估并直接回写设备）
    pub auto_reply: Arc<RwLock<AutoReplyCfg>>,
    /// 告警规则（前端推送；读线程内评估，命中走 mirror + alert-hit 事件）
    pub alerts: Arc<RwLock<AlertCfg>>,
    /// 触发式现场捕获配置（前端推送；读线程内逐行评估触发）
    pub capture: Arc<RwLock<CaptureCfg>>,
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRuntime {
    pub fn new() -> Self {
        Self {
            ring: Arc::new(RingBuf::new()),
            state: Arc::new(RwLock::new(SessionState::default())),
            auto_reply: Arc::new(RwLock::new(AutoReplyCfg::default())),
            alerts: Arc::new(RwLock::new(AlertCfg::default())),
            capture: Arc::new(RwLock::new(CaptureCfg::default())),
        }
    }

    /// 单行入库（分配单调 `no`、更新方向计数、超容淘汰）：本任务仅包装 `RingBuf::push`，
    /// 录制/规则/捕获副作用仍由 manager 的读循环负责（T3 收敛为 `ingest(origin, sink)`）。
    pub fn ingest(&self, line: &LogLine) -> u64 {
        self.ring.push(line)
    }

    /// 仅写共享状态，不发事件（manager 侧薄壳保持「先写状态后 emit」的事件顺序）。
    pub fn set_status(&self, status: SessionStatus) {
        self.state.write().status = status;
    }

    /// 仅记录 last_error，不发事件、不迁移状态。
    pub fn set_error(&self, message: impl Into<String>) {
        self.state.write().last_error = Some(message.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::port::Dir;

    fn mk_log(dir: Dir, text: &str, epoch: u64) -> LogLine {
        LogLine {
            ts: "t".into(),
            dir,
            text: text.into(),
            bytes: None,
            epoch_millis: epoch,
        }
    }

    #[test]
    fn ingest_wraps_ring_push_with_monotonic_no_and_counters() {
        let rt = SessionRuntime::new();
        assert_eq!(rt.state.read().status, SessionStatus::Connecting);
        let no1 = rt.ingest(&mk_log(Dir::Rx, "x", 1));
        let no2 = rt.ingest(&mk_log(Dir::Tx, "y", 2));
        assert_eq!((no1, no2), (1, 2));
        assert_eq!(rt.ring.len(), 2);
        assert_eq!(rt.ring.rx_lines(), 1);
        assert_eq!(rt.ring.tx_lines(), 1);
        assert_eq!(rt.ring.last_no(), 2);
    }

    #[test]
    fn set_status_and_set_error_write_state_without_events() {
        // 无 sink 版本仅写共享状态（事件薄壳在 manager 侧，此处断言不触碰事件通道）
        let rt = SessionRuntime::new();
        rt.set_status(SessionStatus::Connected);
        rt.set_error("boom");
        let st = rt.state.read();
        assert_eq!(st.status, SessionStatus::Connected);
        assert_eq!(st.last_error.as_deref(), Some("boom"));
    }
}
