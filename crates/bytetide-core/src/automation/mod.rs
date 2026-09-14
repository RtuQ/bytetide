//! 场景自动化（Stage 3）：v1 版本化场景模型、静态校验与预编译匹配器（Task 1）、
//! 迭代式运行器与宿主抽象（Task 2 `runner`）、报告 JSON/JUnit 序列化（Task 2
//! `report`）；桌面/CLI 适配（Task 3/5）挂在本模块下游。
//!
//! serde 形状是契约，JSON 黄金样例在仓库根 `testdata/scenarios/`——改字段必须同步
//! fixture 与前端镜像类型（Task 4 的 `src/types/automation.ts`）。

pub mod matcher;
pub mod model;
pub mod report;
pub mod runner;

pub use matcher::{
    compile_matcher, substitute, CompiledMatcher, LineMatcher, MatcherError, MatcherErrorCode,
    VariableError,
};
pub use model::{
    count_executed_leaves, validate_scenario, CaptureSpec, MatcherInstantiateError,
    MatcherTemplate, PinDef, Scenario, ScenarioErrorCode, ScenarioStep, ScenarioValidationError,
    SendModeDef, ValidatedScenario, ValidatedStep, MAX_DELAY_WAIT_MS, MAX_EXECUTED_STEPS,
    MAX_NESTING_DEPTH, MAX_REPEAT_TIMES, SCENARIO_SCHEMA, SCENARIO_VERSION,
};
pub use report::{
    report_json, report_junit, ScenarioReport, ScenarioStatus, StepErrorInfo, StepReport,
    StepStatus,
};
pub use runner::{run_scenario, HostError, ScenarioHost, WAIT_BATCH_LINES, WAIT_POLL_SLICE_MS};
