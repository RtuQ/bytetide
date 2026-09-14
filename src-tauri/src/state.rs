use std::sync::Arc;

use crate::commands::{AutomationRegistry, ReplayRegistry};

use bytetide_core::serial::PortManager;

/// Tauri 全局状态：持有串口会话管理器与回放会话镜像登记表。
pub struct AppState {
    /// 串口会话管理器（Arc：场景运行线程的 host 需跨线程持有，见
    /// commands/automation.rs）
    pub manager: Arc<PortManager>,
    /// 回放会话 speed/looped 镜像（runner 线程内状态 manager 不暴露，见
    /// commands/replay.rs；open 登记、SetSpeed/SetLoop 更新、disconnect 遗忘）
    pub replays: ReplayRegistry,
    /// 场景运行登记表（runId → 运行态/报告/join，见 commands/automation.rs）
    pub automation: AutomationRegistry,
}
