use crate::commands::ReplayRegistry;

use bytetide_core::serial::PortManager;

/// Tauri 全局状态：持有串口会话管理器与回放会话镜像登记表。
pub struct AppState {
    pub manager: PortManager,
    /// 回放会话 speed/looped 镜像（runner 线程内状态 manager 不暴露，见
    /// commands/replay.rs；open 登记、SetSpeed/SetLoop 更新、disconnect 遗忘）
    pub replays: ReplayRegistry,
}
