# Stage 2 验收记录 — 0.6.0 架构解耦

> plan: `docs/superpowers/plans/2026-09-11-stage-2-architecture-modularization.md`
> 本文档在 stage gate 实际执行后按真实输出填写（无预填）。

## 环境

| 项 | 值 |
|---|---|
| 验收 commit | `459ebca7ea273842c86d3833a50f6941c1d54ab1`（feat: open offline logs through paged backend pulls） |
| OS | macOS 26.6.2（arm64） |
| Node | v26.7.0 / Rust 1.97.1 (Homebrew) |
| 验收日期 | 2026-09-12 |

## Stage 2 落地提交（自下而上）

| commit | 任务 |
|---|---|
| `40c57c4` | T1 架构检查器 + Rust/TS 契约特征化冻结 |
| `0573fe8` | T2 core 提取 ring.rs + runtime.rs（manager 2708→2200） |
| `1693a5b` | T4 bridge.rs 4200→14 模块 + commands 拆分（命令名零变化） |
| `8c2de5d` | T5 src/ipc 类型化 IPC + 边界强制（允许清单清空） |
| `b903c0f` | T3 transport/{serial,tcp,udp} + recording + capture（manager→852） |
| `e1c7b67` | T6 session store 拆分 + 40 字段 FIELD_POLICY 穷举 |
| `911b342` | T8a 离线日志 Rust 流式索引分页（200 万行探针） |
| `85b28ec` | T7 持久化 v1 信封 + 16 键迁移 + testdata/protocol fixture |
| `bd67da2` | T7-Rust fixture include_str! 接入（双语言同向量） |
| `459ebca` | T8b 前端离线打开迁移分页拉取 + 1GB 探针脚本 |

## Stage gate 命令结果（全部退出 0）

| # | 命令 | 结果 |
|---|---|---|
| 1 | `npm ci` | ✅ |
| 2 | `npm test` | ✅ 31 文件 / 428 tests |
| 3 | `cargo test --workspace` | ✅ cli 31 · core 99+7(契约) · serial_tool 54+7(路由)+4(离线集成)，0 failed |
| 4 | `cargo fmt --all -- --check` | ✅ 零 diff |
| 5 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 零告警 |
| 6 | `cargo run -p bytetide-cli -- --help` | ✅ |
| 7 | `npm run build` | ✅ vue-tsc + vite |
| 8 | `npm run check:version` | ✅ 0.5.0 四清单一致 |
| 9 | `npm run check:tauri-config` | ✅ |
| 10 | `npm run check:architecture` | ✅ 313 源文件零违规 |
| 11 | `git diff --check` | ✅ |

## 里程碑 2（0.6.0）退出标准核对

- ✅ **无生产源文件超 1200 行**：检查器强制；最大文件 session/compat.ts 680、bridge/routes/mod.rs 983；manager.rs 852（覆盖阈值 1000）
- ✅ **前端 invoke/listen 全走 src/ipc**：架构检查器允许清单 0 文件，硬约束生效
- ✅ **每个会话字段有测试过的生命周期策略**：FIELD_POLICY 40 字段（carry 22 / resetClear 6 / resetReconnect 7 / runtime 5），lifecycle 测试穷举
- ✅ **持久化 schema 版本化 + 迁移**：StoredEnvelope v1 + 迁移注册表（未来版本拒绝覆盖），16 键迁移
- ✅ **Rust/TS 解析契约共享黄金 fixture**：testdata/protocol/{plot,matcher,tsv}-v1 双语言 include_str!/import 同向量
- ✅（结构性等价）**1GB 日志不全量过 IPC、RSS 增长 <500MB**：2,000,001 行(~214MB) Rust 集成探针断言锚点表 <16KiB + 页读有界；前端不再全文过 WebView（旧 read_text_file 链路弃用）；1GB 生成脚本 `npm run probe:offline` 供真机手测。**未在 GUI 真机跑 1GB 实测**——本项按结构性等价验收，真机实测列入遗留。

## 与 plan 的偏差记录

1. **初始阈值按现实基线**（T1）：plan 写 manager≤2200/bridge≤2900，实际 2708/4200——以现实设覆盖（2750/4300/1450），随任务逐级收紧至 plan 目标（manager 1000、bridge 模块 ≤1000、store ≤700）。
2. **SessionRuntime.ingest 返回扩两个字段**（T3）：`alerts`/`capture_hit` 由传输循环按原序执行——ingest 内直接 emit 会破坏告警攒批节奏与捕获档案行序（逐 case 核对，注释说明）。
3. **BridgeService trait 方法名换新**（T4）：按 plan 签名（list_sessions/session/…/send，Result<_,ServiceError>）；`bridge_follow` 移除由 lines_after+last_no 组合等价实现；旧 `serial_tool_lib::bridge::*` 路径经 mod.rs 再导出保持。
4. **T6 未建 Pinia 子 store**：plan 限定「仅响应式确需时建」——registry 形状与 store state 同构、单写入口已满足唯一真相，跨 store 组合无响应式收益。
5. **离线初始装载取尾窗**（T8b）：plan Step 4 文本写 `offlineLinesAfter(sessionId, 0, N)`，但从 0 起拉会停在文件头且 requestBackfill 只向后（无法到达尾部）；尾窗与旧链路截尾+followTail 视觉等价且 no==rn==文件行号使回补可达 no=1。
6. **IngestOrigin/Replay 隔离结构性成立**（T3）：回放路径无 CaptureController 可触达，测试断言零自动回复/零捕获/告警仍评估。
7. **0.6.0 版本号未 bump**：与 Stage 1 同策略，四处清单在发版打 tag 时原子化（check:version 会强制 tag 对照）。

## 遗留（不阻塞 gate）

- 1GB 真机 GUI 实测（probe:offline 脚本已备）。
- 离线会话无自动回裁：持续上滑会把整文件渐进载入视图缓冲（用户驱动、单次 2000 行，与 live 尾部不可裁同取舍）。
- `Starting` 运行态仍为预留；旧命令 read_text_file_cmd/create_offline_session_cmd 保留一个发布周期待清理（plan integration policy）。
- cli-release.yml 独立 tag 门禁仍未统一（Stage 1 遗留顺延）。
