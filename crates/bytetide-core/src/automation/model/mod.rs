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
//! `wait_too_long` / `repeat_too_many` / `wait_too_short` / `repeat_too_few` /
//! `invalid_variable_name` / `undefined_variable` / `capture_group_invalid`。
//! `invalid_dir` 仅为错误码表完整性保留：`Dir` 是封闭枚举，未知 dir 在 serde
//! 反序列化层即被拒绝（`unknown variant`），走不到本层校验。
//! 错误携带索引路径（如 `steps[3].steps[1]`）与 message，`Display` 格式
//! `"{path}: {message} [code={code}]"`。
//!
//! # 变量替换与静态引用分析
//! 替换原语（`${name}` 语法、`$$`/非法名/无闭合 `}` 的字面量边界）见
//! [`crate::automation::matcher`]。设计契约：`Send.text`、`Assert.message`
//! **与 matcher 模式**（literal/regex/hex/mask 的 pattern 字段）均参与替换与
//! 引用检查。matcher 模式在 validate 期检查结构（恰一 pattern）与引用可达性；
//! 含引用的模式保留为 [`MatcherTemplate::Dynamic`]，语法校验（regex 可编译 /
//! hex/mask 严格解析）推迟到运行期替换后进行（[`MatcherTemplate::instantiate`]，
//! 失败以稳定错误码 `undefined_variable` / `invalid_regex` / `invalid_hex` /
//! `invalid_mask` / `matcher_conflict` 报步级错误）；无引用的模式 validate 期
//! 即预编译（[`MatcherTemplate::Static`]，运行零开销）。
//!
//! 保守引用可达性（静态分析，严格首轮顺序）：
//! - 直线块内严格按序：引用必须在初始 `variables` 或更早步骤 `Wait.save` 的变量集合内；
//! - Repeat 体同样严格按序分析（体的可用集 = 外层可用集 ∪ 体内更早的 save）——
//!   「循环体先引用后保存」在第一轮迭代必然未定义，validate 期即报
//!   `undefined_variable`；跨迭代携带变量须在初始 `variables` 预声明；
//! - Repeat 后其体 saves 并入外层可用集：`times ≥ 1`（校验下界恒真）时体内
//!   每条 save 都在执行路径上（未命中即场景 failed，其后的引用不可达），
//!   故块后引用体内 save 合法定义。
//!   真正未定义的引用运行期 [`substitute`] 仍兜底报错（防御绕过校验的手工
//!   构造 [`ValidatedScenario`]）。
//!
//! 注意：`substitute`/`VariableError` 定义在 [`crate::automation::matcher`]（文本层
//! 原语与 matcher 同文件），本模块做引用可达性校验与模板装配。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::automation::matcher::{
    compile_matcher, find_refs, is_valid_var_name, substitute, CompiledMatcher, LineMatcher,
    MatcherError, MatcherErrorCode, VariableError,
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

/// Wait/Assert 的 matcher 模板（校验产物）。设计契约：`${name}` 可用于 matcher
/// 模式——无引用的模式 validate 期预编译（[`MatcherTemplate::Static`]，运行
/// 零开销）；含引用的保留模板，运行期替换后编译（[`MatcherTemplate::Dynamic`]）。
#[derive(Debug, Clone)]
pub enum MatcherTemplate {
    /// 模式无变量引用：已按原样预编译。
    Static(CompiledMatcher),
    /// 模式含 `${name}` 引用：保留原模板（每个非空 pattern 字段运行期替换）。
    Dynamic(LineMatcher),
}

/// 动态 matcher 实例化错误（运行期）：变量未定义或替换后模式编译失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatcherInstantiateError {
    Variable(VariableError),
    Compile(MatcherError),
}

