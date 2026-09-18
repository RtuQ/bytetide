//! 回放会话命令层（Stage 3 Task 7）：打开回放 / 控制面（pause/resume/seek/speed/
//! loop/stop）/ 状态查询。tauri 壳只差 State 提取与 `replay-state` 事件 emit——
//! 纯逻辑（action/value 解析、控制投递+到位等待、视图组装）拆为可测函数，
//! 集成测试 tests/replay_commands.rs 与命令走同一路径。
//!
//! Sender 持有方式：manager 的 SessionHandle.replay_tx 已持有控制通道（T6），
//! 命令层经 `manager.replay_control` 投递，不另存 sender；speed/looped 当前值
//! runner 线程内维护、manager 不暴露，AppState.replays 以 ReplayRegistry 镜像
//! （open 登记、SetSpeed/SetLoop 更新、disconnect/replay_status 失败时遗忘）。
//!
//! 事件 `replay-state`：plan 指定仅在状态/控制变化时发、不逐行——control 命令
//! 执行后 emit 一次（载荷=ReplayView，含当前文件行号水位）；EOF/Error 等
//! 无控制命令的状态变化由前端 ReplayControls 的 status 轮询兜底。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytetide_core::replay::{valid_speed, ReplayCmd, ReplayConfig, ReplayState};
use bytetide_core::serial::manager::PortManager;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::gui_sink::GuiSink;
use crate::state::AppState;

use super::errors::cmd_err;

/// 控制命令到位等待上限：runner 命令排水 ≤50ms（SLICE_MS），500ms 富余充足；
/// 超时按当前真实状态返回视图，不报错（状态查询面是唯一真相）。
const SETTLE_CAP: Duration = Duration::from_millis(500);

/// 回放控制面视图（camelCase 对齐前端 ReplayView；兼作 replay-state 事件载荷）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayView {
    pub session_id: String,
    /// 细粒度回放状态（core ReplayState::as_str）
    pub state: String,
    pub speed: f64,
    pub looped: bool,
    /// 当前文件行号水位（最后已 ingest 的源文件行；seek 后未恢复=目标-1）
    pub line: u64,
}

/// 打开回放会话的返回（plan Task 7 形状：{sessionId,lineCount,durationMs}）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayOpenView {
    pub session_id: String,
    pub line_count: u64,
    /// 源文件首末行 epoch 差（毫秒）。口径说明：离线索引的 epoch 为「当日毫秒或
    /// 行序回退」，跨午夜日志会低估——与回放调度所用的相邻 epoch 差同源同偏差，
    /// 仅作时长展示。
    pub duration_ms: u64,
}

/// 回放会话的 speed/looped 镜像（runner 线程内状态，manager 查询面不暴露）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayMeta {
    pub speed: f64,
    pub looped: bool,
}

/// 回放会话镜像登记表（AppState 持有；id → speed/looped）。
#[derive(Default)]
pub struct ReplayRegistry {
    inner: parking_lot::Mutex<HashMap<String, ReplayMeta>>,
}

impl ReplayRegistry {
    pub fn track(&self, id: &str, meta: ReplayMeta) {
        self.inner.lock().insert(id.to_string(), meta);
    }

    pub fn get(&self, id: &str) -> Option<ReplayMeta> {
        self.inner.lock().get(id).copied()
    }

    pub fn set_speed(&self, id: &str, speed: f64) {
        if let Some(m) = self.inner.lock().get_mut(id) {
            m.speed = speed;
        }
    }

    pub fn set_looped(&self, id: &str, looped: bool) {
        if let Some(m) = self.inner.lock().get_mut(id) {
            m.looped = looped;
        }
    }

    pub fn forget(&self, id: &str) {
        self.inner.lock().remove(id);
    }
}

