# ByteTide 三阶段修复复审报告

- 复审日期：2026-09-15
- 复审基线：`d56e355` 之后的当前未提交工作区（34 个已修改文件，约 1759 行新增、346 行删除）
- 复审目标：核对 2026-09-14 首轮报告中的 4 个 P1、3 个 P2、3 个 P3 修复情况，并检查修复引入的回归
- 总体结论：**Changes required / 暂不建议进入发布候选**

## 1. 结论摘要

首轮问题的大部分修复方向正确：REST 查询已不再先构造全量 snapshot，Repeat 变量顺序、wait/repeat 下界、显式步骤进度、Replay 发送守卫、回放线程 spawn 错误和标签 `@0` 均已处理。

但本次复审发现 3 个仍会影响数据正确性的 P1、2 个 P2，同时当前分支有两个确定失败的 CI 门禁：架构检查失败，Replay quick soak 连续两次失败。因此不能按“已修完”验收。

## 2. P1：发布前必须修复

### 2.1 `/lines` 的选择集分页语义发生回归

涉及位置：

- `src-tauri/src/bridge/routes/lines.rs:178`
- `src-tauri/src/bridge/routes/lines.rs:192`
- `src-tauri/src/bridge/routes/lines.rs:204`
- `src-tauri/src/bridge/routes/lines.rs:238`
- `src-tauri/src/bridge/routes/lines.rs:335`

新的实现解决了“大文件全量物化”的原 P1，但没有保持旧实现“先形成选择集，再对选择集执行 offset/limit”的契约。

最明确的例子是：会话共有 10000 行，请求 `last=100&offset=20&limit=10`。正确结果应是最后 100 行中的第 21～30 行，即 `9921..9930`；当前实现只读取最后 `offset + limit = 30` 行，再跳过 20 行，实际返回 `9991..10000`。

过滤路径存在同类错误：它只保留最后 `offset + limit` 个命中，然后在这个尾窗口中再次应用 offset，返回的是整个选择集末尾的页，而不是最后 N 个命中的指定页。

此外还有三个相关边界：

- `no=X` 分支完全忽略 offset；`no=42&offset=1` 仍返回第 42 行。
- 多个分支在 offset 超出选择集时返回公共 `empty`，把本应保留的 `total` 错误重置为 0。
- 精确命中的 `around` 分支没有把窗口下界钳到 `first_no`；live ring 已淘汰头部时会高估 total 并错位分页。

建议统一先计算选择集的逻辑起止位置，再把 `offset` 映射为实际行号；`last=N` 的首行应为 `last - total + 1`，页首应为该值加 offset。过滤路径应保留“选择集中的目标页”所需窗口，而不是简单保留全局最后 `offset + limit` 个命中。补齐 `last+offset`、`no+offset`、空页 total、ring 头部已淘汰的 around，以及过滤/无过滤对照测试。

### 2.2 最终补拉仍可能与在途拉取竞争并丢尾批

涉及位置：

- `src/composables/useTauriEvents.ts:187`
- `src/composables/useTauriEvents.ts:199`
- `src/stores/session/compat.ts:324`

后端 tombstone 两阶段关闭方向正确，但前端仍用 `draining: Set` 加固定 500ms 轮询等待在途拉取：

1. 普通拉取已占用 `draining`；
2. 用户执行停止，后端把 ring 放进 tombstone；
3. `drainSessionTail` 最多等待 500ms；
4. 如果普通拉取仍未结束，函数直接返回；
5. `stopSession` 立即调用 `releaseSession`，tombstone 被释放；
6. 原普通拉取后续无法完成最终拉空，尾部数据仍可能丢失。

这在渲染线程被节流、IPC 慢或一次需要拉多页时并非理论情况，恰好是原问题要覆盖的场景。建议把 Set 改成每会话可等待的 in-flight Promise/串行队列：停止流程必须等待已有 drain 结束，再执行一次 `ignoreStatus=true` 的最终 drain；只有这次最终 drain 明确完成后才能 release。不要用固定等待时间作为所有权交接。

