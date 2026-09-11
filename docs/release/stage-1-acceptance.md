# Stage 1 验收记录 — 0.5.1 可靠性补丁

> plan: `docs/superpowers/plans/2026-09-11-stage-1-reliability-and-release.md`
> 本文档在 stage gate 实际执行后按真实输出填写（无预填）。

## 环境

| 项 | 值 |
|---|---|
| 验收 commit | `84a9353415d20eb9a97469933f3d816ccd96b62a`（test: gate the reliability patch with route coverage） |
| OS | macOS 26.6.2（arm64） |
| Node | v26.7.0（满足 Node 20+ 要求） |
| rustc / cargo | 1.97.1 (Homebrew) |
| 验收日期 | 2026-09-11 |

## Stage 1 落地提交（自下而上）

| commit | 任务 |
|---|---|
| `2eb25c0` | Task 1 版本一致性预检 + Node 20 + workflow 门禁 |
| `d945c3d` | Task 6 Windows 后台 flag 修复 + tauri 配置断言 |
| `bcb837e` | Task 5 二进制字节计数 + 打开当前日志文件 |
| `6327c10` | Task 2 /exchange 基线先于发送 + 严格 matcher 400 |
| `989fe9c` | Task 3 core/REST 会话状态权威化（SessionState） |
| `f4bb9a0` | Task 4 bridge token OS 随机数 + 监听生命周期 + 远程绑定确认 |
| `a25d625` | fmt/clippy 门禁清零 + release verify job |
| `84a9353` | Task 7 路由级测试 + CI 步骤 + README 预检说明 |

## Stage gate 命令结果（全部退出 0）

| # | 命令 | 结果 |
|---|---|---|
| 1 | `npm ci` | ✅ 安装成功（npm warn install-scripts 为提示性告警，非失败） |
| 2 | `npm run check:version` | ✅ `check:version ok — all manifests at 0.5.0` |
| 3 | `npm run check:tauri-config` | ✅ `check:tauri-config ok — window[0] "ByteTide · 字节潮" additionalBrowserArgs valid` |
| 4 | `npm test` | ✅ 22 files / 308 tests passed |
| 5 | `cargo test --workspace` | ✅ bytetide-cli 31 · bytetide-core 41 · serial_tool 56 · bridge_routes(integration) 7，全部 0 failed |
| 6 | `cargo fmt --all -- --check` | ✅ 零 diff（bridge_routes.rs 一处归一后通过） |
| 7 | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ 零告警 |
| 8 | `cargo run -p bytetide-cli -- --help` | ✅ 正常输出用法 |
| 9 | `npm run build` | ✅ vue-tsc --noEmit 通过 + vite build 成功（926ms） |
| 10 | `git diff --check` | ✅ 无空白错误 |

本机 cargo 为 Homebrew 独立工具链，`cargo fmt`/`cargo clippy` 子命令被 ~/.cargo/bin 残缺 rustup shim 劫持，验收时以
`/opt/homebrew/Cellar/rust/1.97.1/bin/{rustfmt,cargo-clippy}` 直调等价执行；CI（windows-latest）无此问题，走标准子命令。

## 完成清单（plan Stage 1 completion checklist）

- [x] `/exchange` 基线在 send 前捕获（`bridge_last_no` 游标，fake-service 排序回归断言 last_no→send→follow）
- [x] 非法 regex/HEX/mask/dir 一律 400（稳定错误码 invalid_regex / invalid_hex / invalid_mask / invalid_direction / conflicting_matchers）
- [x] REST 状态与 lastError 与运行时事件一致（core `SessionState` 共享事实源，集成测试断言 bridge_list ↔ VecSink 事件一致）
- [x] 版本与发布 tag 强校验（`check:version`，`version.workspace = true` 修复 0.4.1 漂移）
- [x] CI 与文档要求 Node 20+
- [x] OS CSPRNG 供给 bridge token（getrandom 0.3，失败不回退）；绑失败经 `BridgeRuntime` 可达 UI
- [x] 二进制字节按原始 bytes 计数（`byteLength`，bytes 优先、UTF-8 回退）
- [x] 日志按钮真打开当前分段文件（`session_log_path_cmd` + plugin-opener，失败 toast）
- [x] Windows 后台 flag 通过配置断言（`check:tauri-config`；修复误入 `--disable-features` 列表的 bug）
- [x] Stage 1 gate 全部退出 0（见上表）

## 与 plan 的偏差记录

1. **`SessionSnap.status` 保持 `String`**（Task 3）：由 `SessionStatus::as_str()` 填充而非改枚举类型——REST 线上字符串完全不变，桥侧 `SessionDetail.status` 零改动。枚举本体 `SessionStatus` 已在 core 落地，后续 Stage 2 拆桥时可切类型。
2. **`/exchange` 缺会话 404**（Task 2）：原实现静默 baseline=0 轮询到超时，属漏网缺陷；plan 指定的 handler 顺序（`ok_or_else(not_found)`）本就要求 404。
3. **远程绑定确认**（Task 4）：plan 引用「prior security plan 的既有确认行为」，实测 2026-09-09 P0 计划的 `confirm_remote` 从未落地，按 P0 规格补齐（一次性确认、固定错误文本、前端两步警告）。
4. **`BridgeService` trait 多两个 notify 方法**（Task 2）：`BridgeCtx` 去 AppHandle 后 annotation/plot 变更回推前端经 trait 通知（生产 emit / 测试 no-op）。
5. **release.yml 结构**（Task 1→7 演进）：Task 1 先在打包 job 内加预检，Task 7 verify job 落地后移除 job 内重复步骤，由 `needs: [verify]` 统一承担。

## 遗留（不阻塞 gate）

- `Starting` 运行态为预留（同步 bind 下启动结果在命令返回前即确定，标 `#[allow(dead_code)]`）。
- cli-release.yml 独立 `cli-v*` 推送仅有版本预检（无 Node/Rust 全量门禁）——CLI tag 独立于 `v*` 版本体系，待 Stage 2 统一。
- 本验收在 macOS 单机执行；Windows 特有路径（后台 flag 实际生效、openPath 行为）依赖 CI 与真机使用验证。