/// 控制动作解析：action/value → ReplayCmd。非法 action/value 一律 Err 稳定
/// `code|detail`（speed 越界/NaN、seek 非「≥1 整数」、loop 非 0/1）；中文句子由
/// 前端词典 `errors.<code>` 承担，detail 携带原值或 "missing value"。
pub fn parse_replay_action(action: &str, value: Option<f64>) -> Result<ReplayCmd, String> {
    match action {
        "pause" => Ok(ReplayCmd::Pause),
        "resume" => Ok(ReplayCmd::Resume),
        "stop" => Ok(ReplayCmd::Stop),
        "seek" => {
            let v = value.ok_or_else(|| cmd_err("replay_seek_invalid", "missing value"))?;
            if !v.is_finite() || v < 1.0 || v.fract() != 0.0 {
                return Err(cmd_err("replay_seek_invalid", v));
            }
            Ok(ReplayCmd::SeekLine(v as u64))
        }
        "speed" => {
            let v = value.ok_or_else(|| cmd_err("replay_speed_invalid", "missing value"))?;
            if !valid_speed(v) {
                return Err(cmd_err("replay_speed_invalid", v));
            }
            Ok(ReplayCmd::SetSpeed(v))
        }
        "loop" => {
            let v = value.ok_or_else(|| cmd_err("replay_loop_invalid", "missing value"))?;
            if v == 0.0 {
                Ok(ReplayCmd::SetLoop(false))
            } else if v == 1.0 {
                Ok(ReplayCmd::SetLoop(true))
            } else {
                Err(cmd_err("replay_loop_invalid", v))
            }
        }
        other => Err(cmd_err("unknown_replay_action", other)),
    }
}

/// 控制投递 + 到位等待 + 镜像更新（命令层与集成测试共用路径）。
/// 等待目标仅取「确实会迁移/推进」的组合：
/// - Pause(Running→Paused)、Resume(Paused→Running)、Stop(→Stopped) 等状态迁移；
/// - SeekLine：等 runner 把行号水位推到 目标-1（seek 本身同步写水位；运行中若
///   下一行已抢先入库则以 line≥目标 判定）——Paused 下 seek 由此获得确定性视图；
/// - Finished 下的 SeekLine 额外等复活为 Running。
///
/// 其余（暂停中的状态保持、speed/loop）不等待。等待超时按当前真实状态返回，
/// 不报错（状态查询面是唯一真相）。Stop 幂等：已 Stopped（通道已关）直接返回。
pub fn apply_replay_control(
    manager: &PortManager,
    replays: &ReplayRegistry,
    id: &str,
    cmd: ReplayCmd,
) -> Result<(), String> {
    let before = manager.replay_view(id).map(|(s, _)| s);
    if cmd == ReplayCmd::Stop && before == Some(ReplayState::Stopped) {
        return Ok(());
    }
    manager.replay_control(id, cmd).map_err(|e| e.to_string())?;
    match cmd {
        ReplayCmd::SeekLine(n) => {
            wait_seek_applied(manager, id, n, SETTLE_CAP);
            if before == Some(ReplayState::Finished) {
                wait_for_state(manager, id, ReplayState::Running, SETTLE_CAP);
            }
        }
        ReplayCmd::Pause if before == Some(ReplayState::Running) => {
            wait_for_state(manager, id, ReplayState::Paused, SETTLE_CAP);
        }
        ReplayCmd::Resume if before == Some(ReplayState::Paused) => {
            wait_for_state(manager, id, ReplayState::Running, SETTLE_CAP);
        }
        ReplayCmd::Stop => wait_for_state(manager, id, ReplayState::Stopped, SETTLE_CAP),
        _ => {}
    }
    match cmd {
        ReplayCmd::SetSpeed(v) => replays.set_speed(id, v),
        ReplayCmd::SetLoop(b) => replays.set_looped(id, b),
        _ => {}
    }
    Ok(())
}

