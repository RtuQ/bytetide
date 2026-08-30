use crate::serial::PortManager;

/// Tauri 全局状态：持有串口会话管理器。
pub struct AppState {
    pub manager: PortManager,
}
