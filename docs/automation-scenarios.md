# 场景自动化（Scenario Automation）用户指南

场景是一份版本化的 JSON 脚本（schema `bytetide.scenario` v1），按序执行发送、
延时、信号线、等待、断言与循环六类步骤，驱动设备并校验其响应。同一引擎在三端
运行：**桌面工作台**（侧栏「规则 → 场景」面板）、**CLI**（`bytetide run`）、
**core**（`bytetide_core::automation`，供嵌入式/测试复用）——三端对同一场景与
同一设备输入产出**逐字段等效**的执行报告（时间戳除外）。

---

## 1. 场景 JSON 结构

```json
{
  "schema": "bytetide.scenario",
  "version": 1,
  "name": "ping-pong",
  "variables": { "cmd": "PING" },
  "steps": [
    { "kind": "send", "mode": "ascii", "text": "${cmd}", "appendNewline": true },
    { "kind": "wait",
      "matcher": { "dir": "rx", "regex": "^PONG ([0-9A-Fa-f]{2})" },
      "timeoutMs": 2000,
      "save": { "variable": "value", "group": 1 } },
    { "kind": "assert", "matcher": { "hex": "50 4f 4e 47" },
      "withinLast": 10, "message": "expected PONG, got ${value}" }
  ]
}
```

顶层字段：

| 字段 | 类型 | 说明 |
|---|---|---|
| `schema` | string | 必须恰为 `bytetide.scenario` |
| `version` | number | 必须恰为 `1` |
| `name` | string | 非空；报告与 JUnit testsuite 名 |
| `variables` | object | 初始变量表（可省略）；变量名须匹配 `[A-Za-z_][A-Za-z0-9_]*` |
| `steps` | array | 非空；步骤树（Repeat 可嵌套 ≤4 层） |

## 2. 六类步骤

### send — 发送

| 字段 | 类型 | 说明 |
|---|---|---|
| `mode` | `"ascii"` \| `"hex"` | hex 模式文本须为偶数个十六进制字符（空白忽略），非法在 host 侧报错 |
| `text` | string | 发送内容，做 `${name}` 变量替换 |
| `appendNewline` | boolean | true 时追加 `\n`（换行由运行器追加，host 不处理） |

### delay — 延时

| 字段 | 类型 | 说明 |
|---|---|---|
| `ms` | number | 1..=600000（毫秒上限 10 分钟） |

### signal — 信号线

| 字段 | 类型 | 说明 |
|---|---|---|
| `pin` | `"dtr"` \| `"rts"` | 仅串口源支持；网络源/离线/回放会话报错 |
| `level` | boolean | true=置位 |

### wait — 等待（可捕获）

| 字段 | 类型 | 说明 |
|---|---|---|
| `matcher` | LineMatcher | 见下节 |
| `timeoutMs` | number | 1..=600000；超时报 `wait_timeout` 场景失败 |
| `save` | object? | `{ "variable": "名", "group": N }` 命中后捕获 |

Wait 只看**场景开始之后的新行**（游标语义：每个 Wait 只消费 `no` 大于当前
游标的行；一批数据被观察过即整体消费，无论是否命中）。捕获规则：regex 匹配
取 `group` 对应捕获组（group=0 为整个匹配；未参与匹配的组存空串）；
literal/hex/mask（校验层只允许 group=0）存命中行整行文本。

### assert — 断言

| 字段 | 类型 | 说明 |
|---|---|---|
| `matcher` | LineMatcher | 见下节 |
| `withinLast` | number | 在「最近 N 行」内回看（0 视为 1）；不消费 Wait 游标 |
| `message` | string | 失败时的说明（做变量替换）；未命中报 `assert_failed` |

### repeat — 循环

| 字段 | 类型 | 说明 |
|---|---|---|
| `times` | number | 1..=10000；0 视为整体跳过（无叶子无报告） |
| `steps` | array | 循环体（可继续嵌套 Repeat，≤4 层） |

报告路径加 0 基迭代后缀：`steps[6].steps[1].steps[0]#1#0`。

## 3. 行匹配器（LineMatcher）

`dir` 可选（省略 = RX/TX 都匹配）；`literal / regex / hex / mask` **恰一个**非空：

