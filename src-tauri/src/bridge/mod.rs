//! REST 分析桥：进程内 axum server，供外部 AI CLI（经 skill + curl）读取/分析串口日志。
//!
//! - 无 MCP：纯 HTTP + Bearer token。
//! - 远程/虚拟机：绑定 `127.0.0.1`（默认）或 `0.0.0.0`（远程可达），URL+token 由用户复制到 AI 机。
//! - 数据源：manager 的每会话环形缓冲（带原始字节）。
//! - 访问面：`BridgeService` trait 收敛全部 manager 访问（唯一读 Tauri `AppState` 的
//!   适配器是 `service::ManagerBridgeService`），路由 handler 只依赖 trait，
//!   router 可脱离 Tauri 构造（生产注入 `ManagerBridgeService`；测试注入 fake service）。
//! - 安全：桥默认关；启用需 token；`/send`、`/exchange` 由 `allowSend` 独立门控。
//!
//! 模块划分：`server.rs` 控制器/配置/监听生命周期与 router 组装；`service.rs` 服务
//! trait 与生产适配器；`error.rs` REST 错误（`ApiError`）+ 服务层错误（`ServiceError`）；
//! `auth.rs` Bearer 中间件；`routes/` 按资源分组的 handler
//! （metadata / lines / analysis / annotations / exchange）。
//!
//! 对外路径兼容：以下 `pub use` 保持 `serial_tool_lib::bridge::*` 与抽取前一致
//! （router / BridgeService / BridgeConfig / BridgeView / BridgeController 等），
//! 集成测试与 lib.rs 均按旧路径引用。

pub mod auth;
pub mod error;
pub mod routes;
pub mod server;
pub mod service;

pub use error::{ApiError, ServiceError};
pub use server::{
    router, BridgeConfig, BridgeConfigError, BridgeConfigPatch, BridgeController, BridgeRuntime,
    BridgeView, RuntimeState,
};
pub use service::{BridgeService, ManagerBridgeService};
