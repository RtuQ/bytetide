//! 桥服务面：`BridgeService` trait + 生产适配器 `ManagerBridgeService`。
//!
//! - trait 收敛全部 manager 访问：路由 handler 只依赖 `Arc<dyn BridgeService>`，
//!   router 可脱离 Tauri 构造（集成测试与本文件回归测试注入 fake 实现）。
//! - `ManagerBridgeService` 是**唯一**读 Tauri `AppState` 的适配器（包 `AppHandle`），
//!   并承担两路前端回推事件（`bridge-annotations-updated` / `bridge-plot-updated`，
//!   载荷形状与抽取前一致）。
//! - `/follow` 长轮询由路由层用 `lines_after` + `last_no` 组合实现，trait 不设 follow。

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::error::ServiceError;
use crate::state::AppState;
use bytetide_core::serial::manager::{
    BridgeAlert, BridgeAnnotation, BridgeBookmark, BridgeLine, BridgeStats, PlotConfig,
    SendRequest, SessionSnap,
};

/// 桥对会话数据的访问面：把 manager 访问从 handler 抽出。
/// pub：集成测试（tests/bridge_routes.rs）注入 fake service 构造 router。
pub trait BridgeService: Send + Sync + 'static {
    fn list_sessions(&self) -> Vec<SessionSnap>;
    fn session(&self, id: &str) -> Result<SessionSnap, ServiceError>;
    fn stats(&self, id: &str) -> Result<BridgeStats, ServiceError>;
    fn snapshot(&self, id: &str) -> Result<Vec<BridgeLine>, ServiceError>;
    /// `no` 之后（不含）的行，至多 `max` 条（生产侧钳到 ring 容量，一轮必取全增量）。
    fn lines_after(&self, id: &str, no: u64, max: usize) -> Result<Vec<BridgeLine>, ServiceError>;
    /// 按行号精确读单行（会话缺失 Err；行不存在 `Ok(None)`；`no=0` 恒 None）。
    /// `/lines?no=` 与批注回填的有界读取面（评审 P1-1：替代全量 snapshot）。
    fn line_by_no(&self, id: &str, no: u64) -> Result<Option<BridgeLine>, ServiceError>;
    fn last_no(&self, id: &str) -> Result<u64, ServiceError>;
    fn log_path(&self, id: &str) -> Result<PathBuf, ServiceError>;
    fn plot(&self, id: &str) -> Result<PlotConfig, ServiceError>;
    fn set_plot(&self, id: &str, config: PlotConfig) -> Result<(), ServiceError>;
    fn bookmarks(&self, id: &str) -> Result<Vec<BridgeBookmark>, ServiceError>;
    fn alerts(&self, id: &str) -> Result<Vec<BridgeAlert>, ServiceError>;
    fn annotations(&self, id: &str) -> Result<Vec<BridgeAnnotation>, ServiceError>;
    fn set_annotations(&self, id: &str, values: Vec<BridgeAnnotation>) -> Result<(), ServiceError>;
    fn send(&self, id: &str, request: SendRequest) -> Result<(), ServiceError>;
    /// 批注变化后推送前端界面（生产 = `bridge-annotations-updated` 事件；测试 no-op）。
    fn notify_annotations_changed(&self, session_id: &str, annotations: &[BridgeAnnotation]);
    /// 绘图文法写回后通知前端实时采纳（生产 = `bridge-plot-updated` 事件；测试 no-op）。
    fn notify_plot_updated(&self, session_id: &str, config: &PlotConfig);
}

/// 桥写回批注后推送前端的事件载荷。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnnotationsPayload {
    pub session_id: String,
    pub annotations: Vec<BridgeAnnotation>,
}

/// 桥写回绘图文法后通知前端实时采纳的事件载荷。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlotUpdatedPayload {
    pub session_id: String,
    pub config: PlotConfig,
}

