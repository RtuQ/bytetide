# ByteTide 三阶段实现代码审查

- 审查日期：2026-09-14
- 审查范围：`43cc31f..d56e355`
- 审查方式：静态代码审查、前后端测试、构建检查、回放 quick soak
- 总体结论：**暂不建议直接发布**。整体架构与测试质量较高，但仍有 4 个 P1 问题和 3 个 P2 问题需要处理。

## 1. 审查摘要

本轮实现已经完成三阶段规划中的主要工程化与功能建设，包括 typed IPC、持久化迁移、Rust core 拆分、离线日志分页、自动化场景、CLI 场景执行及时序回放。

以下部分完成质量较好：

- 前端不再直接散落调用 Tauri `invoke`，IPC 类型与命令入口已集中。
- `SessionRuntime::ingest` 成为实时传输与回放的共用数据入口。
- 离线日志采用稀疏索引和分页读取，前端不再全量读取后再回传 Rust。
- 自动化 matcher、报告、CLI、桌面命令之间建立了共享 fixture 和跨层测试。
- 回放调度具备 fake clock 测试，覆盖暂停、恢复、seek、调速、循环、EOF 和停止。
- 生命周期字段有显式策略及枚举式测试，能够防止新增会话字段遗漏迁移规则。

不过，跨模块组合后仍存在大文件 REST 查询重新全量物化、停止时尾批数据丢失、自动化静态校验不可靠等问题。这些问题单模块测试均可能通过，但会影响实际使用或破坏已确认的设计契约。

## 2. P1：发布前应修复

### 2.1 REST 分页查询会重新全量物化离线文件

涉及位置：

- `src-tauri/src/bridge/routes/lines.rs:61`
- `src-tauri/src/bridge/routes/annotations.rs:109`
- `crates/bytetide-core/src/serial/manager.rs:683`
- `crates/bytetide-core/src/offline/reader.rs:185`

`GET /lines` 在执行 `limit`、`offset` 和范围选择之前先调用 `snapshot()`。对于分页离线会话，`bridge_snapshot()` 最终会调用 `OfflineReader::snapshot()`，后者遍历整个文件并把所有数据行构造成一个 `Vec<BridgeLine>`。

这意味着即使请求：

```text
GET /sessions/{id}/lines?limit=10
```

面对 1GB 日志仍可能读取、解析并分配整个文件，造成：

- 瞬时内存从有界页读取退化为与文件行数线性相关；
- 大量 `String`、`BridgeLine` 分配可能远高于原文件大小；
- `OfflineReader` 锁在全量读取期间被持续占用；
- REST handler 所在异步执行线程长时间执行同步文件 IO；
- 极端情况下出现界面阻塞或 OOM。

批注接口为了按 `no` 回填单行内容，也会先取得全量 snapshot。

建议：

1. 给 `BridgeService` 增加分页和按行号读取接口，例如：

   ```rust
   fn lines_page(&self, id: &str, after: u64, limit: usize) -> Result<Vec<BridgeLine>, ServiceError>;
   fn line_by_no(&self, id: &str, no: u64) -> Result<Option<BridgeLine>, ServiceError>;
   ```

2. 无过滤的 `/lines` 查询直接把 `offset/limit` 下推到 `OfflineReader`。
3. 有过滤条件时采用固定页大小流式扫描，只保留命中页和必要计数，不构造全量快照。
4. 批注回填使用 `line_by_no`，不要调用 snapshot。
5. 增加离线百万行 REST 测试，断言 `limit=10` 时实际读取/返回量有界。

### 2.2 手动停止会话可能丢失最后一批日志

涉及位置：

- `crates/bytetide-core/src/serial/manager.rs:392`
- `src/stores/session/compat.ts:320`
- `src/composables/useTauriEvents.ts:195`

当前 `PortManager::disconnect` 的顺序是：

1. 从 `sessions` Map 删除句柄；
2. 设置停止标志或发送 Replay Stop；
3. join 后端线程；
4. 返回前端。

