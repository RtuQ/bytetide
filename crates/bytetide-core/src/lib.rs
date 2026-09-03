//! ByteTide 核心：串口/网络会话管理（ring 拉模型 + 后端规则评估 + TSV 落盘）。
//! 零 Tauri 依赖——桌面端（src-tauri）与 bytetide-cli 共用；宿主经 [`sink::EventSink`]
//! 接收状态/错误/告警事件（GUI 实现转发为 Tauri emit，CLI 实现写 stderr/日志）。

pub mod logfmt;
pub mod serial;
pub mod session;
pub mod sink;
