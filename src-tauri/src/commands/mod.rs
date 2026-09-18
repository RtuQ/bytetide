//! Tauri 命令层：按职责分文件——
//! - `sessions.rs`：连接/ring 拉取/实时规则/录制/信号线/离线会话
//! - `files.rs`：文件读写/导出/现场捕获档案/性能诊断
//! - `bridge.rs`：REST 桥配置/令牌/前端镜像同步
//! - `replay.rs`：时序回放（打开/控制面/状态查询，Stage 3 Task 7）
//! - `automation.rs`：场景自动化（校验/启动/停止/状态/报告，Stage 3 Task 3）
//!
//! 本文件再导出全部命令，使 lib.rs 的 `generate_handler!` 列表与命令名零改动。

mod automation;
mod bridge;
mod errors;
mod files;
mod replay;
mod sessions;

pub use automation::*;
pub use bridge::*;
pub use files::*;
pub use replay::*;
pub use sessions::*;