前端收到 `disconnected` 后确实会调用 `drainSession()` 补拉最后一批数据，但是 manager 已经提前删除会话，此时 `ring_lines_no_cmd` 只能返回“会话不存在”。

由于正常拉取周期为 200ms，用户点击“停止”时，最近一个周期内已经进入后端 ring、尚未进入前端 store 的数据可能永久丢失。渲染进程被系统节流时，缺口可能不止 200ms。

这与“停止仅断开连接，保留标签页与日志”的行为约定冲突。

建议采用以下方案之一：

- 两阶段关闭：停止并 join 线程，保留会话 ring；前端拉空后再显式释放句柄。
- `disconnect_cmd` 直接返回停止后的尾批行和最终游标，前端先 append 再完成状态切换。
- 引入短期 tombstone：断开后的 ring 在有限时间内保持只读，供最终补拉使用。

需要新增集成测试：先向 ring 写入一条尚未拉取的行，执行停止，再确认该行最终进入前端或 disconnect 返回值。

### 2.3 Repeat 变量可达性分析会放过必然失败的场景

涉及位置：

- `crates/bytetide-core/src/automation/model.rs:363`
- `crates/bytetide-core/src/automation/model.rs:499`

校验 Repeat 时，代码会先递归收集循环体内所有 `Wait.save`，然后把这些变量预先放进循环体的可用变量集合。这会让下面的场景通过预检：

```json
{
  "kind": "repeat",
  "times": 1,
  "steps": [
    {
      "kind": "send",
      "mode": "ascii",
      "text": "${token}",
      "appendNewline": false
    },
    {
      "kind": "wait",
      "matcher": { "literal": "TOKEN=" },
      "timeoutMs": 1000,
      "save": { "variable": "token", "group": 0 }
    }
  ]
}
```

第一次循环执行到 send 时，`token` 实际尚未产生，因此场景会在运行期失败。

另外，`repeat.times=0` 时循环体完全不会执行，但循环体声明的保存变量仍会被传播到 Repeat 之后；后续引用同样会通过静态校验、运行时失败。

这违反三阶段计划中“变量必须来自初始变量或每条可达执行路径上更早的保存步骤”的约束，也削弱了“运行前验证”的价值。

建议：

1. 第一轮循环必须严格按步骤顺序进行变量可达性分析。
2. 只有 `times >= 1` 时，循环体确定产生的变量才允许传播到外层。
3. 如果允许跨迭代引用，只能从第二轮开始使用第一轮已经确定保存的变量，不能放宽第一轮。
4. 分支未来扩展后，应采用所有可达路径变量集合的交集，而不是并集。
5. 增加“循环体先引用后保存”“0 次循环后引用”“嵌套循环变量传播”等失败用例。

### 2.4 Matcher 变量替换与已确认设计不一致

涉及位置：

- `docs/superpowers/specs/2026-09-11-bytetide-three-stage-evolution-design.md:128`
- `crates/bytetide-core/src/automation/model.rs:30`
- `crates/bytetide-core/src/automation/model.rs:445`
- `docs/automation-scenarios.md:105`

已确认的三阶段设计明确要求 `${name}` 可用于 send text 和 matcher pattern。但当前实现只替换：

- `Send.text`
- `Assert.message`

Wait/Assert 的 literal、regex、hex 和 mask matcher 都在校验期静态编译，运行期不会代入变量。用户文档随后被改成“matcher 模式不做变量替换”，但验收记录没有把它列为正式需求降级。

影响示例：场景先从设备响应中捕获地址或 token，再等待包含该变量的下一条响应，目前无法表达。

建议先确认产品决策：

- 如果原需求仍然有效：ValidatedStep 应保存 matcher 模板，执行步骤前进行变量替换并编译；静态校验仍可检查模板结构及变量引用。
- 如果正式取消：应同步修改最初设计文档，并在 Stage 3 acceptance 中明确记录该降级及原因。

## 3. P2：重要但不阻断核心数据正确性

### 3.1 场景进度事件会漏报步骤

涉及位置：

- `src-tauri/src/commands/automation.rs:561`
- `src-tauri/src/commands/automation.rs:625`