impl fmt::Display for MatcherInstantiateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Variable(e) => write!(f, "{e}"),
            Self::Compile(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MatcherInstantiateError {}

impl MatcherTemplate {
    /// 运行期实例化：Static 克隆（Regex 为 Arc 引用计数，近零开销）；Dynamic
    /// 对每个非空 pattern 字段做 `${name}` 替换后编译。变量未定义或替换后
    /// 模式语法非法（regex 不可编译 / hex、mask 严格解析不过）在此报出。
    pub fn instantiate(
        &self,
        vars: &BTreeMap<String, String>,
    ) -> Result<CompiledMatcher, MatcherInstantiateError> {
        match self {
            Self::Static(m) => Ok(m.clone()),
            Self::Dynamic(t) => {
                let field =
                    |p: &Option<String>| -> Result<Option<String>, MatcherInstantiateError> {
                        match p {
                            Some(s) => substitute(s, vars)
                                .map(Some)
                                .map_err(MatcherInstantiateError::Variable),
                            None => Ok(None),
                        }
                    };
                let resolved = LineMatcher {
                    dir: t.dir,
                    literal: field(&t.literal)?,
                    regex: field(&t.regex)?,
                    hex: field(&t.hex)?,
                    mask: field(&t.mask)?,
                };
                compile_matcher(&resolved).map_err(MatcherInstantiateError::Compile)
            }
        }
    }
}

/// 校验后的步骤：文本保留模板原样（运行期做变量替换），matcher 装配为模板。
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
        matcher: MatcherTemplate,
        timeout_ms: u64,
        save: Option<CaptureSpec>,
    },
    Assert {
        matcher: MatcherTemplate,
        within_last: usize,
        template: String,
    },
    Repeat {
        times: u32,
        steps: Vec<ValidatedStep>,
    },
}

/// 校验通过的场景：模型 + matcher 模板树（Static 预编译 / Dynamic 运行期实例化），
/// runner 直接执行，免重编译。
#[derive(Debug, Clone)]
pub struct ValidatedScenario {
    pub name: String,
    pub variables: BTreeMap<String, String>,
    pub steps: Vec<ValidatedStep>,
}

