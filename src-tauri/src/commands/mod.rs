//! Tauri 命令层：按职责分文件——
//! - `sessions.rs`：连接/ring 拉取/实时规则/录制/信号线/离线会话
//! - `files.rs`：文件读写/导出/现场捕获档案/性能诊断
//! - `bridge.rs`：REST 桥配置/令牌/前端镜像同步
//!
//! 本文件再导出全部命令，使 lib.rs 的 `generate_handler!` 列表与命令名零改动。

mod bridge;
mod files;
mod sessions;

pub use bridge::*;
pub use files::*;
pub use sessions::*;