| 模式 | 语义 |
|---|---|
| `literal` | 子串包含（区分大小写） |
| `regex` | Rust regex 语法，须可编译 |
| `hex` | 十六进制字节序列子串查找（如 `"50 4f 4e 47"`；去空白后须偶数长纯 hex）；优先匹配行原始字节，无字节行回退文本 |
| `mask` | 每对 `??` 通配或两位 hex，如 `"5a ?? 4f"` |

matcher 模式**支持变量替换**（`${name}` 语法与 send text 同源）：含引用的模式在
执行该步时替换后编译（regex 可编译 / hex、mask 严格解析在替换后校验）；无引用的
模式预编译、运行零开销——仍可字面匹配含 `${` 的设备输出（`${` 后非合法变量名的
序列按字面量保留）。

## 4. 变量规则

- 引用语法 `${name}`；`send.text`、`assert.message` 与 matcher 模式均参与替换
  与引用检查。
- 变量来源：初始 `variables`，或更早 `wait.save` 的捕获。
- 静态校验严格按首轮迭代顺序：直线块与 Repeat 体内一致——引用必须来自初始
  `variables` 或**更早步骤**的 save（「循环体先引用后保存」第一轮迭代必然
  未定义，预检即报 `undefined_variable`）；跨迭代携带变量须在初始 `variables`
  预声明。Repeat 体内 save 在块后（`times ≥ 1`）即已定义，其后的引用合法。
- 运行期 `substitute` 兜底报 `undefined_variable`（防御绕过校验的构造）。
- 字面量边界：`$$` 原样保留（无转义）；`$` 后跟非 `{` 原样保留；`${` 无闭合 `}`
  或内容非法 → 整段按字面量保留（不算引用）。

## 5. 上限（执行前静态校验，越界即拒绝）

| 项 | 范围 |
|---|---|
| Repeat 嵌套深度 | 4 层 |
| 执行步数（Repeat 展开后） | ≤ 10,000 |
| delay.ms | ≤ 600,000 ms |
| wait.timeoutMs | 1 – 600,000 ms |
| repeat.times | 1 – 10,000 |

## 6. 错误码表

### 校验错误（执行前；桌面 `scenario-validate` 摘要与 CLI exit 2）

| code | 含义 |
|---|---|
| `invalid_schema` | schema 不是 `bytetide.scenario` |
| `invalid_version` | version 不是 1 |
| `empty_name` | name 为空 |
| `empty_steps` | steps 为空 |
| `matcher_conflict` | matcher 的 pattern 缺失或多于一个 |
| `invalid_regex` | regex 编译失败 |
| `invalid_hex` | hex 非空/偶数长/纯 hex 对校验失败 |
| `invalid_mask` | mask 对非法（须 `??` 或两位 hex） |
| `invalid_dir` | 保留码（dir 是封闭枚举，未知值在反序列化层即拒） |
| `nesting_too_deep` | Repeat 嵌套超过 4 层 |
| `step_limit_exceeded` | 执行步静态上界超 10,000 |
| `delay_too_long` | delay.ms 超 600,000 |
| `wait_too_long` | wait.timeoutMs 超 600,000 |
| `repeat_too_many` | repeat.times 超 10,000 |
| `wait_too_short` | wait.timeoutMs 为 0（下界 1） |
| `repeat_too_few` | repeat.times 为 0（下界 1） |
| `invalid_variable_name` | 变量名不合法 |
| `undefined_variable` | 引用了不可达（未声明/未捕获）的变量 |
| `capture_group_invalid` | save.group 超出该 matcher 允许范围（动态模板推迟到运行期捕获时报） |

校验错误携带索引路径（如 `steps[3].steps[1]`）。

### 运行期步级错误（报告中）

| code | 含义 |
|---|---|
| `wait_timeout` | 等待超时 |
| `assert_failed` | 断言未命中（message 为替换后的失败说明） |
| `undefined_variable` | 运行期替换遇到未定义变量 |
| `invalid_regex` / `invalid_hex` / `invalid_mask` / `matcher_conflict` | 含引用的 matcher 替换后编译失败（沿用校验码） |
| `capture_group_invalid` | 动态模板 save.group 运行期越界 |
| `step_limit_exceeded` | 运行期步数预算（10,000）耗尽 |
| `host_backpressure` | host 发送/接收队列满被拒 |
| `host_transport` | 传输/会话层故障（断连、hex 解码失败、会话不支持该操作等） |
| `host_cancelled` | sleep 中被取消 |
| `cancelled` | skipped 步的统一标记（取消中断或未执行） |