/// 静态执行叶步上界（Repeat 按次数展开；与 runner 运行期计数同口径，
/// 校验摘要与运行进度回调的 totalSteps 共用）。
pub fn count_executed_leaves(steps: &[ValidatedStep]) -> u64 {
    steps
        .iter()
        .map(|s| match s {
            ValidatedStep::Repeat { times, steps } => {
                u64::from(*times).saturating_mul(count_executed_leaves(steps))
            }
            _ => 1,
        })
        .sum()
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
    /// 下界：`wait.timeoutMs` 必须 ≥ 1（设计契约 wait 1–600,000 ms）。
    WaitTooShort,
    /// 下界：`repeat.times` 必须 ≥ 1（设计契约 repeat 1–10,000）。
    RepeatTooFew,
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
            Self::WaitTooShort => "wait_too_short",
            Self::RepeatTooFew => "repeat_too_few",
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
            format!(
                "capture group {group} out of range: matcher exposes {groups} group(s) (group 0 = whole match)"
            ),
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

struct BlockOut {
    steps: Vec<ValidatedStep>,
    /// 该块的执行步静态上界（叶子步 × 路径上 repeat 展开倍数）。
    expansion: u64,
    /// 本块（含嵌套 Repeat 体）内全部 `Wait.save` 变量名——调用方（Repeat 步骤
    /// 或顶层）据此把「块后已定义」变量并入可用集。
    saves: BTreeSet<String>,
}

/// matcher pattern 非空字段数（恰一校验在 refs 检查前先行，保持原错误优先级：
/// 冲突 matcher 先报 `matcher_conflict` 再谈引用）。
fn pattern_count(m: &LineMatcher) -> usize {
    usize::from(m.literal.is_some())
        + usize::from(m.regex.is_some())
        + usize::from(m.hex.is_some())
        + usize::from(m.mask.is_some())
}

/// 模式字段是否含 `${name}` 引用（含引用 ⟹ 走 Dynamic 模板，语法校验推迟）。
fn matcher_has_refs(m: &LineMatcher) -> bool {
    [&m.literal, &m.regex, &m.hex, &m.mask]
        .into_iter()
        .any(|p| p.as_deref().is_some_and(|s| !find_refs(s).is_empty()))
}

/// 校验并装配 matcher 模板：恰一 pattern（原错误码/文案）→ 引用可达性 →
/// 无引用时立即编译（regex/hex/mask 语法错误在 validate 期报出），含引用时
/// 保留 [`MatcherTemplate::Dynamic`]（语法校验在运行期替换后进行）。
fn validate_matcher(
    matcher: &LineMatcher,
    path: &str,
    avail: &BTreeSet<String>,
) -> Result<MatcherTemplate, ScenarioValidationError> {
    if pattern_count(matcher) != 1 {
        return Err(matcher_error(
            path,
            compile_matcher(matcher).expect_err("pattern_count != 1 must fail compile"),
        ));
    }
    for p in [
        &matcher.literal,
        &matcher.regex,
        &matcher.hex,
        &matcher.mask,
    ]
    .into_iter()
    .flatten()
    {
        check_refs(p, path, avail)?;
    }
    if matcher_has_refs(matcher) {
        return Ok(MatcherTemplate::Dynamic(matcher.clone()));
    }
    compile_matcher(matcher)
        .map(MatcherTemplate::Static)
        .map_err(|e| matcher_error(path, e))
}

/// 校验并编译一个线性步骤块。`block_path` 是块路径（顶层 `"steps"`，Repeat 体为
/// `"{repeat_path}.steps"`），步骤路径 = `{block_path}[{i}]`。
/// `avail` 严格按序累积：块内 Wait.save（及 `times ≥ 1` 的 Repeat 体 saves）
/// 并入，引用必须在初始变量或更早的保存步骤集合内。
fn walk_block(
    steps: &[ScenarioStep],
    block_path: &str,
    depth: u32,
    avail: &mut BTreeSet<String>,
) -> Result<BlockOut, ScenarioValidationError> {
    let mut out = Vec::with_capacity(steps.len());
    let mut saves: BTreeSet<String> = BTreeSet::new();
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
                if *timeout_ms == 0 {
                    return Err(err(
                        ScenarioErrorCode::WaitTooShort,
                        &path,
                        "wait timeoutMs must be >= 1 (design limit: 1..=600000 ms)",
                    ));
                }
                if *timeout_ms > MAX_DELAY_WAIT_MS {
                    return Err(err(
                        ScenarioErrorCode::WaitTooLong,
                        &path,
                        format!(
                            "wait timeoutMs {timeout_ms} exceeds maximum {MAX_DELAY_WAIT_MS} ms"
                        ),
                    ));
                }
                let template = validate_matcher(matcher, &path, avail)?;
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
                    // 静态 matcher：组范围 validate 期可查；动态（含引用）推迟到
                    // 运行期捕获时（capture_group_invalid 步级错误）。
                    if let MatcherTemplate::Static(compiled) = &template {
                        check_capture_group(compiled, spec.group, &path)?;
                    }
                }
                expansion += 1;
                out.push(ValidatedStep::Wait {
                    matcher: template,
                    timeout_ms: *timeout_ms,
                    save: save.clone(),
                });
                if let Some(spec) = save {
                    avail.insert(spec.variable.clone());
                    saves.insert(spec.variable.clone());
                }
            }
            ScenarioStep::Assert {
                matcher,
                within_last,
                message,
            } => {
                let template = validate_matcher(matcher, &path, avail)?;
                check_refs(message, &path, avail)?;
                expansion += 1;
                out.push(ValidatedStep::Assert {
                    matcher: template,
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
                        format!(
                            "repeat nesting depth {inner_depth} exceeds maximum {MAX_NESTING_DEPTH}"
                        ),
                    ));
                }
                if *times == 0 {
                    return Err(err(
                        ScenarioErrorCode::RepeatTooFew,
                        &path,
                        "repeat times must be >= 1 (design limit: 1..=10000)",
                    ));
                }
                if *times > MAX_REPEAT_TIMES {
                    return Err(err(
                        ScenarioErrorCode::RepeatTooMany,
                        &path,
                        format!("repeat times {times} exceeds maximum {MAX_REPEAT_TIMES}"),
                    ));
                }
                // 严格首轮顺序分析：体的可用集从外层可用集起笔，体内 save 按步序
                // 并入——「先引用后保存」在第一轮迭代必然未定义，validate 即报错；
                // 跨迭代携带须在初始 variables 预声明。times ≥ 1（校验下界恒真）
                // 时体内每条 save 都在执行路径上（未命中即 failed，其后引用不可达），
                // 块后并入外层可用集。
                let mut body_avail = avail.clone();
                let body = walk_block(
                    steps,
                    &format!("{path}.steps"),
                    inner_depth,
                    &mut body_avail,
                )?;
                avail.extend(body.saves.iter().cloned());
                saves.extend(body.saves);
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
        saves,
    })
}

#[cfg(test)]
mod tests;
