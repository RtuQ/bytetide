//! 场景 v1 数据模型与静态校验（Stage 3 Task 1，plan：
//! `docs/superpowers/plans/2026-09-11-stage-3-automation-and-replay.md`）。
//!
//! # serde 形状即契约
//! `testdata/scenarios/*.json`（仓库根）是黄金样例——改任何字段/命名必须同步 fixture
//! 与前端镜像类型（Task 4 的 `src/types/automation.ts`）。enum 用 `kind` 内部 tag +
//! camelCase 字段（`appendNewline`/`timeoutMs`/`withinLast`），子枚举小写
//! （`ascii`/`hex`、`dtr`/`rts`、`rx`/`tx`）。
//!
//! # 校验入口
//! [`validate_scenario`] 一次性完成（先反序列化再校验，稳定错误在 validate 层报出）：
//! schema/version 门禁、name/steps 非空、变量名合法、限制（Repeat 嵌套 ≤
//! [`MAX_NESTING_DEPTH`] 层、执行步静态上界 ≤ [`MAX_EXECUTED_STEPS`]、
//! delay/wait ≤ [`MAX_DELAY_WAIT_MS`] ms、repeat ≤ [`MAX_REPEAT_TIMES`] 次）、
//! matcher 编译（恰一 pattern / regex 可编译 / hex/mask 严格解析——语义对齐
//! src-tauri bridge，实现在 [`crate::automation::matcher`]）、捕获组范围、变量引用
//! 可达性。成功返回 [`ValidatedScenario`]（保留模型 + 预编译 matcher 树，runner 免重编译）。
//!
//! # 稳定错误码（[`ScenarioErrorCode`]，GUI/CLI 按码分支，勿改字符串）
//! `invalid_schema` / `invalid_version` / `empty_name` / `empty_steps` /
//! `matcher_conflict` / `invalid_regex` / `invalid_hex` / `invalid_mask` /
//! `invalid_dir` / `nesting_too_deep` / `step_limit_exceeded` / `delay_too_long` /
//! `wait_too_long` / `repeat_too_many` / `invalid_variable_name` /
//! `undefined_variable` / `capture_group_invalid`。
//! `invalid_dir` 仅为错误码表完整性保留：`Dir` 是封闭枚举，未知 dir 在 serde
//! 反序列化层即被拒绝（`unknown variant`），走不到本层校验。
//! 错误携带索引路径（如 `steps[3].steps[1]`）与 message，`Display` 格式
//! `"{path}: {message} [code={code}]"`。
//!
//! # 变量替换与静态引用分析
//! 替换原语（`${name}` 语法、`$$`/非法名/无闭合 `}` 的字面量边界）见
//! [`crate::automation::matcher`]；仅 `Send.text` 与 `Assert.message` 参与替换与
//! 引用检查，matcher 模式不做替换（可字面匹配含 `${` 的设备输出）。
//!
//! 保守引用可达性（静态分析）：
//! - 直线块内严格按序：引用必须在初始 `variables` 或更早步骤 `Wait.save` 的变量集合内；
//! - Repeat 体放宽：体的可用集 = 外层可用集 ∪ 体内（含嵌套）任何 `Wait.save` 的变量
//!   全集——第 1 轮迭代引用后文捕获的变量、跨迭代引用、嵌套体先行引用都不误报；
//!   Repeat 结束后其体 saves 并入外层可用集（`times=0` 时变量实际未定义也放行）。
//!   理由：静态分析无法精确判定 Repeat 的迭代可达路径（V1 禁任意表达式），宁可
//!   漏报（运行期 [`substitute`] 仍会对未定义引用报错兜底）也不误报合法场景。
//!
//! 注意：`substitute`/`VariableError` 定义在 [`crate::automation::matcher`]（文本层
//! 原语与 matcher 同文件），本模块只做引用可达性校验。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::automation::matcher::{
    compile_matcher, find_refs, is_valid_var_name, CompiledMatcher, LineMatcher, MatcherError,
    MatcherErrorCode,
};

/// schema 标识，必须恰为该值。
pub const SCENARIO_SCHEMA: &str = "bytetide.scenario";
/// 唯一支持的 schema 版本。
pub const SCENARIO_VERSION: u32 = 1;
/// Repeat 嵌套层数上限（4 层 Repeat 允许，第 5 层报错）。
pub const MAX_NESTING_DEPTH: u32 = 4;
/// 执行步上限：静态叶子步 × repeat 展开上界估算，超限报错。
pub const MAX_EXECUTED_STEPS: u64 = 10_000;
/// Delay.ms 与 Wait.timeoutMs 的上限（毫秒）。
pub const MAX_DELAY_WAIT_MS: u64 = 600_000;
/// Repeat.times 上限。
pub const MAX_REPEAT_TIMES: u32 = 10_000;

/// 版本化场景（v1）。字段 serde 命名不变（全小写单词）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub schema: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    pub steps: Vec<ScenarioStep>,
}

/// 发送模式（serde 小写 `"ascii"`/`"hex"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SendModeDef {
    Ascii,
    Hex,
}