### 2.3 跨午夜状态在顺序分页时丢失

涉及位置：

- `crates/bytetide-core/src/offline/reader.rs:95`
- `crates/bytetide-core/src/offline/reader.rs:99`
- `crates/bytetide-core/src/offline/reader.rs:156`

`OfflineReader` 的顺序快路径会复用当前文件偏移 `cur_off`，但 DayWrap 状态却通过 `anchor_state(want_start)` 重新取最近稀疏锚点的快照。文件偏移已经在上一页末尾，回卷状态却停留在锚点处；锚点到页末之间发生的午夜回卷不会被重新扫描。

例如第一页读取 1..5000，午夜发生在第 4500 行；第二页从 `cur_off` 直接读取第 5001 行，但恢复的是第 4097 行锚点处的 `day_off/prev_raw`，第 4500 行产生的 +24h 偏移丢失。结果是第二页 epoch 回退，回放间隔和离线时间轴都可能错误。

建议把 DayWrap 快照与 `cur_off/cur_next_no` 一起持久化，并在顺序续读时直接恢复游标快照；或者放弃该快路径，始终从锚点扫描到目标行。测试必须让午夜边界位于页内且下一次读取跨页，现有单页 midnight 测试覆盖不到此问题。

## 3. P2：重要问题

### 3.1 动态非正则 matcher 未校验 capture group

涉及位置：

- `crates/bytetide-core/src/automation/model.rs:595`
- `crates/bytetide-core/src/automation/runner.rs:398`
- `crates/bytetide-core/src/automation/runner.rs:424`

静态 literal/hex/mask 会在校验期拒绝 `save.group > 0`；动态 matcher 因为包含变量而推迟到运行期检查。但运行期 `capture_value` 只对 regex 校验组范围，非 regex 分支无条件返回整行文本，完全忽略 `spec.group`。

因此动态 literal（例如 `"ACK ${token}"`）配 `group: 9` 会通过校验并在运行时成功保存整行，而不是报 `capture_group_invalid`。当前新增测试只覆盖动态 regex 越界，没有覆盖动态 literal/hex/mask。

建议在 `matcher.regex() == None` 时显式要求 group 为 0，并为三种动态非正则 matcher 增加回归测试。

### 3.2 离线单行读取吞掉 IO 错误

涉及位置：

- `crates/bytetide-core/src/serial/manager.rs:572`
- `crates/bytetide-core/src/serial/manager.rs:576`

`bridge_line_by_no` 对离线读取调用 `.ok()`，把文件删除、权限变化或读取失败都转成 `Ok(None)`。REST 层随后会把真实存储故障表现为“没有这一行”，批注回填也无法区分行缺失和后端 IO 失败。

建议传播 `OfflineError`，只在成功读取但确实不存在该 no 时返回 `None`。同类的过滤扫描也使用 `.ok()?`，最好让 `stream_scan` 返回 `Result`，不要把服务错误统一折叠成 not-found。

## 4. CI / 工程门禁失败

### 4.1 架构检查失败

`npm run check:architecture` 返回 3 项违规：

| 文件 | 当前行数 | 上限 |
|---|---:|---:|
| `crates/bytetide-core/src/automation/model.rs` | 1337 | 1200 |
| `crates/bytetide-core/src/automation/runner/tests.rs` | 1211 | 1200 |
| `crates/bytetide-core/src/serial/manager.rs` | 1074 | 1000 |

该命令是 `.github/workflows/ci.yml` 的正式门禁，因此当前提交会直接 CI 失败。建议拆分职责和测试模块，不建议只抬高阈值来消除错误。

### 4.2 Replay quick soak 连续失败

执行两次：

```text
cargo test -p bytetide-core --test replay_soak -- --ignored --nocapture
```

均在 `crates/bytetide-core/tests/replay_soak.rs:235` 失败，错误为 `pause 未生效`，两次耗时约 21.5 秒和 38.4 秒。该测试同样是 CI 正式步骤。

