//! 会话命令：连接/断开/发送/清屏、信号线、ring 拉取、实时规则、录制/分段、离线会话。

use std::path::PathBuf;

use bytetide_core::logfmt::LogConfig;
use bytetide_core::serial::manager::{Pin, RingBounds, SendMode, SendRequest};
use bytetide_core::serial::port::{list_ports, LogLine, PortConfig, PortInfo};
use bytetide_core::serial::rules::{AlertCfg, AutoReplyCfg};
use tauri::{AppHandle, Manager, State};

use crate::gui_sink::GuiSink;
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
    // 默认录制目录：app_data_dir()/sessions（与抽取前行为一致，失败回退 ./logs）
    let sessions_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("logs"))
        .join("sessions");
    state
        .manager
        .connect(
            config,
            log_settings,
            std::sync::Arc::new(GuiSink(app)),
            sessions_dir,
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn disconnect_cmd(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let result = state
        .manager
        .disconnect(&session_id)
        .map_err(|e| e.to_string());
    // 回放会话镜像随会话移除（speed/looped 登记，见 commands/replay.rs）
    state.replays.forget(&session_id);
    result
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
    state
        .manager
        .clear_log(&session_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_signal_cmd(
    session_id: String,
    pin: String,
    level: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let pin = match pin.as_str() {
        "dtr" => Pin::Dtr,
        "rts" => Pin::Rts,
        other => return Err(format!("未知信号线: {other}")),
    };
    state
        .manager
        .set_signal(&session_id, pin, level)
        .map_err(|e| e.to_string())
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

/// 日志分段：关闭当前日志文件，从当前时刻另起带时间戳的新文件继续落盘。
/// 返回新文件完整路径。
#[tauri::command]
pub fn rotate_log_cmd(session_id: String, state: State<'_, AppState>) -> Result<String, String> {
    state
        .manager
        .rotate_log(&session_id)
        .map_err(|e| e.to_string())
}

/// 落盘录制开关：false=暂停写日志文件；true=另起新分段文件继续录制。
#[tauri::command]
pub fn set_recording_cmd(
    session_id: String,
    on: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .manager
        .set_recording(&session_id, on)
        .map_err(|e| e.to_string())
}

/// 前端推送实时规则（自动回复/告警/现场捕获）到会话：拉模型下评估在后端读线程。
#[tauri::command]
pub fn set_live_rules_cmd(
    session_id: String,
    auto_reply: AutoReplyCfg,
    alerts: AlertCfg,
    capture: bytetide_core::serial::rules::CaptureCfg,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .manager
        .set_live_rules(&session_id, auto_reply, alerts, capture)
        .map_err(|e| e.to_string())
}

/// 视图拉模型数据通道：取 ring 中 `no > sinceNo` 的行（封顶 100000=ring 容量）。
/// `no` 单调递增且清屏不回退，游标语义下不重不漏。
#[tauri::command]
pub fn ring_lines_no_cmd(
    session_id: String,
    since_no: u64,
    max: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<bytetide_core::serial::manager::BridgeLine>, String> {
    state
        .manager
        .ring_lines_after_no(&session_id, since_no, max.unwrap_or(5000))
        .map_err(|e| e.to_string())
}

/// 往前翻页补拉：取 ring 中 `no < beforeNo` 的最新 max 行（升序）。
/// 视图缓冲裁掉旧行后用户上滑回看时，从 ring 回补仍存活的旧行。
#[tauri::command]
pub fn ring_lines_before_cmd(
    session_id: String,
    before_no: u64,
    max: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<bytetide_core::serial::manager::BridgeLine>, String> {
    state
        .manager
        .ring_lines_before_no(&session_id, before_no, max.unwrap_or(2000))
        .map_err(|e| e.to_string())
}

/// ring 现存行号边界（空环全 0）：前端判断「上滑还有没有旧行可回补」。
#[tauri::command]
pub fn ring_bounds_cmd(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<RingBounds, String> {
    state
        .manager
        .ring_bounds(&session_id)
        .map_err(|e| e.to_string())
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

/// 流式打开离线日志会话的结果（plan Task 8 形状：{sessionId,lineCount,firstEpoch,lastEpoch}）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineOpenResult {
    pub session_id: String,
    pub line_count: u64,
    pub first_epoch: u64,
    pub last_epoch: u64,
}

/// 流式打开离线日志：core 一次顺序扫描建稀疏索引（每 4096 数据行记字节偏移），
/// ring 恒空，不经前端全量传输、不把文件灌进 ring。之后用 `offline_lines_after_cmd`
/// 按页拉取（行 no=文件内第 N 个数据行，1 起；游标语义与 ring 拉取一致）。
/// 旧路径 `read_text_file_cmd` + `create_offline_session_cmd` 保留一个发布周期。
#[tauri::command]
pub fn open_offline_session_cmd(
    path: String,
    config: PortConfig,
    state: State<'_, AppState>,
) -> Result<OfflineOpenResult, String> {
    let (session_id, index) = state
        .manager
        .load_offline_indexed(config, PathBuf::from(&path))
        .map_err(|e| e.to_string())?;
    Ok(OfflineOpenResult {
        session_id,
        line_count: index.line_count,
        first_epoch: index.first_epoch,
        last_epoch: index.last_epoch,
    })
}

/// 离线会话分页拉取：取 `no > sinceNo` 的前 max 行（升序；core 按页直读源文件）。
/// 与 `ring_lines_no_cmd` 同一游标语义——后端内部就是同一个 manager 查询面。
#[tauri::command]
pub fn offline_lines_after_cmd(
    session_id: String,
    since_no: u64,
    max: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<bytetide_core::serial::manager::BridgeLine>, String> {
    state
        .manager
        .ring_lines_after_no(&session_id, since_no, max.unwrap_or(5000))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    //! 命令层形状冻结：OfflineOpenResult 的 serde camelCase 键与 plan Task 8 对齐。
    use super::OfflineOpenResult;

    #[test]
    fn offline_open_result_serializes_camel_case() {
        let v = serde_json::to_value(OfflineOpenResult {
            session_id: "o1".into(),
            line_count: 200_001,
            first_epoch: 1_000,
            last_epoch: 86_399_999,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "sessionId": "o1",
                "lineCount": 200001,
                "firstEpoch": 1000,
                "lastEpoch": 86399999
            })
        );
    }
}
