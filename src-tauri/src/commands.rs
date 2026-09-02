use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};

use crate::bridge::{BridgeConfig, BridgeConfigPatch, BridgeController};
use crate::logfmt::LogConfig;
use crate::serial::manager::{
    BridgeAlert, BridgeAnnotation, BridgeBookmark, PlotConfig, SendMode, SendRequest,
};
use crate::serial::port::{list_ports, LogLine, PortConfig, PortInfo};
use crate::state::AppState;

#[tauri::command]
pub fn list_ports_cmd() -> Vec<PortInfo> {
    list_ports()
}

#[tauri::command]
pub fn connect_cmd(
    config: PortConfig,
    log_settings: LogConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    state
        .manager
        .connect(config, log_settings, &app)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn disconnect_cmd(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.manager.disconnect(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn send_cmd(
    session_id: String,
    mode: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mode = match mode.as_str() {
        "hex" => SendMode::Hex,
        _ => SendMode::Ascii,
    };
    state
        .manager
        .send(&session_id, SendRequest { mode, text })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_log_cmd(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.manager.clear_log(&session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_log_path_cmd(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .manager
        .session_log_path(&session_id)
        .map_err(|e| e.to_string())
}

/// 将前端给出的日志文本写入用户通过文件对话框选择的路径。
#[tauri::command]
pub fn export_text_cmd(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// 前端性能诊断旁路落盘：把哨兵产生的单条诊断记录追加到
/// app_data/perf-frontend.log。前端调用方不 await、失败静默--
/// 这是“前端自己给自己取证”的通道，卡顿中事件循环仍活着时可用；
/// 完全死透时由后端 perf-heartbeat.log 兜底记录后端视角。
#[tauri::command]
pub fn append_perf_diag_cmd(app: AppHandle, kind: String, session_id: String, lag_ms: u64, batch_ms: u64, lines: u64, vis: String) -> Result<(), String> {
    use std::io::Write as _;
    let Ok(dir) = app.path().app_data_dir() else { return Ok(()) };
    let _ = std::fs::create_dir_all(&dir);
    let mut w = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("perf-frontend.log"))
        .map_err(|e| e.to_string())?;
    writeln!(
        w,
        "{} kind={} s={} lag={}ms batch={}ms n={} vis={}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        kind,
        session_id,
        lag_ms,
        batch_ms,
        lines,
        vis
    )
    .map_err(|e| e.to_string())
}

/// 前端推送实时规则（自动回复/告警）到会话：拉模型下评估在后端读线程。
#[tauri::command]
pub fn set_live_rules_cmd(
    session_id: String,
    auto_reply: crate::serial::rules::AutoReplyCfg,
    alerts: crate::serial::rules::AlertCfg,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .manager
        .set_live_rules(&session_id, auto_reply, alerts)
        .map_err(|e| e.to_string())
}

/// 视图拉模型数据通道：取 ring 中 `no > sinceNo` 的行（封顶 20000=ring 容量）。
/// `no` 单调递增且清屏不回退，游标语义下不重不漏。
#[tauri::command]
pub fn ring_lines_no_cmd(
    session_id: String,
    since_no: u64,
    max: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::serial::manager::BridgeLine>, String> {
    state
        .manager
        .ring_lines_after_no(&session_id, since_no, max.unwrap_or(5000))
        .map_err(|e| e.to_string())
}

/// 读取用户通过打开文件对话框选择的日志文件，返回 lossy UTF-8 文本（供离线分析）。
#[tauri::command]
pub fn read_text_file_cmd(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 创建离线日志会话：前端解析 .log 文件后把行推到后端 ring，供 REST 桥分析。返回 `o{N}` 会话 id。
#[tauri::command]
pub fn create_offline_session_cmd(
    config: PortConfig,
    path: String,
    lines: Vec<LogLine>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    Ok(state
        .manager
        .load_offline(config, PathBuf::from(path), lines))
}

// ===== REST 分析桥命令 =====

#[tauri::command]
pub fn bridge_get_config_cmd(bridge: State<'_, BridgeController>) -> BridgeConfig {
    bridge.get_config()
}

#[tauri::command]
pub fn bridge_set_config_cmd(
    patch: BridgeConfigPatch,
    bridge: State<'_, BridgeController>,
) -> BridgeConfig {
    bridge.set_config(&patch)
}

#[tauri::command]
pub fn bridge_regen_token_cmd(bridge: State<'_, BridgeController>) -> BridgeConfig {
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

/// 前端整包同步 AI 批注镜像（删除/清空批注时回写；REST 写入方向的镜像由 bridge.rs 维护）。
#[tauri::command]
pub fn bridge_sync_annotations_cmd(
    session_id: String,
    annotations: Vec<BridgeAnnotation>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state.manager.bridge_set_annotations(&session_id, annotations) {
        Ok(())
    } else {
        Err("会话不存在".into())
    }
}
