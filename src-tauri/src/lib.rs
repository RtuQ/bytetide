mod bridge;
mod commands;
mod gui_sink;
mod hotplug;
mod state;

use single_instance::SingleInstance;
use tauri::{Manager, Listener};

use crate::state::AppState;

/// Windows：显式退出进程级能效限流（EcoQoS）。
/// 后台/锁屏时系统默认把窗口化进程降入节能队列，IPC/事件派发会被推迟到
/// 数十秒级——长挂监控场景表现为“日志不实时”。StateMask=0 即禁用该限流。
#[cfg(windows)]
fn disable_power_throttling() -> bool {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, SetProcessInformation, ProcessPowerThrottling,
        PROCESS_POWER_THROTTLING_STATE,
    };
    const CURRENT_VERSION: u32 = 1;
    const EXECUTION_SPEED: u32 = 0x1;
    let mut info = PROCESS_POWER_THROTTLING_STATE {
        Version: CURRENT_VERSION,
        ControlMask: EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &mut info as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

#[cfg(not(windows))]
fn disable_power_throttling() -> bool {
    true
}

/// 后端诊断心跳：每 10s 把各 live 会话 ring 末行时间戳落后墙钟的毫秒数
/// 追加到 app_log_dir/perf-heartbeat.log。完全在后端线程，不依赖前端事件循环--
/// 前端卡死时仍能持续记录“后端视角的滞后”，事后直接看文件即可定位
/// （分界：后端滞后=读线程/IO 被节流；后端 0 滞后而前端滞后=WebView 积压）。
/// 启动即调用，内部同时完成 EcoQoS 限流的退出并把结果写进首行。
/// 诊断日志统一落 `app_log_dir`（Windows: %LOCALAPPDATA%\<id>\logs；
/// macOS: ~/Library/Logs/<id>；Linux: XDG state/<id>/logs）——机器本地
/// 数据不随 Roaming 漫游，符合平台惯例。超 5MB 打开时截断（诊断日志
/// 无需无限历史）。返回 None = 目录不可用，调用方静默放弃。
pub(crate) fn open_diag_log(app: &tauri::AppHandle, name: &str) -> Option<std::fs::File> {
    use std::io::Write as _;
    let dir = app.path().app_log_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    const CAP: u64 = 5 * 1024 * 1024;
    if std::fs::metadata(&path).map(|m| m.len() > CAP).unwrap_or(false) {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&path) {
            let _ = f.set_len(0);
            let _ = f.flush();
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

fn start_perf_heartbeat(app: &tauri::AppHandle) {
    use std::io::Write as _;

    let ecoqos_off = disable_power_throttling();
    let Some(mut w) = open_diag_log(app, "perf-heartbeat.log") else { return };
    let _ = writeln!(
        w,
        "# heartbeat start {} ecoqos_disabled={}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        ecoqos_off
    );

    let app = app.clone();
    std::thread::Builder::new()
        .name("perf-heartbeat".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let Some(state) = app.try_state::<AppState>() else { continue };
            let now = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
            for (id, lag_ms, len, rx_lines) in state.manager.perf_snapshot() {
                let _ = writeln!(w, "{now} s={id} lag={lag_ms}ms ring={len} rx={rx_lines}");
            }
            let _ = w.flush();
        })
        .expect("spawn perf-heartbeat thread");
}

/// macOS：无边框窗口（decorations:false）不走系统窗口框，四角是直角。
/// 给 contentView 的 layer 设圆角裁剪，并把窗口背景置透明，
/// 直角外区域透出桌面；窗口阴影仍由系统按内容形状绘制。
#[cfg(target_os = "macos")]
fn round_window_corners(window: &tauri::WebviewWindow) {
    use objc2::{class, msg_send, runtime::AnyObject};

    const CORNER_RADIUS: f64 = 16.0; // 比系统默认（约 10）更明显

    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    unsafe {
        let win = ns_window as *mut AnyObject;
        let content: *mut AnyObject = msg_send![win, contentView];
        if content.is_null() {
            return;
        }
        // contentView 走 layer-backed 渲染，圆角由 layer 裁剪生效
        let () = msg_send![content, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![content, layer];
        if layer.is_null() {
            return;
        }
        let () = msg_send![layer, setCornerRadius: CORNER_RADIUS];
        let () = msg_send![layer, setMasksToBounds: true];
        // 窗口背景透明，圆角外不再是窗口底色方块
        let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
        let () = msg_send![win, setBackgroundColor: clear];
        let () = msg_send![win, setOpaque: false];
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 单实例：若已有实例运行则静默退出；锁创建失败则放行，不阻断启动
    let instance = SingleInstance::new("serial_tool-single-instance-lock");
    if let Ok(ref i) = instance {
        if !i.is_single() {
            return;
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            manager: bytetide_core::serial::PortManager::new(),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            hotplug::start_hotplug(handle);
            start_perf_heartbeat(app.handle());
            // REST 分析桥：按持久化配置自启（默认关，需在前端启用）
            app.manage(bridge::BridgeController::init(app.handle().clone()));
            // macOS 无边框窗口补系统级圆角（仅主窗口，应用为单窗口结构）
            #[cfg(target_os = "macos")]
            if let Some(win) = app.get_webview_window("main") {
                round_window_corners(&win);
            }
            // 防启动白屏：窗口以 visible:false 创建（tauri.conf.json），
            // 前端挂载完成 emit app-ready 后才显示，首帧即完整 UI
            // （v2 listen 返回 EventId，监听器随应用存活，无需保活）
            #[cfg(not(target_os = "macos"))]
            if let Some(win) = app.get_webview_window("main") {
                let w = win.clone();
                app.listen("app-ready", move |_| {
                    let _ = w.show();
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_ports_cmd,
            commands::connect_cmd,
            commands::disconnect_cmd,
            commands::send_cmd,
            commands::clear_log_cmd,
            commands::session_log_path_cmd,
            commands::export_text_cmd,
            commands::append_perf_diag_cmd,
            commands::ring_lines_no_cmd,
            commands::set_live_rules_cmd,
            commands::read_text_file_cmd,
            commands::create_offline_session_cmd,
            commands::bridge_get_config_cmd,
            commands::bridge_set_config_cmd,
            commands::bridge_regen_token_cmd,
            commands::set_plot_config_cmd,
            commands::bridge_sync_bookmarks_cmd,
            commands::bridge_sync_alerts_cmd,
            commands::bridge_sync_annotations_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
