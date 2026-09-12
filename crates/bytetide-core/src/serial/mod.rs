//! 串口/网络数据源子系统：配置、会话管理（ring 拉模型）、规则评估、落盘与现场捕获。
//! 热插拔监听属 GUI 关注点，留在桌面端 crate。
//!
//! Stage 2 Task 3 起按职责拆分（`docs/superpowers/plans/2026-09-11-stage-2-architecture-modularization.md`）：
//! - [`transport`]：链路抽象（`Transport` trait + `open_transport` 分发；串口/TCP/UDP）
//! - [`recording`]：落盘录制（开/写/分段/暂停/午夜轮转）
//! - [`capture`]：触发式现场捕获（行车记录仪）
//! - [`ring`]：环形缓冲与桥数据类型（拉模型唯一数据真相）
//! - [`runtime`]：会话运行态（状态机 + 规则评估入库）与读线程主循环
//! - [`manager`]：编排层（建 runtime → 开链路 → spawn 线程 → 路由命令 → 查询访问器）
//!
//! 跨模块共享的 DTO（桥镜像/会话快照/绘图配置）与发送请求/读线程命令类型集中在本文件。

pub mod capture;
pub mod manager;
pub mod port;
pub mod recording;
pub mod ring;
pub mod rules;
pub mod runtime;
pub mod transport;

pub use manager::PortManager;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use port::PortConfig;
use transport::Pin;

/// 墙钟毫秒（行时间戳 / 告警窗口 / 捕获窗口共用）。
pub(crate) fn now_ms() -> u64 {
    chrono::Local::now().timestamp_millis() as u64
}

/// 绘图/解码配置（与前端 `PlotConfig` camelCase 对齐；供 REST 桥 `/decode` 复用文法）。
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlotConfig {
    pub enabled: bool,
    pub source: String,
    pub frame_head: String,
    pub frame_tail: String,
    pub checksum: String,
    pub channels: u32,
    pub bytes_per_channel: u32,
    pub endian: String,
    pub signed: bool,
    pub max_points: u32,
}

impl Default for PlotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: "binary".into(),
            frame_head: String::new(),
            frame_tail: String::new(),
            checksum: "none".into(),
            channels: 2,
            bytes_per_channel: 2,
            endian: "big".into(),
            signed: false,
            max_points: 2000,
        }
    }
}

/// 会话列表项（REST `/sessions`）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnap {
    pub id: String,
    pub config: PortConfig,
    pub status: String,
    /// 最近一次错误消息（读线程经共享状态上报；无错误为 null）。
    pub last_error: Option<String>,
    pub line_count: usize,
    pub ring_cap: usize,
}

/// REST 桥书签条目（前端推送的只读镜像；`no` 为前端 UI 行号，与后端 `no` 体系无关，
/// 携带行文本/时间戳便于 AI 侧经 `?q=` 反查后端 `no`）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeBookmark {
    pub no: u64,
    pub ts: String,
    pub text: String,
}

/// REST 桥告警历史条目（前端推送的只读镜像，环形 100 条、新的在前；`no` 为前端 UI 行号）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAlert {
    pub id: String,
    pub rule_id: String,
    pub pattern: String,
    pub level: String,
    pub no: u64,
    pub ts: String,
    pub text: String,
    pub at: u64,
}

/// AI 批注（REST 写入，事件推送到前端界面；`no` 为行号——
/// 桥与 UI 行号在会话内 1:1，除非用户清屏重计数）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeAnnotation {
    pub id: String,
    pub no: u64,
    pub ts: String,
    pub text: String,
    pub note: String,
    pub at: u64,
}

pub enum SendMode {
    Ascii,
    Hex,
}

pub struct SendRequest {
    pub mode: SendMode,
    pub text: String,
}

/// 发往读线程的控制命令：发送数据 / 清屏（截断文件）/ 信号线控制。
pub enum PortCmd {
    Send(SendRequest),
    Clear,
    /// 日志落盘控制（录制开关/分段共用）：携带新分段完整路径，读线程内
    /// flush+关闭当前文件后从该路径另起新文件继续录制。分段= manager 计算好
    /// 带时间戳的路径后发此命令；恢复录制=同上。
    RecOn(PathBuf),
    /// 暂停落盘：flush+关闭当前文件（ring/视图不受影响，仅停止写文件）。
    RecOff,
    /// DTR/RTS 置位：串口链路直接写引脚，网络源在链路层报“无信号线”。
    Signal {
        pin: Pin,
        level: bool,
    },
}