/// 生产实现：全部转发到 Tauri managed state 里的 `PortManager`。
/// 整个 bridge 模块中唯一持有 `AppHandle` 并读 `AppState` 的类型。
pub struct ManagerBridgeService {
    app: AppHandle,
}

impl ManagerBridgeService {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl BridgeService for ManagerBridgeService {
    fn list_sessions(&self) -> Vec<SessionSnap> {
        self.app.state::<AppState>().manager.bridge_list()
    }

    fn session(&self, id: &str) -> Result<SessionSnap, ServiceError> {
        self.list_sessions()
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(ServiceError::NotFound)
    }

    fn stats(&self, id: &str) -> Result<BridgeStats, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_stats(id)
            .ok_or(ServiceError::NotFound)
    }

    fn snapshot(&self, id: &str) -> Result<Vec<BridgeLine>, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_snapshot(id)
            .ok_or(ServiceError::NotFound)
    }

    fn lines_after(&self, id: &str, no: u64, max: usize) -> Result<Vec<BridgeLine>, ServiceError> {
        // max 内部钳到 [1, RING_CAP]：传 usize::MAX 即「取全增量」（与抽取前
        // bridge_follow 的无上限语义一致——ring 本身不可能超过 RING_CAP 行）。
        self.app
            .state::<AppState>()
            .manager
            .ring_lines_after_no(id, no, max)
            .map_err(|e| ServiceError::Backend(e.to_string()))
    }

    fn line_by_no(&self, id: &str, no: u64) -> Result<Option<BridgeLine>, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_line_by_no(id, no)
            .map_err(|e| ServiceError::Backend(e.to_string()))
    }

    fn last_no(&self, id: &str) -> Result<u64, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_last_no(id)
            .ok_or(ServiceError::NotFound)
    }

    fn log_path(&self, id: &str) -> Result<PathBuf, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .session_log_path(id)
            .map(PathBuf::from)
            .map_err(|e| ServiceError::Backend(e.to_string()))
    }

    fn plot(&self, id: &str) -> Result<PlotConfig, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_plot(id)
            .ok_or(ServiceError::NotFound)
    }

    fn set_plot(&self, id: &str, config: PlotConfig) -> Result<(), ServiceError> {
        if self
            .app
            .state::<AppState>()
            .manager
            .bridge_set_plot(id, config)
        {
            Ok(())
        } else {
            Err(ServiceError::NotFound)
        }
    }

    fn bookmarks(&self, id: &str) -> Result<Vec<BridgeBookmark>, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_bookmarks(id)
            .ok_or(ServiceError::NotFound)
    }

    fn alerts(&self, id: &str) -> Result<Vec<BridgeAlert>, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_alerts(id)
            .ok_or(ServiceError::NotFound)
    }

    fn annotations(&self, id: &str) -> Result<Vec<BridgeAnnotation>, ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .bridge_annotations(id)
            .ok_or(ServiceError::NotFound)
    }

    fn set_annotations(&self, id: &str, values: Vec<BridgeAnnotation>) -> Result<(), ServiceError> {
        if self
            .app
            .state::<AppState>()
            .manager
            .bridge_set_annotations(id, values)
        {
            Ok(())
        } else {
            Err(ServiceError::NotFound)
        }
    }

    fn send(&self, id: &str, request: SendRequest) -> Result<(), ServiceError> {
        self.app
            .state::<AppState>()
            .manager
            .send(id, request)
            .map_err(|e| ServiceError::Backend(e.to_string()))
    }

    fn notify_annotations_changed(&self, session_id: &str, annotations: &[BridgeAnnotation]) {
        let _ = self.app.emit(
            "bridge-annotations-updated",
            AnnotationsPayload {
                session_id: session_id.to_string(),
                annotations: annotations.to_vec(),
            },
        );
    }

    fn notify_plot_updated(&self, session_id: &str, config: &PlotConfig) {
        let _ = self.app.emit(
            "bridge-plot-updated",
            PlotUpdatedPayload {
                session_id: session_id.to_string(),
                config: config.clone(),
            },
        );
    }
}
