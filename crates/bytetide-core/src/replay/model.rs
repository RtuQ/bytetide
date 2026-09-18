//! 回放模型：配置（速度/循环/gap 钳制上限）、细粒度回放状态与控制命令。
//!
//! 速度约束 plan 指定：有限数值且 `0.1..=100.0`，非法在执行前失败
//! （[`ReplayConfig::validate`]，[`crate::serial::manager::PortManager::start_replay`]
//! 据此拒绝建会话）；控制通道的 [`ReplayCmd::SetSpeed`] 非法值静默忽略（维持原速）。
//! `max_gap_ms` 为相邻行**原始**时间差的钳制上限（默认 10_000）：实际睡眠 =
//! `min(Δ, max_gap_ms) / speed`，恒 ≤ `max_gap_ms / speed`（plan「10-second gap cap」）。

/// 相邻行时间差钳制上限默认值（plan V1 固定 10 秒）。
pub const DEFAULT_MAX_GAP_MS: u64 = 10_000;
/// 回放速度下限（含）。
pub const MIN_SPEED: f64 = 0.1;
/// 回放速度上限（含）。
pub const MAX_SPEED: f64 = 100.0;

/// 回放配置：`speed` 缩放相邻行延迟（delay=Δ/speed）、`looped`=EOF 后回到行 1
/// 循环、`max_gap_ms`=单段延迟的原始时间差上限。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplayConfig {
    pub speed: f64,
    pub looped: bool,
    pub max_gap_ms: u64,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            speed: 1.0,
            looped: false,
            max_gap_ms: DEFAULT_MAX_GAP_MS,
        }
    }
}

impl ReplayConfig {
    /// 执行前校验：speed 必须为有限数值且在 `MIN_SPEED..=MAX_SPEED`（含边界）。
    /// `max_gap_ms` 为 u64 无非法值（0 = 全程瞬放，交用户自担）。
    /// 错误文本走英文技术细节（经 manager 的 `replay_config_invalid|{detail}` 透传）。
    pub fn validate(&self) -> Result<(), String> {
        if !valid_speed(self.speed) {
            return Err(format!(
                "speed must be a finite number within {MIN_SPEED}..={MAX_SPEED}: {}",
                self.speed
            ));
        }
        Ok(())
    }
}

/// 速度合法性判定（配置校验与 SetSpeed 命令共用）：NaN/±inf/越界一律非法。
pub fn valid_speed(speed: f64) -> bool {
    speed.is_finite() && (MIN_SPEED..=MAX_SPEED).contains(&speed)
}

/// 细粒度回放状态：会话级 [`crate::serial::runtime::SessionStatus`] 之外的回放控制面
/// 状态。runner 写入 `SessionRuntime::replay_state`，宿主经 manager 查询面读取；
/// 会话级状态的映射见 runner 模块文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayState {
    /// 会话已建、回放线程尚未起跑（start_replay 到 spawn 之间的瞬态）
    Ready,
    /// 按时间差重放中
    Running,
    /// 用户暂停（虚拟时钟冻结；Resume 从当下重算下一行延迟）
    Paused,
    /// 到达文件尾且未循环（线程驻留：可被 SeekLine 复活，Stop/通道关闭退出）
    Finished,
    /// 用户停止 / 控制通道关闭（终态；disconnect 走 Stop 即此）
    Stopped,
    /// 源文件页读 IO 错误（终态；会话状态置 error）
    Error,
}

impl ReplayState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReplayState::Ready => "ready",
            ReplayState::Running => "running",
            ReplayState::Paused => "paused",
            ReplayState::Finished => "finished",
            ReplayState::Stopped => "stopped",
            ReplayState::Error => "error",
        }
    }
}

/// 回放控制命令（mpsc 控制通道；runner 逐条排水，等待分段 ≤50ms 内响应）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReplayCmd {
    /// 暂停：立即停住（当前行延迟作废——deadline 丢弃，恢复时重算）
    Pause,
    /// 恢复：从恢复时刻重算下一行完整延迟（不补偿暂停期间的 wall-clock 迟到→无爆发）
    Resume,
    /// 跳到第 N 行（1 起、越界钳制）：ring 清屏（seq 不回退）、下一行立即入库；
    /// Finished 状态下复活为 Running
    SeekLine(u64),
    /// 调速（运行中/暂停中均可）：运行中把当前剩余等待按速度比例重标定；
    /// 非法值（NaN/越界）忽略
    SetSpeed(f64),
    /// 开关循环（EOF 到达时生效）
    SetLoop(bool),
    /// 停止：线程退出置 Stopped（会话状态照 live 惯例收尾 disconnected）
    Stop,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validate_bounds_and_non_finite() {
        // 合法：边界值与常规值
        for speed in [MIN_SPEED, 1.0, 42.5, MAX_SPEED] {
            assert!(
                ReplayConfig {
                    speed,
                    ..ReplayConfig::default()
                }
                .validate()
                .is_ok(),
                "{speed} 应合法"
            );
        }
        // 非法：0、越界、负数、非有限值
        for bad in [0.0, 0.099_999, 100.1, -1.0, f64::NAN, f64::INFINITY] {
            let err = ReplayConfig {
                speed: bad,
                ..ReplayConfig::default()
            }
            .validate()
            .expect_err("应拒绝");
            assert!(err.contains("speed must be"), "{err}");
        }
    }

    #[test]
    fn config_default_matches_plan() {
        let c = ReplayConfig::default();
        assert_eq!(c.speed, 1.0);
        assert!(!c.looped);
        assert_eq!(c.max_gap_ms, DEFAULT_MAX_GAP_MS);
        assert_eq!(DEFAULT_MAX_GAP_MS, 10_000);
    }

    #[test]
    fn valid_speed_mirrors_validate() {
        assert!(valid_speed(0.1) && valid_speed(100.0) && valid_speed(3.0));
        assert!(!valid_speed(f64::NAN) && !valid_speed(f64::NEG_INFINITY));
        assert!(!valid_speed(0.0) && !valid_speed(101.0));
    }

    #[test]
    fn state_and_cmd_strings_shapes() {
        assert_eq!(ReplayState::Ready.as_str(), "ready");
        assert_eq!(ReplayState::Running.as_str(), "running");
        assert_eq!(ReplayState::Paused.as_str(), "paused");
        assert_eq!(ReplayState::Finished.as_str(), "finished");
        assert_eq!(ReplayState::Stopped.as_str(), "stopped");
        assert_eq!(ReplayState::Error.as_str(), "error");
        // 命令可调试/比较（控制通道断言用）
        assert_eq!(ReplayCmd::SeekLine(3), ReplayCmd::SeekLine(3));
        assert_ne!(ReplayCmd::Pause, ReplayCmd::Resume);
        assert_eq!(format!("{:?}", ReplayCmd::SetSpeed(2.0)), "SetSpeed(2.0)");
    }
}