当前断言使用整次测试的 `t0` 判断暂停是否在 5 秒内生效。Pause 是 bulk 阶段之后才发送；只要 bulk 阶段已经超过 5 秒，Pause 发出后的第一次轮询就可能立即失败，实际没有给暂停控制 5 秒处理时间。因此它首先是一个确定的门禁计时错误，不能据此断言产品 Pause 功能失效。

建议在 `pause_and_freeze` 内记录独立的 `pause_started/deadline`，修正测试后再判断 Pause 是否真的有竞态；同时调查 quick soak 从先前约 4 秒上升到本次 20～38 秒的性能变化，确认是否与新增逐行时间戳处理或运行环境有关。

## 5. 首轮问题复核

| 首轮项目 | 本次状态 | 说明 |
|---|---|---|
| P1 REST 全量 snapshot | 部分解决 | 内存有界方向成立，但选择集分页语义回归 |
| P1 停止丢尾批 | 部分解决 | tombstone 已加入，前端 drain/release 仍有竞争 |
| P1 Repeat 可达性 | 已解决 | 首轮严格顺序、下界及相关测试已补 |
| P1 matcher 变量替换 | 部分解决 | 动态模板已实现；非 regex capture group 有遗漏 |
| P2 进度启发式 | 已解决 | runner 提供显式 `on_step_started` |
| P2 Replay 发送入口 | 已解决 | 统一 `canSend`，切换时关闭定时发送 |
| P2 replay spawn panic | 已解决 | 改为返回 `io::Result` |
| P3 跨午夜 | 部分解决 | 单页已处理，顺序跨页状态丢失 |
| P3 Replay 标签 `@0` | 已解决 | replay/offline 均不再拼波特率 |
| P3 大模块 | 未通过门禁 | 三个文件超过现行阈值 |

## 6. 验证记录

### 已通过

| 检查 | 结果 |
|---|---|
| `cargo test --workspace` | 通过；core 216 项、desktop 58 项及各集成测试全部通过，ignored soak 除外 |
| `npm run test` | 通过；33 个文件、469 项测试 |
| `npm run test:components` | 通过；3 个文件、22 项测试 |
| `npm run build` | 通过；`vue-tsc --noEmit` 与 Vite build 均成功 |
| `cargo run -p bytetide-cli -- --help` | 通过 |
| `npm run test:architecture` | 通过；架构检查器自身 15 项测试通过 |
| `node scripts/check-release-version.mjs` | 通过；版本均为 0.5.0 |
| `node scripts/check-tauri-config.mjs` | 通过 |
| `git diff --check` | 通过 |

### 未通过

| 检查 | 结果 |
|---|---|
| `npm run check:architecture` | 失败；3 个文件超过行数门禁 |
| Replay quick soak | 连续两次失败；`pause 未生效` |

### 环境受限

- `cargo fmt --all --check` 无法执行：当前 stable toolchain 未安装 `rustfmt`。
- `cargo clippy --workspace --all-targets -- -D warnings` 无法执行：当前 stable toolchain 未安装 `clippy`。
- 未执行真实串口硬件、1GB GUI 真机和真实 30 分钟墙钟 soak。

## 7. 建议修复顺序

1. 修正 `/lines` 的 `last/no/around + offset` 和 total 契约，补过滤/无过滤对照测试。
2. 把最终 drain 改为可等待的单飞 Promise/队列，确保 release 发生在最终拉空之后。
3. 持久化 OfflineReader 顺序游标的 DayWrap 状态，补跨页午夜测试。
4. 修复 quick soak 的局部暂停 deadline，并复查吞吐下降。
5. 拆分超限模块，使架构门禁恢复通过。
6. 补动态非 regex capture group 检查，并传播离线读取 IO 错误。

完成上述 P1 与两项 CI 门禁后，再跑一次全量验证，才适合标记为发布候选。
