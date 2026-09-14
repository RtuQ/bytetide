//! `automation::model` 单测（模型/校验契约与模块文档见 mod.rs）。

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
    let ValidatedStep::Wait { matcher, .. } = &v.steps[1] else {
        panic!("validated step 1 must be wait");
    };
    let MatcherTemplate::Static(matcher) = matcher else {
        panic!("static matcher must precompile");
    };
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
    // 预编译 matcher 种类与方向（无引用模式 → Static 预编译）
    let ValidatedStep::Wait { matcher, save, .. } = &v.steps[2] else {
        panic!("step 2 must be wait");
    };
    let MatcherTemplate::Static(matcher) = matcher else {
        panic!("static matcher must precompile");
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
    let MatcherTemplate::Static(matcher) = matcher else {
        panic!("static matcher must precompile");
    };
    assert_eq!(matcher.kind_name(), "hex");
    assert_eq!(*within_last, 10);
    assert_eq!(template, "expected PONG bytes, captured ${value}");
    let ValidatedStep::Wait { matcher, .. } = &v.steps[4] else {
        panic!("step 4 must be wait");
    };
    let MatcherTemplate::Static(matcher) = matcher else {
        panic!("static matcher must precompile");
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
fn repeat_body_strict_first_iteration_ordering() {
    let save_v = || wait(matcher_lit(None, "OK"), 100, save("v", 0));
    // 评审用例（P1-3）：体内先引用后保存 → 第一轮迭代必然未定义，预检失败
    let e = validate(vec![repeat(1, vec![send("${v}"), save_v()])]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
    assert_eq!(e.path, "steps[0].steps[0]");
    // 跨迭代携带（引用在 save 之前、times>1）同样失败：须在初始 variables 预声明
    let e = validate(vec![repeat(3, vec![send("${v}"), save_v()])]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
    // 体内严格按序：save 之后的引用合法
    validate(vec![repeat(2, vec![save_v(), send("${v}")])])
        .expect("reference after in-body save must validate");
    // 外层路径上的 save（Repeat 之前的直线步）体内可用
    validate(vec![save_v(), repeat(2, vec![send("${v}")])])
        .expect("outer-path save must be visible inside repeat");
    // 块后引用体内 save → 合法（times ≥ 1 时 save 在执行路径上）
    validate(vec![repeat(2, vec![save_v()]), send("${v}")])
        .expect("post-block reference to in-body save must validate");
    // 真未定义仍报错
    let e = validate(vec![repeat(2, vec![send("${ghost}")])]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
    assert_eq!(e.path, "steps[0].steps[0]");
    // 嵌套 Repeat：内层体 save 在内层块后（外层体后续步）可用
    validate(vec![repeat(
        1,
        vec![repeat(2, vec![save_v()]), send("${v}")],
    )])
    .expect("nested repeat saves must propagate to enclosing body");
    // 初始变量预声明后，体内先引用后保存合法（跨迭代携带的正道）
    let mut s = scenario(vec![repeat(2, vec![send("${v}"), save_v()])]);
    s.variables.insert("v".into(), String::new());
    validate_scenario(s).expect("pre-declared variable allows cross-iteration carry");
}

#[test]
fn wait_timeout_and_repeat_times_lower_bounds() {
    // 设计契约下界：wait 1..=600000 ms、repeat 1..=10000（评审回归清单）
    let e = validate(vec![wait(matcher_lit(None, "a"), 0, None)]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::WaitTooShort);
    assert_eq!(e.path, "steps[0]");
    assert_eq!(e.code.as_str(), "wait_too_short");
    let e = validate(vec![repeat(0, vec![delay(1)])]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::RepeatTooFew);
    assert_eq!(e.path, "steps[0]");
    assert_eq!(e.code.as_str(), "repeat_too_few");
}

#[test]
fn matcher_templates_validate_refs_and_stay_dynamic() {
    // matcher 模式参与变量替换（设计契约）：引用已定义 → Dynamic 模板
    let save_v = wait(matcher_lit(None, "OK"), 100, save("tok", 0));
    let mut m = bare(None);
    m.literal = Some("TOKEN=${tok}".into());
    let v = validate(vec![save_v, wait(m, 100, None)]).expect("templated literal must pass");
    let ValidatedStep::Wait { matcher, .. } = &v.steps[1] else {
        panic!("step 1 must be wait");
    };
    assert!(
        matches!(matcher, MatcherTemplate::Dynamic(_)),
        "含引用的 matcher 保留模板"
    );
    // 未定义引用 → 预检失败
    let mut m = bare(None);
    m.regex = Some("PONG ${ghost}".into());
    let e = validate(vec![wait(m, 100, None)]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
    assert_eq!(e.path, "steps[0]");
    // 非法名序列不是引用（${1x} 字面量保留）→ 仍按无引用静态编译
    let mut m = bare(None);
    m.literal = Some("x${1y}z".into());
    let v = validate(vec![wait(m, 100, None)]).expect("invalid-name braces are literal");
    let ValidatedStep::Wait { matcher, .. } = &v.steps[0] else {
        panic!("step 0 must be wait");
    };
    assert!(matches!(matcher, MatcherTemplate::Static(_)));
    // hex 模式带引用：结构/引用合法即可过，语法（替换后）运行期校验
    let save_hex = wait(matcher_lit(None, "OK"), 100, save("b", 0));
    let mut m = bare(None);
    m.hex = Some("50 ${b}".into());
    validate(vec![save_hex, wait(m, 100, None)])
        .expect("templated hex must pass structural validation");
}

#[test]
fn matcher_literal_refs_are_validated() {
    // 原「matcher 不扫引用」行为已按设计契约取消：合法名引用必须可达
    let e = validate(vec![wait(matcher_lit(None, "${not_a_ref}"), 100, None)]).unwrap_err();
    assert_eq!(e.code, ScenarioErrorCode::UndefinedVariable);
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
        (ScenarioErrorCode::WaitTooShort, "wait_too_short"),
        (ScenarioErrorCode::RepeatTooFew, "repeat_too_few"),
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