/// 信号线引脚（serde 小写 `"dtr"`/`"rts"`；不支持其他引脚）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinDef {
    Dtr,
    Rts,
}

/// Wait 命中后的变量捕获：`group=0` 整匹配，`>0` 为 regex 捕获组。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSpec {
    pub variable: String,
    pub group: usize,
}

/// 场景步骤（`kind` 内部 tag + camelCase 字段；变体字段命名必须与 fixture 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScenarioStep {
    #[serde(rename_all = "camelCase")]
    Send {
        mode: SendModeDef,
        text: String,
        append_newline: bool,
    },
    Delay {
        ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    Signal {
        pin: PinDef,
        level: bool,
    },
    #[serde(rename_all = "camelCase")]
    Wait {
        matcher: LineMatcher,
        timeout_ms: u64,
        #[serde(default)]
        save: Option<CaptureSpec>,
    },
    #[serde(rename_all = "camelCase")]
    Assert {
        matcher: LineMatcher,
        /// 语义「在最近 N 行内查找」；0 或超大不在本层限制（runner 层处理），
        /// 但 matcher 仍须编译通过。
        within_last: usize,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    Repeat {
        times: u32,
        steps: Vec<ScenarioStep>,
    },
}

/// 校验后的步骤：文本保留模板原样（运行期做变量替换），matcher 换成预编译版。
#[derive(Debug, Clone)]
pub enum ValidatedStep {
    Send {
        mode: SendModeDef,
        template: String,
        append_newline: bool,
    },
    Delay {
        ms: u64,
    },
    Signal {
        pin: PinDef,
        level: bool,
    },
    Wait {
        matcher: CompiledMatcher,
        timeout_ms: u64,
        save: Option<CaptureSpec>,
    },
    Assert {
        matcher: CompiledMatcher,
        within_last: usize,
        template: String,
    },
    Repeat {
        times: u32,
        steps: Vec<ValidatedStep>,
    },
}

/// 校验通过的场景：模型 + 预编译 matcher 树，runner 直接执行，免重编译。
#[derive(Debug, Clone)]
pub struct ValidatedScenario {
    pub name: String,
    pub variables: BTreeMap<String, String>,
    pub steps: Vec<ValidatedStep>,
}

/// 稳定错误码（字符串见模块注释，GUI/CLI 依赖字面量，勿改）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioErrorCode {
    InvalidSchema,
    InvalidVersion,
    EmptyName,
    EmptySteps,
    MatcherConflict,
    InvalidRegex,
    InvalidHex,
    InvalidMask,
    /// 保留码：`Dir` 封闭枚举在 serde 反序列化层拒绝未知变体，本层不可达。
    InvalidDir,
    NestingTooDeep,
    StepLimitExceeded,
    DelayTooLong,
    WaitTooLong,
    RepeatTooMany,
    InvalidVariableName,
    UndefinedVariable,
    CaptureGroupInvalid,
}

impl ScenarioErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSchema => "invalid_schema",
            Self::InvalidVersion => "invalid_version",
            Self::EmptyName => "empty_name",
            Self::EmptySteps => "empty_steps",
            Self::MatcherConflict => "matcher_conflict",
            Self::InvalidRegex => "invalid_regex",
            Self::InvalidHex => "invalid_hex",
            Self::InvalidMask => "invalid_mask",
            Self::InvalidDir => "invalid_dir",
            Self::NestingTooDeep => "nesting_too_deep",
            Self::StepLimitExceeded => "step_limit_exceeded",
            Self::DelayTooLong => "delay_too_long",
            Self::WaitTooLong => "wait_too_long",
            Self::RepeatTooMany => "repeat_too_many",
            Self::InvalidVariableName => "invalid_variable_name",
            Self::UndefinedVariable => "undefined_variable",
            Self::CaptureGroupInvalid => "capture_group_invalid",
        }
    }
}

/// 校验错误：稳定错误码 + 索引路径（如 `steps[3].steps[1]`；根字段为
/// `schema`/`version`/`name`/`steps`/`variables[".."]`，恒非空）+ message。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioValidationError {
    pub code: ScenarioErrorCode,
    pub path: String,
    pub message: String,
}

impl fmt::Display for ScenarioValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} [code={}]",
            self.path,
            self.message,
            self.code.as_str()
        )
    }
}

impl std::error::Error for ScenarioValidationError {}

