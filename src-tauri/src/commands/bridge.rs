//! REST 分析桥命令：桥配置/令牌 + 前端→后端镜像同步（绘图文法/书签/告警/批注）。

use bytetide_core::serial::manager::{BridgeAlert, BridgeAnnotation, BridgeBookmark, PlotConfig};
use tauri::State;

use crate::bridge::{BridgeConfigPatch, BridgeController, BridgeView};
use crate::state::AppState;

#[tauri::command]
pub fn bridge_get_config_cmd(bridge: State<'_, BridgeController>) -> BridgeView {
    bridge.get_view()
}

#[tauri::command]
pub fn bridge_set_config_cmd(
    patch: BridgeConfigPatch,
    bridge: State<'_, BridgeController>,
) -> Result<BridgeView, String> {
    bridge.set_config(&patch)
}

#[tauri::command]
pub fn bridge_regen_token_cmd(bridge: State<'_, BridgeController>) -> Result<BridgeView, String> {
    bridge.regen_token()
}

/// 前端绘图配置变更时同步到后端（供 REST `/decode` 复用文法）；失败静默。
#[tauri::command]
pub fn set_plot_config_cmd(
    session_id: String,
    config: PlotConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.manager.bridge_set_plot(&session_id, config);
    Ok(())
}

/// 前端推送书签快照到后端镜像（REST `/bookmarks` 只读）；会话已关时返回 Err，前端静默。
#[tauri::command]
pub fn bridge_sync_bookmarks_cmd(
    session_id: String,
    bookmarks: Vec<BridgeBookmark>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.manager.bridge_set_bookmarks(&session_id, bookmarks) {
        Ok(())
    } else {
        Err("会话不存在".into())
    }
}

/// 前端推送告警历史到后端镜像（REST `/alerts` 只读）；会话已关时返回 Err，前端静默。
#[tauri::command]
pub fn bridge_sync_alerts_cmd(
    session_id: String,
    alerts: Vec<BridgeAlert>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.manager.bridge_set_alerts(&session_id, alerts) {
        Ok(())
    } else {
        Err("会话不存在".into())
    }
}

/// 前端整包同步 AI 批注镜像（删除/清空批注时回写；REST 写入方向的镜像由 bridge 模块维护）。
#[tauri::command]
pub fn bridge_sync_annotations_cmd(
    session_id: String,
    annotations: Vec<BridgeAnnotation>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state
        .manager
        .bridge_set_annotations(&session_id, annotations)
    {
        Ok(())
    } else {
        Err("会话不存在".into())
    }
}
