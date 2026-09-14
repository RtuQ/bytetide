//! 场景运行报告与两种序列化格式（Stage 3 Task 2，plan：
//! `docs/superpowers/plans/2026-09-11-stage-3-automation-and-replay.md`）。
//!
//! # 确定性（硬要求）
//! 报告时间戳全部来自 [`crate::automation::runner::ScenarioHost::now_ms`]（可注入假钟）；
//! `variables` 用 `BTreeMap`（键序稳定）；serde 按字段声明序输出——同输入 + 同 host
//! 行为的两次运行 JSON 逐字节相同（假钟下含时间戳也相同）。
//!
//! # JSON（[`report_json`]）
//! `serde_json::to_string_pretty`（2 空格缩进）。场景/步级 `status` 为小写枚举串
//! （`passed|failed|cancelled` / `passed|failed|skipped`）；`matched_no`/`error`
//! 缺省为 `null`。`name` 字段是 plan 形状的一处微调（已回报）：JUnit testsuite 名
//! 取场景名，而 `report_junit(report)` 只收报告（签名 plan 指定），故名字随报告携带。
//!
//! # JUnit（[`report_junit`]）
//! 单 `<testsuite>`（name=场景名；tests/failures/errors/skipped 计数；time=秒·3 位小数）；
//! 每叶子步一个 `<testcase name="{path} [{kind}]" time="秒">`；失败步内嵌
//! `<failure type="{code}" message="{message}"/>`；skipped 步（取消中断与未执行统一
//! error=cancelled）内嵌 `<skipped message="cancelled"/>`。name/message 经
//! [`xml_escape`] 转义（`& < > " '`）。

use std::collections::BTreeMap;

use serde::Serialize;

/// 场景终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ScenarioStatus {
    Passed,
    Failed,
    Cancelled,
}

/// 叶子步结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Passed,
    Failed,
    /// 步被取消中断或因更早的失败/取消而未执行（error 统一为 `cancelled`）。
    Skipped,
}

/// 步级错误（code 为稳定错误码，见 runner 模块文档；JUnit `<failure type=…>`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepErrorInfo {
    pub code: String,
    pub message: String,
}

/// 一个已执行（或 skipped 标记）叶子步的报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepReport {
    /// 执行路径：静态路径（`steps[2].steps[0]`，与校验错误路径同格式）+ 每层
    /// Repeat 追加 0 基迭代后缀 `#{k}`（`#0`=首次、`#1`=第二次…），如
    /// `steps[6].steps[1].steps[0]#1#0`。
    pub path: String,
    /// 步种类：`send|delay|signal|wait|assert`。
    pub kind: &'static str,
    pub started_epoch_ms: u64,
    pub duration_ms: u64,
    /// wait/assert 命中行的 ring `no`；其余步为 `None`。
    pub matched_no: Option<u64>,
    pub error: Option<StepErrorInfo>,
    pub status: StepStatus,
}

/// 场景运行报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScenarioReport {
    pub name: String,
    pub status: ScenarioStatus,
    pub started_epoch_ms: u64,
    pub finished_epoch_ms: u64,
    pub duration_ms: u64,
    /// 最终变量表（初始 variables + 运行期捕获/覆盖）。
    pub variables: BTreeMap<String, String>,
    /// 每个已执行叶子步一条；中止后未执行步以 skipped 条目补齐。
    pub steps: Vec<StepReport>,
}

/// 报告 → 确定性 pretty JSON。
pub fn report_json(report: &ScenarioReport) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

/// 报告 → JUnit XML（单 testsuite；转义与形状见模块文档）。
pub fn report_junit(report: &ScenarioReport) -> String {
    let tests = report.steps.len();
    let failures = report
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Failed)
        .count();
    let skipped = report
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Skipped)
        .count();
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"{}\" time=\"{}\">\n",
        xml_escape(&report.name),
        tests,
        failures,
        skipped,
        secs(report.duration_ms),
    ));
    for step in &report.steps {
        let name = xml_escape(&format!("{} [{}]", step.path, step.kind));
        let time = secs(step.duration_ms);
        match step.status {
            StepStatus::Passed => {
                out.push_str(&format!("  <testcase name=\"{name}\" time=\"{time}\"/>\n"));
            }
            StepStatus::Failed => {
                let (code, message) = step
                    .error
                    .as_ref()
                    .map_or(("", ""), |e| (e.code.as_str(), e.message.as_str()));
                out.push_str(&format!(
                    "  <testcase name=\"{name}\" time=\"{time}\">\n    <failure type=\"{}\" message=\"{}\"/>\n  </testcase>\n",
                    xml_escape(code),
                    xml_escape(message),
                ));
            }
            StepStatus::Skipped => {
                out.push_str(&format!(
                    "  <testcase name=\"{name}\" time=\"{time}\">\n    <skipped message=\"cancelled\"/>\n  </testcase>\n"
                ));
            }
        }
    }
    out.push_str("</testsuite>\n");
    out
}

