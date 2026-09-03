//! core EventSink 的 GUI 实现：转发为 Tauri 事件（事件名/载荷形状与抽取前一致）。

use bytetide_core::serial::manager::BridgeAlert;
use bytetide_core::sink::EventSink;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusPayload {
    session_id: String,
    status: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ErrorPayload {
    session_id: String,
    error: String,
}

/// 告警命中事件载荷（稀疏：仅命中时发；通知与提示音在 UI 侧执行）
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AlertHitPayload {
    session_id: String,
    hits: Vec<BridgeAlert>,
}

pub struct GuiSink(pub AppHandle);

impl EventSink for GuiSink {
    fn status(&self, session_id: &str, status: &str) {
        let _ = self.0.emit(
            "session-status",
            StatusPayload {
                session_id: session_id.to_string(),
                status: status.to_string(),
            },
        );
    }

    fn error(&self, session_id: &str, error: &str) {
        let _ = self.0.emit(
            "session-error",
            ErrorPayload {
                session_id: session_id.to_string(),
                error: error.to_string(),
            },
        );
    }

    fn alert_hits(&self, session_id: &str, hits: Vec<BridgeAlert>) {
        let _ = self.0.emit(
            "alert-hit",
            AlertHitPayload {
                session_id: session_id.to_string(),
                hits,
            },
        );
    }
}
