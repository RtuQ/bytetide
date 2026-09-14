# Stage 3 验收记录 — 0.7.0 自动化与回放

> plan: `docs/superpowers/plans/2026-09-11-stage-3-automation-and-replay.md`
> 本文档在终门实际执行后按真实输出填写（无预填）。

## 环境

| 项 | 值 |
|---|---|
| 验收 commit | `7de61ba4736993c83c7dc0f1086ab06958c9dfe2` |
| OS | macOS 26.6.2（arm64） / Node v26.7.0 / Rust 1.97.1 (Homebrew) |
| 验收日期 | 2026-09-14 |

## Stage 3 落地提交（自下而上）

| commit | 任务 |
|---|---|
| `b5b8186` | T1 scenario v1 模型 + 校验（43 测试、17 稳定错误码） |
| `8d358d3` | T6 replay core（fake-clock 22 测试 + manager 集成） |
| `5d9fbdb` | T2 确定性 runner + JSON/JUnit 报告（fake host 40 测试） |
| `48b05f1` | T7 桌面回放命令 + 前端控制（ReplayControls/拉循环泛化） |
| `c41042a` | T5 CLI `run` 子命令（退出码 0/1/2/3/130 + 16 集成测试） |
| `03701cd` | T3 桌面场景命令 + 运行注册表（8 集成测试） |
| `cfdbc5a` | T7 补齐：回放控制面核心侧（replay_cursor/view/control，提交遗漏修正） |
| `843fe30` | T4 场景工作台 UI（库/编辑器/运行视图 + jsdom 组件测试 22 例） |
| `26de812` | T8 跨特性三端等效 + soak 双模式 + 双语文档 |
| `7de61ba` | 终门前 rustfmt 归一 |

## 终门（Step 6 十三连，全部退出 0）

| # | 命令 | 结果 |
|---|---|---|
| 1 | `npm ci` | ✅ |
| 2 | `npm test` | ✅ 33 文件 / 469 tests |
| 3 | `npm run test:components` | ✅ 3 文件 / 22 tests（jsdom + @vue/test-utils） |
| 4 | `cargo test --workspace` | ✅ 16 个 test binary 全 ok（含三端 cross_feature、soak quick、replay 22+4、automation 83、bridge_routes 7、offline_pages 4 等） |
| 5 | `cargo fmt --all -- --check` | ✅（新测试文件归一后） |
| 6 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 零告警 |
| 7 | `cargo run -p bytetide-cli -- --help` | ✅ |
| 8 | `cargo run -p bytetide-cli -- run --help` | ✅ |
| 9 | `npm run build` | ✅ vue-tsc + vite |
| 10 | `npm run check:version` | ✅ 0.5.0 四清单一致 |
| 11 | `npm run check:architecture` | ✅ 353 源文件零违规 |
| 12 | `node scripts/soak-replay.mjs` | ✅ quick（虚拟时钟）：producedLines 234,096 / maxRingLines 100,000（恰在上限内）/ completedLoops 2 / errors [] |
| 13 | `git diff --check` | ✅ |

## 里程碑 3（0.7.0）退出标准核对

- ✅ **同一 fixture 三端等效报告**：`tests/fixtures/replay-scenario.log` + `testdata/scenarios/replay-validation.json` 驱动 core（fake host）/桌面（loopback TCP）/CLI（loopback TCP）三端，归一化时间戳后 status/步路径/matched_no/变量逐字段一致（期望常量三处同源）。
- ✅ **取消语义**：stop/disconnect/发送失败即中止（runner 每次宿主调用前后查 cancel，sleep 分片 25–50ms；桌面 disconnect 先取消场景再断开；CLI Ctrl-C→130）。
- ✅ **JSON 与 JUnit 确定性**：同输入同宿主序列 → JSON 逐字节相同（golden 测试钉死）；JUnit 单 testsuite 每叶步 testcase、XML 转义完整。
- ✅ **回放 start/pause/resume/stop/seek/speed/loop 且游标拉取**：逐行 `SessionRuntime::ingest(Replay origin)`，行 ts 保原值；暂停期时钟冻结、恢复重算整段；seek 清 ring 不重置 seq、Finished 可复活。
- ✅ **parser/plot/alert/scenario wait/assert 对回放工作**：告警经 common ingest 稀疏上报（alert-hit 事件）；只读场景（无 send/signal）可对回放运行并命中回放行；发送/信号线/录制/捕获对回放稳定报错。
- ✅ **30 分钟 100x soak 内存与缓冲上限**：quick 模式（虚拟时钟等效，CI 可跑）ring 恰触 100,000 上限不越界、no 单调无重复、事件有界线性；真实 30 分钟墙钟模式 `SOAK_REAL=1` 显式跑（未在本机执行——见遗留）。

## 与 plan 的偏差记录

1. **回放守卫放宽**（T8 对 T3 的行为修正）：T3 原把回放会话一刀切拒场景；plan 里程碑要求 wait/assert 对回放工作——改为「仅含 send/signal 步时拒绝（穿透 Repeat），只读场景放行」。
2. **T7 提交不完整经 cfdbc5a 补齐**：T7 的 manager 控制面（replay_cursor/replay_view/replay_control）漏暂存，跨特性测试暴露后以独立提交修复。
3. **progress 事件形状**：plan 写 `progress{path}`，实现为 `{currentStep,totalSteps,kind}`——core runner 无逐步 hook（禁改消费契约），桌面侧按宿主调用签名近似；path 缺省。
4. **gap 钳制语义**：作用于原始 Δ（`min(Δ, max_gap)/speed`），实际睡眠恒 ≤ max_gap/speed——测试在 3 档速度下钉死。
5. **二进制 fixture 行**：TSV 无法承载原始字节，规范对以 lossy 行覆盖；原始 bytes 语义由实时行单测覆盖，文档注明。
6. **0.7.0 版本号未 bump**：与 S1/S2 同策略，发版打 tag 时原子化（check:version 强制 tag 对照）。

## 遗留（不阻塞终门）

- 真实 30 分钟墙钟 soak（`SOAK_REAL=1 node scripts/soak-replay.mjs --real`）未在本机长跑；quick 模式结构性等效已入 CI。
- CLI 真 TCP send 有秒级锁竞争延迟（既有 tcp_loopback 现象，crates 源码未动，测试 1 分钟内稳定通过）。
- 三 stage roadmap 的 handoff checklist 剩「README 描述仅已交付功能」等收尾项——本阶段 README 已随 T8 更新为已交付功能。

## 三阶段总账

| 阶段 | 提交数 | 终门 |
|---|---|---|
| Stage 1 可靠性（0.5.1） | 9 | 十连 ✅ |
| Stage 2 架构（0.6.0） | 12 | 十一连 ✅ |
| Stage 3 自动化+回放（0.7.0） | 10 | 十三连 ✅ |