/// XML 属性/文本转义：`& < > " '`（`&` 先行，避免二次转义）。
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// 毫秒 → 秒字符串（3 位小数；JUnit time 属性）。
fn secs(ms: u64) -> String {
    format!("{:.3}", ms as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造的确定性报告：passed / failed（含转义字符）/ skipped 各一步。
    fn demo_report() -> ScenarioReport {
        let mut variables = BTreeMap::new();
        variables.insert("value".to_string(), "4f".to_string());
        ScenarioReport {
            name: "demo".to_string(),
            status: ScenarioStatus::Failed,
            started_epoch_ms: 1_000_000,
            finished_epoch_ms: 1_000_250,
            duration_ms: 250,
            variables,
            steps: vec![
                StepReport {
                    path: "steps[0]".to_string(),
                    kind: "send",
                    started_epoch_ms: 1_000_000,
                    duration_ms: 5,
                    matched_no: None,
                    error: None,
                    status: StepStatus::Passed,
                },
                StepReport {
                    path: "steps[1]".to_string(),
                    kind: "assert",
                    started_epoch_ms: 1_000_005,
                    duration_ms: 0,
                    matched_no: None,
                    error: Some(StepErrorInfo {
                        code: "assert_failed".to_string(),
                        message: "expected <PONG> & \"ack\" 'x'".to_string(),
                    }),
                    status: StepStatus::Failed,
                },
                StepReport {
                    path: "steps[2]".to_string(),
                    kind: "wait",
                    started_epoch_ms: 1_000_005,
                    duration_ms: 245,
                    matched_no: None,
                    error: Some(StepErrorInfo {
                        code: "cancelled".to_string(),
                        message: "cancelled".to_string(),
                    }),
                    status: StepStatus::Skipped,
                },
            ],
        }
    }

    #[test]
    fn json_golden_is_exact_and_deterministic() {
        let report = demo_report();
        let a = report_json(&report).expect("serialize");
        let b = report_json(&report).expect("serialize");
        assert_eq!(a, b, "同输入两次序列化必须逐字节相同");
        let expected = r#"{
  "name": "demo",
  "status": "failed",
  "started_epoch_ms": 1000000,
  "finished_epoch_ms": 1000250,
  "duration_ms": 250,
  "variables": {
    "value": "4f"
  },
  "steps": [
    {
      "path": "steps[0]",
      "kind": "send",
      "started_epoch_ms": 1000000,
      "duration_ms": 5,
      "matched_no": null,
      "error": null,
      "status": "passed"
    },
    {
      "path": "steps[1]",
      "kind": "assert",
      "started_epoch_ms": 1000005,
      "duration_ms": 0,
      "matched_no": null,
      "error": {
        "code": "assert_failed",
        "message": "expected <PONG> & \"ack\" 'x'"
      },
      "status": "failed"
    },
    {
      "path": "steps[2]",
      "kind": "wait",
      "started_epoch_ms": 1000005,
      "duration_ms": 245,
      "matched_no": null,
      "error": {
        "code": "cancelled",
        "message": "cancelled"
      },
      "status": "skipped"
    }
  ]
}"#;
        assert_eq!(a, expected);
    }

    #[test]
    fn junit_golden_with_escaping() {
        let xml = report_junit(&demo_report());
        let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuite name="demo" tests="3" failures="1" errors="0" skipped="1" time="0.250">
  <testcase name="steps[0] [send]" time="0.005"/>
  <testcase name="steps[1] [assert]" time="0.000">
    <failure type="assert_failed" message="expected &lt;PONG&gt; &amp; &quot;ack&quot; &apos;x&apos;"/>
  </testcase>
  <testcase name="steps[2] [wait]" time="0.245">
    <skipped message="cancelled"/>
  </testcase>
</testsuite>
"#;
        assert_eq!(xml, expected);
    }

    #[test]
    fn junit_counts_reflect_step_statuses() {
        let mut report = demo_report();
        report.status = ScenarioStatus::Cancelled;
        report.steps[1].status = StepStatus::Skipped;
        let xml = report_junit(&report);
        assert!(
            xml.contains(r#"<testsuite name="demo" tests="3" failures="0" errors="0" skipped="2""#)
        );
        assert!(!xml.contains("<failure"), "无 failed 步则无 failure 元素");
    }

    #[test]
    fn junit_escapes_scenario_name_and_path() {
        let report = ScenarioReport {
            name: "a<b>&\"c\"".to_string(),
            status: ScenarioStatus::Passed,
            started_epoch_ms: 0,
            finished_epoch_ms: 0,
            duration_ms: 0,
            variables: BTreeMap::new(),
            steps: vec![StepReport {
                path: "steps[0]#0".to_string(),
                kind: "wait",
                started_epoch_ms: 0,
                duration_ms: 1,
                matched_no: Some(7),
                error: None,
                status: StepStatus::Passed,
            }],
        };
        let xml = report_junit(&report);
        assert!(xml.contains(r#"<testsuite name="a&lt;b&gt;&amp;&quot;c&quot;""#));
        assert!(xml.contains(r#"<testcase name="steps[0]#0 [wait]" time="0.001"/>"#));
    }

    #[test]
    fn xml_escape_covers_all_five_chars() {
        assert_eq!(xml_escape("a<b>&\"'c"), "a&lt;b&gt;&amp;&quot;&apos;c");
        assert_eq!(xml_escape(""), "");
        assert_eq!(xml_escape("plain"), "plain");
    }

    #[test]
    fn secs_format_is_three_decimals() {
        assert_eq!(secs(0), "0.000");
        assert_eq!(secs(5), "0.005");
        assert_eq!(secs(250), "0.250");
        assert_eq!(secs(61_500), "61.500");
    }
}