当前没有由 core runner 直接报告步骤边界，而是在 `RunnerHost` 中根据 Host 方法调用序列推测当前开始了哪一种步骤。

该启发式存在确定性漏报：

- 连续两个 Wait：第一个 Wait 结束时最后调用仍可能是 `LinesAfter`，第二个 Wait 的第一次 `lines_after` 会被误判为同一步轮询。
- 小于等于 25ms 的 Delay 不会 tick。
- 短 Delay 后接 Wait 时，Delay 会留下 `SleepShort`，随后的 Wait 也可能不 tick。

最终可能出现场景已经 passed，但 UI 的 `currentStep` 仍小于 `totalSteps`。

建议在 core runner 增加显式步骤回调：

```rust
fn on_step_started(&mut self, path: &str, kind: StepKind, current: u64, total: u64);
```

桌面端通过这个 hook 更新 registry 并发出稀疏事件，CLI 则用同一 hook 输出 stderr 进度。这样还能提供设计中已有的精确 `path`。

### 3.2 Replay 会话仍启用了普通发送、快捷帧和定时发送

涉及位置：

- `src/components/SendPanel.vue:17`
- `src/components/SendPanel.vue:49`
- `src/components/SendPanel.vue:106`
- `src/components/SendPanel.vue:319`

SendPanel 对所有活动会话渲染。Replay 运行时的会话状态是 `connected`，但以下入口只检查连接状态，没有检查 `kind === 'live'`：

- 普通发送按钮；
- Ctrl+Enter；
- 快捷帧点击；
- 定时发送循环。

后端确实会拒绝 Replay 发送，所以不会与真实设备交互，但 UI 会让用户执行必然失败的操作。更严重的是，如果用户在 live 会话开启定时发送后切到 replay，会持续调用后端、静默吞掉错误，最低间隔为 50ms。

建议：

1. 提供统一计算属性：

   ```ts
   const canSend = computed(
     () => active.value?.kind === 'live' && active.value.status === 'connected',
   )
   ```

2. 所有发送入口都复用 `canSend`。
3. 会话切换到不可发送状态时自动关闭定时发送。
4. Replay/Offline 会话可以隐藏发送区，或保留只读内容但统一禁用发送动作并显示原因。

### 3.3 回放线程创建失败会导致进程 panic

涉及位置：

- `crates/bytetide-core/src/replay/runner.rs:81`
- `crates/bytetide-core/src/replay/runner.rs:99`

实时会话创建 reader thread 时会把创建错误转换为 `anyhow::Result`，但回放使用：

```rust
.expect("spawn replay thread failed")
```

在线程资源耗尽、系统限制或创建失败时，这会让应用进程 panic，而不是向 UI 返回可处理错误。

建议将签名改为：

```rust
pub fn spawn_replay(...) -> std::io::Result<JoinHandle<()>>
```

再由 `PortManager::start_replay_indexed` 转换为 `anyhow::Result`，保持与实时连接相同的错误策略。

## 4. P3：非阻断问题

### 4.1 跨午夜回放丢失边界间隔

涉及位置：

- `crates/bytetide-core/src/offline/mod.rs:132`
- `crates/bytetide-core/src/replay/runner.rs:322`

离线时间戳只解析为当日毫秒，回放间隔使用：

```rust
line.epoch_millis.saturating_sub(prev)
```

当日志从 `23:59:59` 跨到 `00:00:00` 时，该段间隔会变成 0。回放打开结果中的 `durationMs` 也会低估跨午夜文件。

可以在索引/读取时维护日期回卷偏移：发现合法时间戳显著小于上一时间戳时累加 24 小时，使回放内部 epoch 保持单调；显示用的原始 `ts` 不变。

### 4.2 Replay 标签显示为 `文件名@0`

涉及位置：

- `src/components/TabBar.vue:31`

当前仅 Offline 会话跳过波特率后缀，Replay 会显示成 `xxx@0`。建议 live 显示端口/波特率，offline 和 replay 只显示文件名，或给 replay 增加“回放”徽标。

### 4.3 仍存在多个超大模块

当前较大的文件包括：