## 7. 会话守卫（哪类会话能跑什么）

| 会话 | 规则 |
|---|---|
| live（串口/网络已连接） | 全部步型可跑 |
| 离线会话 | 拒绝：「离线会话不支持场景」 |
| 回放会话 | 仅**只读场景**（不含 send/signal，Repeat 体同判）可跑——wait/assert
  对回放产生的行有效；含发送面步骤报「回放会话不支持发送步骤」 |

同一会话同时只允许一个运行中场景；停止（幂等）与断开会话都会把运行置为
`cancelled`。完成报告保留最近 50 份（只逐出已完成条目）。

## 8. 桌面工作台用法

1. 侧栏「规则」组展开 **场景** 面板 → 新建/导入场景 JSON（v1 schema）。
2. 「编辑」进入编辑器：六类步骤增删改、拖拽排序、Repeat 嵌套（可视化缩进，
   深度超限即报路径化错误）；校验错误定位到 `steps[i]…` 路径。
3. 选中已连接会话「运行」：运行视图显示进度（currentStep/totalSteps）与状态；
   「停止」幂等取消。
4. 完成后可导出 JSON / JUnit 报告。进度经稀疏事件（`scenario-progress` /
   `scenario-finished`）驱动，无逐行事件。

## 9. CLI 用法

```text
bytetide run --scenario FILE [--port PORT | --tcp HOST:PORT | ...]
             [--report FILE] [--report-format json|junit]
```

- 进度/错误 → stderr；报告只写 `--report` 文件，`--report -` 写 stdout
  （run 唯一的 stdout 输出；无 `--report` 时 stdout 完全干净）。
- 退出码：`0` 通过；`1` 运行时/连接失败（host 错误、变量未定义等）；
  `2` 参数/场景校验失败；`3` 断言或等待失败；`130` Ctrl-C 取消。

## 10. 报告格式

### JSON（`report_json`）

```json
{
  "name": "replay-validation",
  "status": "passed",
  "started_epoch_ms": 1730000000000,
  "finished_epoch_ms": 1730000001350,
  "duration_ms": 1350,
  "variables": { "value": "1234" },
  "steps": [
    { "path": "steps[0]", "kind": "wait", "started_epoch_ms": 1730000000000,
      "duration_ms": 12, "matched_no": 1, "error": null, "status": "passed" },
    { "path": "steps[2]", "kind": "wait", "started_epoch_ms": 1730000000050,
      "duration_ms": 30, "matched_no": 5, "error": null, "status": "passed" }
  ]
}
```

- `status`：`passed | failed | cancelled`；步级另有 `skipped`。
- `matched_no`：wait/assert 命中行的 ring `no`；其余步为 `null`。
- 同输入 + 同 host 行为 + 同钟 → 报告 JSON 逐字节确定。

### JUnit（`report_junit`）

单 `<testsuite>`（name=场景名，tests/failures/errors/skipped 计数）；每叶子步一个
`<testcase name="{path} [{kind}]" time="秒">`；失败步内嵌
`<failure type="{code}" message="{message}"/>`；skipped 步内嵌
`<skipped message="cancelled"/>`。XML 转义齐全，可直接接 CI 测试报告。

## 11. 三端等效与规范输入对

仓库根的规范输入对是跨特性验收基准，三端（core 集成测试 / 桌面命令层集成测试 /
CLI 子进程测试）跑同一对并断言归一化后的报告逐字段一致：

- `tests/fixtures/replay-scenario.log`——12 数据行：RX/TX 文本、二进制 lossy 行
  （TSV 落盘为 lossy 文本，原始字节不落盘，以 U+FFFD 行覆盖二进制语义）、
  变量捕获样本（`VALUE=1234`）、ASCII-Hex 绘图兼容帧、告警命中样本、
  一处 >10s 时间 gap（回放钳制样本）；
- `testdata/scenarios/replay-validation.json`——wait → send 探测 → 捕获变量 →
  send 引用变量 → wait → wait → assert 七步，全通过。

三端期望常量（步路径序 / matched_no / 捕获变量）内嵌于
`crates/bytetide-core/tests/cross_feature.rs`、`src-tauri/tests/cross_feature.rs`、
`crates/bytetide-cli/tests/cross_feature.rs`（注释标注「三端同源」，改 fixture
必须三处同步）。