/// 校验场景并预编译全部 matcher。一次 DFS 完成所有检查，首个错误按文档序返回。
pub fn validate_scenario(input: Scenario) -> Result<ValidatedScenario, ScenarioValidationError> {
    if input.schema != SCENARIO_SCHEMA {
        return Err(err(
            ScenarioErrorCode::InvalidSchema,
            "schema",
            format!("schema must be {SCENARIO_SCHEMA:?}, got {:?}", input.schema),
        ));
    }
    if input.version != SCENARIO_VERSION {
        return Err(err(
            ScenarioErrorCode::InvalidVersion,
            "version",
            format!(
                "unsupported scenario version {}, expected {SCENARIO_VERSION}",
                input.version
            ),
        ));
    }
    if input.name.is_empty() {
        return Err(err(
            ScenarioErrorCode::EmptyName,
            "name",
            "name must not be empty",
        ));
    }
    if input.steps.is_empty() {
        return Err(err(
            ScenarioErrorCode::EmptySteps,
            "steps",
            "steps must not be empty",
        ));
    }
    for name in input.variables.keys() {
        if !is_valid_var_name(name) {
            return Err(err(
                ScenarioErrorCode::InvalidVariableName,
                &format!("variables[{name:?}]"),
                format!("invalid variable name {name:?}: must match [A-Za-z_][A-Za-z0-9_]*"),
            ));
        }
    }
    // 直线可用集从初始 variables 起步，随 Wait.save / Repeat 保守并集增长。
    let mut avail: BTreeSet<String> = input.variables.keys().cloned().collect();
    let block = walk_block(&input.steps, "steps", 0, &mut avail)?;
    Ok(ValidatedScenario {
        name: input.name,
        variables: input.variables,
        steps: block.steps,
    })
}

fn err(code: ScenarioErrorCode, path: &str, message: impl Into<String>) -> ScenarioValidationError {
    ScenarioValidationError {
        code,
        path: path.to_string(),
        message: message.into(),
    }
}

fn matcher_error(path: &str, e: MatcherError) -> ScenarioValidationError {
    let code = match e.code {
        MatcherErrorCode::Conflict => ScenarioErrorCode::MatcherConflict,
        MatcherErrorCode::InvalidRegex => ScenarioErrorCode::InvalidRegex,
        MatcherErrorCode::InvalidHex => ScenarioErrorCode::InvalidHex,
        MatcherErrorCode::InvalidMask => ScenarioErrorCode::InvalidMask,
    };
    err(code, path, e.message)
}

/// 捕获组范围：group 0（整匹配）任何 matcher 都允许；>0 仅 regex 且须存在于模式中
/// （literal/hex/mask 没有捕获组）。
fn check_capture_group(
    compiled: &CompiledMatcher,
    group: usize,
    path: &str,
) -> Result<(), ScenarioValidationError> {
    // regex::captures_len 含隐式 group 0；非 regex 视为只有整匹配 1 组
    let groups = compiled.regex().map_or(1, |re| re.captures_len());
    if group >= groups {
        return Err(err(
            ScenarioErrorCode::CaptureGroupInvalid,
            path,
            format!("capture group {group} out of range: matcher exposes {groups} group(s) (group 0 = whole match)"),
        ));
    }
    Ok(())
}

/// 检查模板文本中的 `${name}` 引用全部可用。非法变量名的 `${...}` 序列不是引用
/// （按字面量保留），此处天然跳过。
fn check_refs(
    text: &str,
    path: &str,
    avail: &BTreeSet<String>,
) -> Result<(), ScenarioValidationError> {
    for (span, name) in find_refs(text) {
        if !avail.contains(name) {
            return Err(err(
                ScenarioErrorCode::UndefinedVariable,
                path,
                format!(
                    "undefined variable ${{{name}}} at byte offset {}: must be declared in `variables` or captured by an earlier Wait.save",
                    span.start
                ),
            ));
        }
    }
    Ok(())
}

/// 递归收集一个块（含嵌套 Repeat）内所有 Wait.save 的变量名。
/// 仅用于 Repeat 体的保守放宽；save 变量名的合法性由主遍历负责报错。
fn collect_saves(steps: &[ScenarioStep], out: &mut BTreeSet<String>) {
    for step in steps {
        match step {
            ScenarioStep::Wait {
                save: Some(spec), ..
            } => {
                out.insert(spec.variable.clone());
            }
            ScenarioStep::Repeat { steps, .. } => collect_saves(steps, out),
            _ => {}
        }
    }
}

struct BlockOut {
    steps: Vec<ValidatedStep>,
    /// 该块的执行步静态上界（叶子步 × 路径上 repeat 展开倍数）。
    expansion: u64,
}

