//! 场景自动化（Stage 3）：v1 版本化场景模型、静态校验与预编译匹配器。
//! runner/report（Task 2）与桌面/CLI 适配（Task 3/5）挂在本模块后续子模块。
//!
//! serde 形状是契约，JSON 黄金样例在仓库根 `testdata/scenarios/`——改字段必须同步
//! fixture 与前端镜像类型（Task 4 的 `src/types/automation.ts`）。

pub mod matcher;
pub mod model;

pub use matcher::{
    compile_matcher, substitute, CompiledMatcher, LineMatcher, MatcherError, MatcherErrorCode,
    VariableError,
};
pub use model::{
    validate_scenario, CaptureSpec, PinDef, Scenario, ScenarioErrorCode, ScenarioStep,
    ScenarioValidationError, SendModeDef, ValidatedScenario, ValidatedStep, MAX_DELAY_WAIT_MS,
    MAX_EXECUTED_STEPS, MAX_NESTING_DEPTH, MAX_REPEAT_TIMES, SCENARIO_SCHEMA, SCENARIO_VERSION,
};
