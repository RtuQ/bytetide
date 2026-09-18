//! 文件命令：文本导出/读取、性能诊断落盘、现场捕获档案（列出/删除/目录）。

use std::io::Write as _;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use super::errors::cmd_err;

/// 现场档案目录（与 connect_cmd 的 sessions_dir 同源：app_data_dir()/sessions/captures）
fn captures_dir_of(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("logs"))
        .join("sessions")
        .join("captures")
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureMeta {
    pub file_name: String,
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// 列出现场档案（sessions/captures 下的 .log，按修改时间倒序）。
#[tauri::command]
pub fn list_captures_cmd(app: AppHandle) -> Vec<CaptureMeta> {
    let dir = captures_dir_of(&app);
    let mut out: Vec<CaptureMeta> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("log") || !p.is_file() {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            let modified_ms = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.push(CaptureMeta {
                file_name: p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                path: p.to_string_lossy().into_owned(),
                size: md.len(),
                modified_ms,
            });
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.modified_ms));
    out
}

/// 现场档案目录绝对路径（前端「打开目录」用；前端无法自行解析 app_data_dir）。
#[tauri::command]
pub fn captures_dir_cmd(app: AppHandle) -> String {
    captures_dir_of(&app).to_string_lossy().into_owned()
}

/// 删除单个现场档案。守卫：只允许删 captures 目录内的 .log（防误删任意文件）。
#[tauri::command]
pub fn delete_capture_cmd(app: AppHandle, path: String) -> Result<(), String> {
    let dir = captures_dir_of(&app)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let p = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !p.starts_with(&dir) || p.extension().and_then(|x| x.to_str()) != Some("log") {
        return Err(cmd_err("path_outside_captures", ""));
    }
    std::fs::remove_file(&p).map_err(|e| e.to_string())
}

/// 将前端给出的日志文本写入用户通过文件对话框选择的路径。
#[tauri::command]
pub fn export_text_cmd(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// 读取用户通过打开文件对话框选择的日志文件，返回 lossy UTF-8 文本（供离线分析）。
#[tauri::command]
pub fn read_text_file_cmd(path: String) -> Result<String, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 前端性能诊断旁路落盘：把哨兵产生的单条诊断记录追加到
/// app_log_dir/perf-frontend.log。前端调用方不 await、失败静默--
/// 这是“前端自己给自己取证”的通道，卡顿中事件循环仍活着时可用；
/// 完全死透时由后端 perf-heartbeat.log 兜底记录后端视角。
#[tauri::command]
pub fn append_perf_diag_cmd(
    app: AppHandle,
    kind: String,
    session_id: String,
    lag_ms: u64,
    batch_ms: u64,
    lines: u64,
    vis: String,
) -> Result<(), String> {
    let Some(mut w) = crate::open_diag_log(&app, "perf-frontend.log") else {
        return Ok(());
    };
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