fn wait_for_state(manager: &PortManager, id: &str, want: ReplayState, cap: Duration) {
    let start = Instant::now();
    while manager.replay_view(id).map(|(s, _)| s) != Some(want) {
        if start.elapsed() >= cap {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// seek 到位等待：水位到 目标-1（seek 同步写）或运行中已 ingest 过目标行。
fn wait_seek_applied(manager: &PortManager, id: &str, target: u64, cap: Duration) {
    let watermark = target.saturating_sub(1);
    let start = Instant::now();
    loop {
        if let Some((state, line)) = manager.replay_view(id) {
            if line == watermark || (state == ReplayState::Running && line >= target) {
                return;
            }
        }
        if start.elapsed() >= cap {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// 组装回放控制面视图：manager 查询面（状态+行号水位）+ 镜像（speed/looped）。
/// 会话不存在 / 非回放会话 → Err `replay_session_not_found|`（前端 status 轮询据此
/// 静默停拉）。
pub fn build_view(
    manager: &PortManager,
    replays: &ReplayRegistry,
    id: &str,
) -> Result<ReplayView, String> {
    let (state, line) = manager
        .replay_view(id)
        .ok_or_else(|| cmd_err("replay_session_not_found", ""))?;
    let meta = replays.get(id).unwrap_or(ReplayMeta {
        speed: 1.0,
        looped: false,
    });
    Ok(ReplayView {
        session_id: id.to_string(),
        state: state.as_str().to_string(),
        speed: meta.speed,
        looped: meta.looped,
        line,
    })
}

/// 打开时序回放会话：core start_replay（open_offline 建索引 + spawn 回放线程），
/// 镜像登记后 emit 一次 replay-state（open 本身即状态创建；回放线程起跑即
/// connected，会话级 session-status 照常上报）。速度非法/文件缺失 → Err 稳定文案。
#[tauri::command]
pub fn open_replay_session_cmd(
    path: String,
    speed: f64,
    looped: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ReplayOpenView, String> {
    let config = ReplayConfig {
        speed,
        looped,
        ..ReplayConfig::default()
    };
    let sink = Arc::new(GuiSink(app.clone()));
    // sessions_dir：回放无落盘/无捕获（core 预留参数），传空
    let (session_id, _tx, index) = state
        .manager
        .start_replay_indexed(&PathBuf::from(&path), config, sink, PathBuf::new())
        .map_err(|e| e.to_string())?;
    state
        .replays
        .track(&session_id, ReplayMeta { speed, looped });
    let view = build_view(&state.manager, &state.replays, &session_id)?;
    let _ = app.emit("replay-state", &view);
    Ok(ReplayOpenView {
        session_id,
        line_count: index.line_count,
        duration_ms: index.last_epoch.saturating_sub(index.first_epoch),
    })
}

/// 回放控制：action/value 校验 → 投递 + 到位等待 → 视图回传 + emit replay-state。
#[tauri::command]
pub fn replay_control_cmd(
    session_id: String,
    action: String,
    value: Option<f64>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ReplayView, String> {
    let cmd = parse_replay_action(&action, value)?;
    apply_replay_control(&state.manager, &state.replays, &session_id, cmd)?;
    let view = build_view(&state.manager, &state.replays, &session_id)?;
    let _ = app.emit("replay-state", &view);
    Ok(view)
}

/// 回放状态查询（前端轮询兜底；查询失败顺手遗忘镜像防泄漏）。
#[tauri::command]
pub fn replay_status_cmd(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<ReplayView, String> {
    let view = build_view(&state.manager, &state.replays, &session_id);
    if view.is_err() {
        state.replays.forget(&session_id);
    }
    view
}

#[cfg(test)]
mod tests {
    //! 命令层形状冻结：ReplayView/ReplayOpenView 的 serde camelCase 键与 plan Task 7 对齐。
    use super::*;

    #[test]
    fn replay_views_serialize_camel_case() {
        let v = serde_json::to_value(ReplayView {
            session_id: "r1".into(),
            state: "running".into(),
            speed: 2.5,
            looped: true,
            line: 42,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "sessionId": "r1",
                "state": "running",
                "speed": 2.5,
                "looped": true,
                "line": 42
            })
        );
        let o = serde_json::to_value(ReplayOpenView {
            session_id: "r2".into(),
            line_count: 200_001,
            duration_ms: 86_399_999,
        })
        .unwrap();
        assert_eq!(
            o,
            serde_json::json!({
                "sessionId": "r2",
                "lineCount": 200001,
                "durationMs": 86399999
            })
        );
    }
}