/// 校验并编译一个线性步骤块。`block_path` 是块路径（顶层 `"steps"`，Repeat 体为
/// `"{repeat_path}.steps"`），步骤路径 = `{block_path}[{i}]`。
/// `avail` 直线累积：块内 Wait.save / Repeat 体 saves 会并入，调用方（Repeat 步骤）
/// 据此获得块后的保守可用集。
fn walk_block(
    steps: &[ScenarioStep],
    block_path: &str,
    depth: u32,
    avail: &mut BTreeSet<String>,
) -> Result<BlockOut, ScenarioValidationError> {
    let mut out = Vec::with_capacity(steps.len());
    let mut expansion: u64 = 0;
    for (i, step) in steps.iter().enumerate() {
        let path = format!("{block_path}[{i}]");
        match step {
            ScenarioStep::Send {
                mode,
                text,
                append_newline,
            } => {
                check_refs(text, &path, avail)?;
                expansion += 1;
                out.push(ValidatedStep::Send {
                    mode: *mode,
                    template: text.clone(),
                    append_newline: *append_newline,
                });
            }
            ScenarioStep::Delay { ms } => {
                if *ms > MAX_DELAY_WAIT_MS {
                    return Err(err(
                        ScenarioErrorCode::DelayTooLong,
                        &path,
                        format!("delay {ms} ms exceeds maximum {MAX_DELAY_WAIT_MS} ms"),
                    ));
                }
                expansion += 1;
                out.push(ValidatedStep::Delay { ms: *ms });
            }
            ScenarioStep::Signal { pin, level } => {
                expansion += 1;
                out.push(ValidatedStep::Signal {
                    pin: *pin,
                    level: *level,
                });
            }
            ScenarioStep::Wait {
                matcher,
                timeout_ms,
                save,
            } => {
                if *timeout_ms > MAX_DELAY_WAIT_MS {
                    return Err(err(
                        ScenarioErrorCode::WaitTooLong,
                        &path,
                        format!(
                            "wait timeoutMs {timeout_ms} exceeds maximum {MAX_DELAY_WAIT_MS} ms"
                        ),
                    ));
                }
                let compiled = compile_matcher(matcher).map_err(|e| matcher_error(&path, e))?;
                if let Some(spec) = save {
                    if !is_valid_var_name(&spec.variable) {
                        return Err(err(
                            ScenarioErrorCode::InvalidVariableName,
                            &path,
                            format!(
                                "invalid capture variable name {:?}: must match [A-Za-z_][A-Za-z0-9_]*",
                                spec.variable
                            ),
                        ));
                    }
                    check_capture_group(&compiled, spec.group, &path)?;
                }
                expansion += 1;
                out.push(ValidatedStep::Wait {
                    matcher: compiled,
                    timeout_ms: *timeout_ms,
                    save: save.clone(),
                });
                if let Some(spec) = save {
                    avail.insert(spec.variable.clone());
                }
            }
            ScenarioStep::Assert {
                matcher,
                within_last,
                message,
            } => {
                let compiled = compile_matcher(matcher).map_err(|e| matcher_error(&path, e))?;
                check_refs(message, &path, avail)?;
                expansion += 1;
                out.push(ValidatedStep::Assert {
                    matcher: compiled,
                    within_last: *within_last,
                    template: message.clone(),
                });
            }
            ScenarioStep::Repeat { times, steps } => {
                let inner_depth = depth + 1;
                if inner_depth > MAX_NESTING_DEPTH {
                    return Err(err(
                        ScenarioErrorCode::NestingTooDeep,
                        &path,
                        format!("repeat nesting depth {inner_depth} exceeds maximum {MAX_NESTING_DEPTH}"),
                    ));
                }
                if *times > MAX_REPEAT_TIMES {
                    return Err(err(
                        ScenarioErrorCode::RepeatTooMany,
                        &path,
                        format!("repeat times {times} exceeds maximum {MAX_REPEAT_TIMES}"),
                    ));
                }
                // 保守放宽（见模块注释）：体的可用集 = 当前可用集 ∪ 体内全部 Wait.save
                let body_saves = {
                    let mut saves = BTreeSet::new();
                    collect_saves(steps, &mut saves);
                    saves
                };
                let mut body_avail: BTreeSet<String> = avail.union(&body_saves).cloned().collect();
                let body = walk_block(
                    steps,
                    &format!("{path}.steps"),
                    inner_depth,
                    &mut body_avail,
                )?;
                avail.extend(body_saves); // 块后视为「可能已定义」（保守放行）
                expansion =
                    expansion.saturating_add(u64::from(*times).saturating_mul(body.expansion));
                out.push(ValidatedStep::Repeat {
                    times: *times,
                    steps: body.steps,
                });
            }
        }
    }
    if expansion > MAX_EXECUTED_STEPS {
        return Err(err(
            ScenarioErrorCode::StepLimitExceeded,
            block_path,
            format!("executed step upper bound {expansion} exceeds maximum {MAX_EXECUTED_STEPS}"),
        ));
    }
    Ok(BlockOut {
        steps: out,
        expansion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::port::Dir;

    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/scenarios/");

    fn load_fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{FIXTURE_DIR}{name}")).expect("fixture must exist")
    }

    fn parse_fixture(name: &str) -> Scenario {
        serde_json::from_str(&load_fixture(name)).expect("fixture must deserialize")
    }

    fn matcher_lit(dir: Option<Dir>, literal: &str) -> LineMatcher {
        LineMatcher {
            dir,
            literal: Some(literal.into()),
            regex: None,
            hex: None,
            mask: None,
        }
    }

    fn bare(dir: Option<Dir>) -> LineMatcher {
        LineMatcher {
            dir,
            literal: None,
            regex: None,
            hex: None,
            mask: None,
        }
    }

    fn send(text: &str) -> ScenarioStep {
        ScenarioStep::Send {
            mode: SendModeDef::Ascii,
            text: text.into(),
            append_newline: false,
        }
    }

    fn delay(ms: u64) -> ScenarioStep {
        ScenarioStep::Delay { ms }
    }

    fn wait(m: LineMatcher, timeout_ms: u64, save: Option<CaptureSpec>) -> ScenarioStep {
        ScenarioStep::Wait {
            matcher: m,
            timeout_ms,
            save,
        }
    }

    fn save(name: &str, group: usize) -> Option<CaptureSpec> {
        Some(CaptureSpec {
            variable: name.into(),
            group,
        })
    }

    fn repeat(times: u32, steps: Vec<ScenarioStep>) -> ScenarioStep {
        ScenarioStep::Repeat { times, steps }
    }

    fn assert_step(message: &str) -> ScenarioStep {
        ScenarioStep::Assert {
            matcher: matcher_lit(None, "x"),
            within_last: 1,
            message: message.into(),
        }
    }

    fn scenario(steps: Vec<ScenarioStep>) -> Scenario {
        Scenario {
            schema: SCENARIO_SCHEMA.into(),
            version: SCENARIO_VERSION,
            name: "t".into(),
            variables: BTreeMap::new(),
            steps,
        }
    }

    fn validate(steps: Vec<ScenarioStep>) -> Result<ValidatedScenario, ScenarioValidationError> {
        validate_scenario(scenario(steps))
    }

    fn code_of(r: Result<ValidatedScenario, ScenarioValidationError>) -> ScenarioErrorCode {
        r.expect_err("validation must fail").code
    }

    // ---------- fixtures（serde 形状互证） ----------

    #[test]
    fn minimal_fixture_deserializes_and_validates() {
        let s = parse_fixture("minimal.json");
        assert_eq!(s.schema, "bytetide.scenario");
        assert_eq!(s.version, 1);
        assert_eq!(s.name, "minimal");
        assert_eq!(
            s.variables.get("greeting").map(String::as_str),
            Some("hello")
        );
        assert_eq!(s.steps.len(), 2);
        let ScenarioStep::Send {
            mode,
            text,
            append_newline,
        } = &s.steps[0]
        else {
            panic!("step 0 must be send");
        };
        assert_eq!(*mode, SendModeDef::Ascii);
        assert_eq!(text, "${greeting}");
        assert!(*append_newline);
        let ScenarioStep::Wait {
            matcher,
            timeout_ms,
            save,
        } = &s.steps[1]
        else {
            panic!("step 1 must be wait");
        };
        assert_eq!(*timeout_ms, 5000);
        assert!(save.is_none());
        assert_eq!(matcher.dir, Some(Dir::Rx));
        assert_eq!(matcher.literal.as_deref(), Some("OK"));

        let v = validate_scenario(s).expect("minimal fixture must validate");
        assert_eq!(v.name, "minimal");
        assert_eq!(v.variables.len(), 1);
        assert_eq!(v.steps.len(), 2);
        let ValidatedStep::Send { template, .. } = &v.steps[0] else {
            panic!("validated step 0 must be send");
        };
        assert_eq!(template, "${greeting}");
        let ValidatedStep::Wait {
            matcher,
            timeout_ms,
            save,
        } = &v.steps[1]
        else {
            panic!("validated step 1 must be wait");
        };
        assert_eq!(*timeout_ms, 5000);
        assert!(save.is_none());
        assert_eq!(matcher.kind_name(), "literal");
        assert_eq!(matcher.dir, Some(Dir::Rx));
    }

    #[test]
    fn full_fixture_deserializes_and_validates() {
        let s = parse_fixture("full.json");
        assert_eq!(s.steps.len(), 7);
        assert!(matches!(
            &s.steps[0],
            ScenarioStep::Signal {
                pin: PinDef::Dtr,
                level: true
            }
        ));
        assert!(matches!(&s.steps[5], ScenarioStep::Delay { ms: 250 }));
        let ScenarioStep::Repeat { times, steps } = &s.steps[6] else {
            panic!("step 6 must be repeat");
        };
        assert_eq!(*times, 2);
        assert_eq!(steps.len(), 4);
        assert!(matches!(
            steps[0],
            ScenarioStep::Signal {
                pin: PinDef::Rts,
                level: false
            }
        ));
        let ScenarioStep::Send { mode, text, .. } = &steps[1] else {
            panic!("inner step 1 must be send");
        };
        assert_eq!(*mode, SendModeDef::Hex);
        assert_eq!(text, "aa ${token}");

        let v = validate_scenario(s).expect("full fixture must validate");
        assert_eq!(v.name, "full-coverage");
        assert_eq!(v.variables.len(), 2);
        assert_eq!(v.steps.len(), 7);
        // 预编译 matcher 种类与方向
        let ValidatedStep::Wait { matcher, save, .. } = &v.steps[2] else {
            panic!("step 2 must be wait");
        };
        assert_eq!(matcher.kind_name(), "regex");
        assert_eq!(matcher.dir, Some(Dir::Rx));
        assert_eq!(
            save.as_ref().map(|c| (&c.variable, c.group)),
            Some((&"value".to_string(), 1))
        );
        let ValidatedStep::Assert {
            matcher,
            within_last,
            template,
        } = &v.steps[3]
        else {
            panic!("step 3 must be assert");
        };
        assert_eq!(matcher.kind_name(), "hex");
        assert_eq!(*within_last, 10);
        assert_eq!(template, "expected PONG bytes, captured ${value}");
        let ValidatedStep::Wait { matcher, .. } = &v.steps[4] else {
            panic!("step 4 must be wait");
        };
        assert_eq!(matcher.kind_name(), "mask");
        assert_eq!(matcher.dir, Some(Dir::Tx));
        let ValidatedStep::Repeat { times, steps } = &v.steps[6] else {
            panic!("step 6 must be repeat");
        };
        assert_eq!(*times, 2);
        assert_eq!(steps.len(), 4);
        let ValidatedStep::Send { template, .. } = &steps[1] else {
            panic!("inner step 1 must be send");
        };
        assert_eq!(template, "aa ${token}");
    }

    #[test]
    fn full_fixture_round_trip_values_frozen() {
        let s = parse_fixture("full.json");
        // 键序不冻结：重新序列化再反序列化，值相等即互证成立
        let reserialized = serde_json::to_string(&s).expect("serialize");
        let reparsed: Scenario = serde_json::from_str(&reserialized).expect("reparse");
        assert_eq!(reparsed, s);
        // 原 fixture 也必须能无损往返（省略的可选字段走 default）
        assert_eq!(s, parse_fixture("full.json"));
    }

    #[test]
    fn invalid_future_fixture_deserializes_but_fails_validation() {
        // 形状合法（serde 层放行），未知版本在 validate 层报稳定错误
        let s = parse_fixture("invalid-future.json");
        assert_eq!(s.version, 2);
        let e = validate_scenario(s).expect_err("future version must be rejected");
        assert_eq!(e.code, ScenarioErrorCode::InvalidVersion);
        assert_eq!(e.path, "version");
        assert!(e.to_string().contains("invalid_version"));
    }

    // ---------- schema / version / name / steps ----------

    #[test]
    fn wrong_schema_rejected() {
        let mut s = scenario(vec![send("hi")]);
        s.schema = "bytetide.other".into();
        let e = validate_scenario(s).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::InvalidSchema);
        assert_eq!(e.path, "schema");
    }

    #[test]
    fn wrong_version_rejected() {
        let mut s = scenario(vec![send("hi")]);
        s.version = 0;
        let e = validate_scenario(s).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::InvalidVersion);
        assert_eq!(e.path, "version");
    }

    #[test]
    fn empty_name_rejected() {
        let mut s = scenario(vec![send("hi")]);
        s.name = String::new();
        let e = validate_scenario(s).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::EmptyName);
        assert_eq!(e.path, "name");
    }

    #[test]
    fn empty_steps_rejected() {
        let e = validate(vec![]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::EmptySteps);
        assert_eq!(e.path, "steps");
    }

    #[test]
    fn missing_fields_rejected_by_serde() {
        // 缺 steps：serde 缺字段错误（形状层拒绝，非 validate 层错误码）
        let r = serde_json::from_str::<Scenario>(
            r#"{"schema":"bytetide.scenario","version":1,"name":"x"}"#,
        );
        assert!(r.is_err());
        // save 省略走 default（None）
        let s: Scenario = serde_json::from_str(
            r#"{"schema":"bytetide.scenario","version":1,"name":"x",
                "steps":[{"kind":"wait","matcher":{"literal":"a"},"timeoutMs":1}]}"#,
        )
        .expect("wait without save must deserialize");
        assert_eq!(s.steps.len(), 1);
    }

    #[test]
    fn invalid_dir_rejected_by_serde() {
        // Dir 封闭枚举：未知 dir 在反序列化层即拒绝（invalid_dir 码仅为完整性保留）
        let r = serde_json::from_str::<Scenario>(
            r#"{"schema":"bytetide.scenario","version":1,"name":"x",
                "steps":[{"kind":"wait","matcher":{"dir":"sideways","literal":"a"},"timeoutMs":1}]}"#,
        );
        let msg = r.expect_err("unknown dir must be rejected").to_string();
        assert!(msg.contains("unknown variant"), "got {msg}");
    }

    // ---------- matcher 校验 ----------

    #[test]
    fn zero_or_multiple_patterns_conflict() {
        let e = validate(vec![wait(bare(None), 100, None)]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::MatcherConflict);
        assert_eq!(e.path, "steps[0]");

        let mut m = bare(None);
        m.literal = Some("a".into());
        m.regex = Some("a".into());
        let e = validate(vec![wait(m, 100, None)]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::MatcherConflict);
    }

    #[test]
    fn invalid_regex_reported_with_path() {
        let mut m = bare(None);
        m.regex = Some("(unclosed".into());
        let e = validate(vec![wait(m, 100, None)]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::InvalidRegex);
        assert_eq!(e.path, "steps[0]");
    }

    #[test]
    fn invalid_hex_reported() {
        for bad in ["", "abc", "zz", "1 2 3"] {
            let mut m = bare(None);
            m.hex = Some(bad.into());
            let e = validate(vec![wait(m, 100, None)]).unwrap_err();
            assert_eq!(e.code, ScenarioErrorCode::InvalidHex, "case {bad:?}");
        }
    }

    #[test]
    fn invalid_mask_reported() {
        for bad in ["", "5a ?x", "4?", "abc"] {
            let mut m = bare(None);
            m.mask = Some(bad.into());
            let e = validate(vec![wait(m, 100, None)]).unwrap_err();
            assert_eq!(e.code, ScenarioErrorCode::InvalidMask, "case {bad:?}");
        }
    }

    // ---------- 变量 ----------

    #[test]
    fn invalid_variable_name_in_initial_map() {
        let mut s = scenario(vec![send("hi")]);
        s.variables.insert("1bad".into(), "v".into());
        let e = validate_scenario(s).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::InvalidVariableName);
        assert_eq!(e.path, r#"variables["1bad"]"#);

        let mut s = scenario(vec![send("hi")]);
        s.variables.insert("a-b".into(), "v".into());
        assert_eq!(
            validate_scenario(s).unwrap_err().code,
            ScenarioErrorCode::InvalidVariableName
        );
    }

    #[test]
    fn invalid_capture_variable_name() {
        let e = validate(vec![wait(matcher_lit(None, "OK"), 100, save("9lives", 0))]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::InvalidVariableName);
        assert_eq!(e.path, "steps[0]");
    }

    #[test]
    fn undefined_variable_in_send_text() {
        let e = validate(vec![send("hello ${missing}")]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
        assert_eq!(e.path, "steps[0]");
        assert!(e.message.contains("${missing}"));
    }

    #[test]
    fn undefined_variable_in_assert_message() {
        let e = validate(vec![assert_step("boom ${nope}")]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
        assert_eq!(e.path, "steps[0]");
    }

    #[test]
    fn reference_ordering_strict_in_linear_block() {
        // 引用在 save 之前 → 未定义
        let e = validate(vec![
            send("${v}"),
            wait(matcher_lit(None, "OK"), 100, save("v", 0)),
        ])
        .unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
        assert_eq!(e.path, "steps[0]");

        // 引用在 save 之后 → 通过
        validate(vec![
            wait(matcher_lit(None, "OK"), 100, save("v", 0)),
            send("${v}"),
        ])
        .expect("reference after save must validate");
    }

    #[test]
    fn repeat_body_conservative_union() {
        let save_v = || wait(matcher_lit(None, "OK"), 100, save("v", 0));
        // 同块内引用先于 save → 保守放行（第 2 轮迭代可用）
        validate(vec![repeat(2, vec![send("${v}"), save_v()])])
            .expect("same-block reference before save is conservatively allowed");
        // 外层路径上的 save（Repeat 之前的直线步）体内可用
        validate(vec![save_v(), repeat(2, vec![send("${v}")])])
            .expect("outer-path save must be visible inside repeat");
        // 块后引用体内 save → 保守放行（times=0 时运行期兜底报错）
        validate(vec![repeat(0, vec![save_v()]), send("${v}")])
            .expect("post-block reference to in-body save is conservatively allowed");
        // 真未定义仍报错
        let e = validate(vec![repeat(2, vec![send("${ghost}")])]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
        assert_eq!(e.path, "steps[0].steps[0]");
        // 体内嵌套 Repeat 的 save 也并入外层（collect_saves 递归）
        validate(vec![repeat(
            1,
            vec![repeat(1, vec![save_v()]), send("${v}")],
        )])
        .expect("nested repeat saves must widen enclosing block");
    }

    #[test]
    fn matcher_patterns_are_not_scanned_for_refs() {
        // literal 匹配器中的 ${...} 是字面量（可匹配设备原样输出），不参与引用校验
        validate(vec![wait(matcher_lit(None, "${not_a_ref}"), 100, None)])
            .expect("matcher literal must not be treated as a variable reference");
    }

    // ---------- 限制 ----------

    #[test]
    fn nesting_depth_boundary() {
        let leaf = || vec![delay(1)];
        // 4 层 Repeat 允许
        validate(vec![repeat(
            1,
            vec![repeat(1, vec![repeat(1, vec![repeat(1, leaf())])])],
        )])
        .expect("4 repeat levels must be allowed");
        // 第 5 层报错，路径指到最内层 Repeat
        let five = vec![repeat(
            1,
            vec![repeat(
                1,
                vec![repeat(1, vec![repeat(1, vec![repeat(1, leaf())])])],
            )],
        )];
        let e = validate(five).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::NestingTooDeep);
        assert_eq!(e.path, "steps[0].steps[0].steps[0].steps[0].steps[0]");
    }

    #[test]
    fn delay_and_wait_bounds() {
        validate(vec![delay(MAX_DELAY_WAIT_MS)]).expect("delay at limit must pass");
        assert_eq!(
            code_of(validate(vec![delay(MAX_DELAY_WAIT_MS + 1)])),
            ScenarioErrorCode::DelayTooLong
        );
        validate(vec![wait(matcher_lit(None, "a"), MAX_DELAY_WAIT_MS, None)])
            .expect("wait at limit must pass");
        let e = validate(vec![wait(
            matcher_lit(None, "a"),
            MAX_DELAY_WAIT_MS + 1,
            None,
        )])
        .unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::WaitTooLong);
        assert_eq!(e.path, "steps[0]");
    }

    #[test]
    fn repeat_times_bounds() {
        validate(vec![repeat(MAX_REPEAT_TIMES, vec![delay(1)])])
            .expect("repeat at limit (x1 leaf) must pass");
        let e = validate(vec![repeat(MAX_REPEAT_TIMES + 1, vec![delay(1)])]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::RepeatTooMany);
        assert_eq!(e.path, "steps[0]");
    }

    #[test]
    fn step_limit_expansion() {
        // 上界恰为 10,000：通过
        validate(vec![repeat(10_000, vec![delay(1)])]).expect("10_000 executed steps must pass");
        // repeat 展开超限：10,000 × 2 叶
        let e = validate(vec![repeat(10_000, vec![delay(1), delay(1)])]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::StepLimitExceeded);
        assert_eq!(e.path, "steps");
        // 平铺 10,001 叶超限
        let flat: Vec<ScenarioStep> = (0..=10_000).map(|i| send(&format!("s{i}"))).collect();
        let e = validate(flat).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::StepLimitExceeded);
        assert!(e.message.contains("10001"));
    }

    // ---------- 捕获组 ----------

    #[test]
    fn capture_group_bounds() {
        let mut m = bare(None);
        m.regex = Some("a(b)(c)".into());
        // captures_len = 3（含 group 0）：0/1/2 合法，3 越界
        for g in [0usize, 1, 2] {
            validate(vec![wait(m.clone(), 100, save("v", g))])
                .unwrap_or_else(|e| panic!("group {g} must be in range: {e}"));
        }
        let e = validate(vec![wait(m, 100, save("v", 3))]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::CaptureGroupInvalid);
        assert_eq!(e.path, "steps[0]");
        // 非 regex matcher 没有捕获组：group 0 允许，>0 报错
        validate(vec![wait(matcher_lit(None, "OK"), 100, save("v", 0))])
            .expect("group 0 on literal matcher must pass");
        let e = validate(vec![wait(matcher_lit(None, "OK"), 100, save("v", 1))]).unwrap_err();
        assert_eq!(e.code, ScenarioErrorCode::CaptureGroupInvalid);
    }

    // ---------- 错误形状 ----------

    #[test]
    fn error_path_format_for_nested_steps() {
        // repeat(1)[ send, repeat(1)[ send, send("${ghost}") ] ] → steps[0].steps[1].steps[1]
        let s = vec![repeat(
            1,
            vec![send("a"), repeat(1, vec![send("b"), send("${ghost}")])],
        )];
        let e = validate(s).unwrap_err();
        assert_eq!(e.path, "steps[0].steps[1].steps[1]");
        assert_eq!(e.code.as_str(), "undefined_variable");
        let display = e.to_string();
        assert!(
            display.starts_with("steps[0].steps[1].steps[1]: "),
            "got {display}"
        );
        assert!(
            display.contains("[code=undefined_variable]"),
            "got {display}"
        );
    }

    #[test]
    fn error_codes_are_stable_strings() {
        for (code, s) in [
            (ScenarioErrorCode::InvalidSchema, "invalid_schema"),
            (ScenarioErrorCode::InvalidVersion, "invalid_version"),
            (ScenarioErrorCode::EmptyName, "empty_name"),
            (ScenarioErrorCode::EmptySteps, "empty_steps"),
            (ScenarioErrorCode::MatcherConflict, "matcher_conflict"),
            (ScenarioErrorCode::InvalidRegex, "invalid_regex"),
            (ScenarioErrorCode::InvalidHex, "invalid_hex"),
            (ScenarioErrorCode::InvalidMask, "invalid_mask"),
            (ScenarioErrorCode::InvalidDir, "invalid_dir"),
            (ScenarioErrorCode::NestingTooDeep, "nesting_too_deep"),
            (ScenarioErrorCode::StepLimitExceeded, "step_limit_exceeded"),
            (ScenarioErrorCode::DelayTooLong, "delay_too_long"),
            (ScenarioErrorCode::WaitTooLong, "wait_too_long"),
            (ScenarioErrorCode::RepeatTooMany, "repeat_too_many"),
            (
                ScenarioErrorCode::InvalidVariableName,
                "invalid_variable_name",
            ),
            (ScenarioErrorCode::UndefinedVariable, "undefined_variable"),
            (
                ScenarioErrorCode::CaptureGroupInvalid,
                "capture_group_invalid",
            ),
        ] {
            assert_eq!(code.as_str(), s);
        }
    }
}