| 文件 | 行数 |
|---|---:|
| `crates/bytetide-core/src/serial/runtime.rs` | 1125 |
| `crates/bytetide-core/src/automation/model.rs` | 1117 |
| `src-tauri/src/bridge/routes/mod.rs` | 1104 |
| `src/parser/engine.ts` | 996 |
| `crates/bytetide-core/src/serial/manager.rs` | 971 |
| `crates/bytetide-core/src/replay/runner.rs` | 949 |

部分行数来自内联测试，因此不是单纯的生产逻辑膨胀。不过，为降低后续修改的认知负担，仍建议：

- Rust 大型测试迁入同模块的 `tests.rs`；
- `bridge/routes/mod.rs` 继续按 matcher、filter、frame/value 等职责拆分；
- `runtime.rs` 把状态转换、ingest pipeline 和 session loop 分开；
- 架构检查除总行数外，增加生产代码函数长度和圈复杂度约束。

## 5. 验证记录

### 5.1 已通过

| 检查 | 结果 |
|---|---|
| `node scripts/check-release-version.mjs` | 通过，版本均为 0.5.0 |
| `node scripts/check-tauri-config.mjs` | 通过 |
| `node scripts/check-architecture.mjs` | 通过，共检查 354 个源文件 |
| `npm run test` | 通过，33 个文件、469 项测试 |
| `npm run test:components` | 通过，3 个文件、22 项测试 |
| `npm run build` | 通过，`vue-tsc` 与 Vite build 均成功 |
| `cargo test --workspace` | 通过，共 371 项测试 |
| `node scripts/soak-replay.mjs` | 通过，234,096 行、2 次循环、0 errors |
| `git diff --check 43cc31f..HEAD` | 通过 |

Quick soak 摘要：

```json
{
  "mode": "quick",
  "alertHitEvents": 75,
  "alertHits": 75,
  "completedLoops": 2,
  "durationMs": 4419,
  "errors": [],
  "maxRingLines": 100000,
  "observedWrapClears": 2,
  "producedLines": 234096,
  "ringCap": 100000,
  "speed": 100
}
```

### 5.2 未执行或受限项

- `cargo clippy --workspace --all-targets -- -D warnings` 未执行成功：当前 stable toolchain 未安装 `cargo-clippy`。这不是代码测试失败。
- 未执行真实串口硬件验证。
- 未执行 1GB GUI 真机验证；当前 Stage 2 acceptance 使用结构性等价探针代替。
- 未执行真实 30 分钟回放 soak，仅执行 quick 模式。

### 5.3 工作区状态

审查期间未修改业务代码。开始审查前已有未跟踪文件：

```text
docs/functional-gap-research.md
```

该文件未被改动。

## 6. 建议修复顺序

1. REST 离线查询改为真正的分页/流式扫描，先解除 OOM 风险。
2. 修复停止时的最终 drain，保证日志完整性。
3. 收紧自动化变量可达性和 wait/repeat 下界，并补失败用例。
4. 确认 matcher 变量替换是否仍属于发布范围；实现或正式记录降级。
5. 把场景进度改为 runner 显式回调。
6. 统一 Replay/Offline 发送能力守卫。
7. 消除回放线程创建的生产路径 panic。
8. 处理跨午夜回放和标签显示等非阻断项。

## 7. 最终意见

当前实现已经具备较好的工程基础，测试覆盖和跨层契约意识明显提升。主要问题不是单模块功能缺失，而是分页、拉取、停止和自动化校验在跨模块组合后的行为没有完全闭环。

建议完成全部 P1 修复后重新执行全量测试，并至少补充以下专项回归：

- 百万行离线会话 REST `limit=10` 内存有界测试；
- 停止瞬间尾批日志不丢测试；
- Repeat 首轮先引用后保存必须预检失败；
- `wait.timeoutMs=0`、`repeat.times=0` 必须预检失败；
- matcher 变量替换契约测试；
- 连续 Wait 与短 Delay 的精确进度事件测试。

在这些问题解决之前，结论为：**Changes required / 不建议进入正式发布候选。**
