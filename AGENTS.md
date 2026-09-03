# ByteTide(字节潮)— 项目记忆

串口调试工具: Tauri 2 + Vue 3 (`<script setup>`) + TypeScript + Pinia + Vite。原名 Serial Tool。
本文档记录 UI/UX 风格规范与代码约定，**改 UI 前先读这里**，保持风格一致。

---

## 1. 设计系统（来源：ui-ux-pro-max skill）

风格定位：**数据密集型仪表盘 + 深色 OLED**。信息密集但可读，靠分层制造纵深（避免扁平无层次的反模式）。

### 配色 token（`src/styles.css` `:root`，单一事实来源）

| token | 值 | 用途 |
|---|---|---|
| `--bg-base` | `#0b1220` | 应用最底层背景 |
| `--bg-surface` | `#0f172a` | 主面板/侧栏 |
| `--bg-elevated` | `#1e293b` | 卡片/输入/按钮底色 |
| `--bg-hover` / `--bg-active` | `#334155` / `#475569` | hover / pressed |
| `--border` / `--border-strong` | `#1e293b` / `#334155` | 弱/强描边 |
| `--border-focus` | `#3b82f6` | 聚焦环 |
| `--text` / `--text-muted` / `--text-dim` | `#f1f5f9` / `#94a3b8` / `#64748b` | 主/次/弱文字 |
| `--accent` / `--accent-hover` | `#3b82f6` / `#2563eb` | 主操作蓝 |
| `--rx` / `--tx` | `#34d399` / `#fbbf24` | 接收绿 / 发送琥珀 |
| `--err` / `--ok` / `--warn` | `#f87171` / `#22c55e` / `#fbbf24` | 错误/正常/警告 |
| `--hl-search-bg` / `--hl-search-fg` | `rgba(59,130,246,.22)` / `#93c5fd` | 搜索命中高亮 |

关键词高亮调色板（多色并行高亮）见 `src/types/index.ts` 的 `KEYWORD_PALETTE`，新增关键词时循环取色。

### ANSI 颜色渲染（`src/composables/useAnsi.ts`）
- 日志视图常开渲染 SGR：重置/加粗、16 色（`--ansi-*`/`--ansi-bright-*` token，深浅主题各一套）、256 色（cube/灰阶公式转 `rgb()`）、真彩透传 `rgb()`（设备数据直出，属不走 token 的合理例外）；非 SGR CSI、游离 ESC、行尾未闭合段整体吞掉。
- 管线：`parseAnsi` 切样式游程 → 游程内跑 `segmentsFor` 叠加搜索/关键词高亮（高亮优先，`hlStyle` 覆盖 ANSI 色）→ LogView `rowSegments/segStyle` 渲染。
- 性能：无 ESC 行走 `indexOf` 快速路径；`rowMinWidth` 估宽对含 ESC 行按 `stripAnsi` 后长度。
- 限制：每行独立解析状态（嵌入式日志惯例行内自带 reset），跨行颜色延续不做；搜索/过滤/导出/HEX 仍基于含转义的原文。

### 字体
- UI 文字（标签/按钮/正文）：`--font-ui`，系统无衬线（Segoe UI 优先，Windows 友好，离线可用）
- 数据/日志/HEX/代码：`--font-mono`，等宽（Cascadia Code → JetBrains Mono → Fira Code → Consolas）
- **不要引入需联网的 Google Fonts**（桌面工具常离线运行）；新增字体只走系统栈或本地字体

### 圆角 / 阴影 / 过渡
- 圆角 `--r-sm` 6px / `--r-md` 8px / `--r-lg` 10px
- 阴影 `--shadow-sm` / `--shadow-md`（仅浮层用，日常靠边框分层）
- 过渡 `--t` 200ms、`--t-fast` 120ms，缓动 `--ease`；微交互 150–300ms
- 全局 `@media (prefers-reduced-motion: reduce)` 已禁用动画，新增动效不要绕过

### 主题（深/浅）
- 默认浅色（白）；深色可选，由 `:root[data-theme='light']` / 非 light 选择器覆盖同一组 token（仅覆盖值，类名不变）。默认值只在两处初始化：`index.html` 内联脚本与 `useTheme.loadTheme`，改默认必须同步两处。
- 钩子挂在 `<html data-theme>`：`index.html` 内联脚本在 CSS 前据 `serialtool.theme` 设好，防首屏闪；`useTheme` 组合式（`src/composables/useTheme.ts`）提供 `theme` ref 与 `toggleTheme`，PortBar 有切换按钮。
- **PlotView 用 `getComputedStyle(documentElement)` 读 token 并缓存**：切主题时已 `watch(theme)` 清缓存重绘；新增任何自读 token 的组件都要照此处理。
- 新增颜色一律走 token，**不要硬编码 rgba/hex**（`.log-row` 分隔线/hover 旧硬编码已改回 `var(--border)`/`var(--bg-hover)`）。

---

## 2. 交互与可访问性约定

- **图标只用内联 SVG**（Lucide 风格，`stroke="currentColor"`、`viewBox="0 0 24 24"`、`stroke-width` 正文图标 2 / 勾选 3 / chevron 2.5）。**禁止用 emoji 或 unicode 字符当图标**（旧的 `○◐●✕` 已清除）
- 所有可点击元素 `cursor: pointer`；hover 给颜色/背景反馈，**不要用会撑动布局的 scale 变换**
- `:focus-visible` 必须有可见焦点环（`box-shadow: 0 0 0 2px var(--bg-surface), 0 0 0 4px var(--border-focus)`）
- 纯图标按钮必须带 `title` + `aria-label`
- 文字对比度 ≥ 4.5:1（`--text-dim` 只用于提示/大字，正文用 `--text-muted` 以上）

### 复用控件（`src/styles.css` 已就绪，直接用类名）
- 按钮：`.btn` + 变体 `.btn-primary`(蓝) / `.btn-danger`(红) / `.btn-ghost`(透明) + 尺寸 `.btn-sm` + `.btn-icon`(方形)
- 下拉：`.select`（自带 chevron，`appearance:none`）
- 输入：`.input` / `.textarea`（`.input-mono` 套等宽）
- 复选框：`.check` > `input[hidden]` + `.box > svg` + 文字（蓝填充勾选动画，**不要用原生 checkbox 样式**）
- 分段控件：`.seg` > `.seg-item.active`（替代 radio，用于 ASCII/HEX 等二选一）
- 字段组：`.field` > `.field-label`（小号大写）+ 控件，端口栏用 `.field-port` 宽列

---

## 3. 布局结构（`src/App.vue`）

```
.app (column, 100vh)
 ├─ PortBar     ← 常驻单行：＋新建连接(弹层：数据源+参数+从预设) / 打开日志 / 设置
 │                （SettingsPopover 弹层按 日志/集成/视图 分组，二级视图同浮层切换；
 │                上次连接参数 localStorage 记忆，预设从设置或新建弹层应用）
 ├─ TabBar      ← 串口会话标签
 └─ .app-main (row, flex:1)
     ├─ .app-center (column)
     │   ├─ LogView   ← 工具栏 + 虚拟滚动日志(横向可滚) + 空状态
     │   └─ SendPanel ← 发送区（<details> 折叠条，默认收起）
     └─ .app-sidebar (312px, column, overflow-y:auto) ← 十个可折叠面板（全默认收起）
         ├─ SearchPanel        (搜索+紧邻其下的命中列表折叠节+过滤链编辑区)
         ├─ BookmarkPanel      行书签列表
         ├─ KeywordPanel
         ├─ AutoReplyPanel
         ├─ AlertPanel         告警规则+历史
         ├─ ConfigPresetsPanel 预设库(四类+JSON导入导出)
         ├─ ComparePanel       双会话时间对齐对比
         ├─ PlotConfigPanel
         ├─ AiNotesPanel       AI 批注（REST 写入→事件实时显示，可删/清空；跟随活动会话）
         └─ MatchStats         (显示名「监控」，纯流量迷你图，flex:1 内部滚动；
                                收起时 :not([open]) 回退 flex:0 0 auto 不占位)
```

### 侧栏面板约定
每个侧栏组件根元素是 `<details class="panel">`（**默认收起**，不加 `open`），`<summary class="panel-head">` 内含：`.panel-icon` + `.panel-title` + 可选 `.badge`(数量) + `.chevron`(展开旋转 90°)。
body 用 `.panel-body`；需限高滚动的用 `.kw-body` / `.ar-body`（已带 max-height + overflow）。

### Cargo workspace 布局
根 `Cargo.toml` 是 workspace（members：`crates/bytetide-core`、`crates/bytetide-cli`、`src-tauri`）；锁文件与构建产物统一在仓库根（根 `Cargo.lock` / 根 `target/`，src-tauri 下已无）。
- `crates/bytetide-core`：从 src-tauri 抽出的纯逻辑——`serial/{manager,port,rules}.rs`、`session.rs`（TSV 录制）、`logfmt.rs`（路径/时间戳模板）、`sink.rs`（`EventSink`），**100% 不依赖 tauri**
- `crates/bytetide-cli`：bin `bytetide`，无头监控（服务器 / ARM 板，musl 静态部署）；ring 的第二个消费者
- `src-tauri`：只剩 GUI 壳（commands / bridge / hotplug / gui_sink），业务实现全在 core

### 数据源与新增会话级字段
- `PortConfig.transport`：None/'serial' 串口；'tcp-client'/'tcp-server'/'udp' 网络源（serde default，旧 JSON 兼容）。后端串口/网络共用 `stream_loop`（core 的 `serial/manager.rs`），加新传输=扩展 `establish_link` 即可。
- 会话级 UI 偏好字段（showLineNo/showDir/droppedLines/bookmarks/aiNotes/alerts/filters…）**必须同时**改三处：`makeSession` 默认值、`reconnectSession` 的 carried 迁移清单、相关 actions。clearLog 会重置 lineCounter 与 droppedLines 并清空 bookmarks 与 aiNotes（aiNotes 同时回写后端镜像清空；droppedLines 在重连迁移中保留）。
- localStorage 键：`serialtool.theme/.lastPortConfig/.logConfig/.searchHistory/.portPresets/.configPresets/.alertSound/.update.lastCheck/.update.dismissedVersion`。预设库 payload 各类别形状校验在 `applyConfigPreset/importConfigPresets`。
- 更新检查（`useUpdateChecker`）：启动延迟 5s 静默查 GitHub Releases API（24h 节流，失败也记间隔）；`UPDATE_REPO` 常量已定 `RtuQ/bytetide`（与 scripts/portable-README.txt 主页链接联动，改一处必改另一处）。免安装版策略 = 只提示 + 跳转下载页，不做自更新；TitleBar 版本徽标在 `status==='available'` 时亮起，「忽略此版本」按 tag 记忆。
- **长跑性能红线**：`lines` 元素必须在 `appendLines`/`appendPulled` 处 `markRaw`（日志行不可变，禁 Proxy 开销）；侧栏折叠面板 body 仍处于挂载态，**禁止无守卫的全量行 computed**——折叠/空态必须早退或停算（参考 SearchPanel 命中节 hitsOpen、BookmarkPanel 空书签早退）。
- **数据流 = 拉模型（feat/pull-based-view 起）**：后端 ring 是唯一真相（`no` 游标单调递增、清屏不回退）；前端 `useTauriEvents` 每 200ms 按会话 `pullNo` 调 `ring_lines_no_cmd` 拉 delta（`appendPulled` 入表），**不再有 `log` 事件流**（40ms 推事件曾把 WebView2 渲染进程调度饿死成死亡螺旋，实测积压 15 分钟、1s 定时器饿到 48s 才醒）。渲染进程被节流时最坏滞后=一个拉取周期，醒来一次拉齐即收敛。新增实时数据通道时走游标拉取，勿回加高频 emit。CLI（bytetide-cli）是 ring 的第二个消费者：每 50ms 调 `ring_lines_after_no` 游标拉取、零事件流，新增消费者照此办理。
- **自动回复/告警 = Rust 侧评估（feat/pull-based-view 第 2 步）**：设备交互的正确性不依赖前端存活。规则存 `SessionHandle.auto_reply/alert_cfg`（前端 `pushLiveRules` 整体覆盖推送：连接时+规则变更时）；`stream_loop` 逐 RX 行在 core 的 `serial/rules.rs`（纯函数：`build_test_matcher` 语义对齐前端 `buildTestMatcher`、`auto_reply_payload`、`alert_eval` 窗口/冷却状态机）评估：自动回复命中在读线程内直接回写设备（TX 回显照常进 ring/落盘）；告警命中攒批写后端 mirror（REST `/alerts`）+ `alert-hit` 稀疏事件——通知/提示音/历史入 UI 在事件监听侧执行（`useTauriEvents`），行号用 LogLine.rn（ring no）回查 UI no。改规则语义时 Rust 与前端两处测试都要动。
- **事件出口 = `EventSink` trait（core 去 tauri 化）**：core 不再 import tauri；状态/错误/告警（`status`/`error`/`alert_hits`）经 `sink.rs` 的 `EventSink` trait 送出，桌面端 `src-tauri/src/gui_sink.rs` 转发为原事件名（`session-status`/`session-error`/`alert-hit`）。改事件名必须同步前端 `useTauriEvents`。`PortManager::connect` 现签名 `(config, log_config, sink: Arc<dyn EventSink>, sessions_dir)`，`sessions_dir` 传空 = 不落盘（CLI 默认）。
- **后台/锁屏不实时根因 = Windows EcoQoS**：进程后台化或锁屏时系统把窗口化进程降入节能队列，IPC 派发被推迟到数十秒级（症状：`batchMs` 仅 2-6ms 但 `lagMs` 飙到 30s）。治本在 `lib.rs::disable_power_throttling()`，进程启动即调 `SetProcessInformation(ProcessPowerThrottling, StateMask=0)` 退出限流；`windows-sys` 仅 Windows target 引入。哨兵 `usePerfWatch` 的 `DiagEntry.vis` 记录每条滞后发生时的窗口可见性，`hidden` 时滞后=系统限流，`visible` 时滞后=真积压，一眼可辨。注意：`--disable-features=CalculateNativeWinOcclusion` 生效时被遮挡窗口也报 `visible`，此时 vis 判读失效，需以后端 `perf-heartbeat.log` 对照。
- **存储位置规范**：会话录制（用户数据）在 `app_data_dir()/sessions/`（Roaming）；诊断日志（perf-frontend.log / perf-heartbeat.log）统一走 `lib.rs::open_diag_log` → `app_log_dir()`（Win: %LOCALAPPDATA%\<id>\logs；macOS: ~/Library/Logs/<id>；Linux: XDG state），超 5MB 打开时截断轮转。新增日志写点一律走 `open_diag_log`，勿再散落 app_data 根目录。
- **serialport feature 纪律**：core 的 serialport 是 `default-features = false` + 可选 `udev` feature——桌面端开（USB 元数据），CLI 永不开（musl 静态构建依赖此）。升级 serialport 时桌面 / CLI 两条线都要验证。

### 后续迭代计划（未排期）
- AI 分析入口：前端内嵌调用 REST 桥的对话式分析（离线选区→“让 AI 解释”）
- 时序回放（离线日志按原间隔重放为伪实时会话）
- 对比视图完整 diff 算法；tcp-server 多并发接入；UDP 对端发送

### 连接控制（停止 / 重连 / 关闭）
- **停止**：`store.stopSession(id)` —— 断开串口但保留标签页与日志，状态 → `disconnected`
- **重连**：`store.reconnectSession(id)` —— 用原配置重连，后端生成新 id，前端把日志/搜索/关键词/自动回复/历史迁移过去
- **关闭**：`store.closeTab(id)` —— 断开并删除标签页（彻底丢弃）

LogView 工具栏的按钮按 `active.status` 切换：`connected/connecting` → 红色“停止”；`disconnected/error` → 蓝色“重连”。
**不要再让“断开”等于“关闭”**——这是用户明确反对的旧行为。

标签页状态点（`.tab .dot`）用 CSS 颜色驱动：`disconnected` 灰 / `connecting` 琥珀脉冲 / `connected` 绿发光 / `error` 红。

---

## 4. 代码边界（重要）

UI 层与逻辑层严格分离：

| 层 | 文件 | 规则 |
|---|---|---|
| **core（Rust 纯逻辑）** | `crates/bytetide-core/**` | 串口/会话/ring/规则/落盘；桌面与 CLI 共用，改这里必须跑 core+src-tauri 两边测试 |
| **CLI** | `crates/bytetide-cli/**` | args / render / pick / input；纯函数单测同 core 规范 |
| **逻辑（改 UI 时勿动）** | `src/types/index.ts`、`src/stores/session.ts`、`src/composables/*`、`src/main.ts`、`src-tauri/src/**`（GUI 壳：commands/bridge/hotplug/gui_sink） | 业务/数据/事件逻辑。只做纯 UI 时不要改这里 |
| **UI** | `src/App.vue`、`src/components/*.vue` 的 `<template>`、`src/styles.css`、`index.html`、`src-tauri/tauri.conf.json`(窗口) | 自由改 |

- 改 `.vue` 时**只动 `<template>`**，`<script setup>` 原样保留，除非确需新增 UI 交互态（如本地的折叠/切换 ref）
- 新增功能需要 store 动作时（如 `stopSession`），**只新增 action，不改既有 action 的行为**
- 后端命令在 `src-tauri/src/commands.rs`，核心实现已移 `crates/bytetide-core`（`serial/manager.rs` 等）；前端经 `invoke` 调用。优先复用现有命令，避免改 Rust

### TypeScript 约束
`tsconfig.json` 开了 `noUnusedLocals` + `noUnusedParameters` + `strict`。
删模板里某变量/常量的最后一处用法时，必须同步删掉它在 `<script>` 里的声明或 import，否则 `vue-tsc` 报错、构建失败。
（例：把 unicode 状态点换成 CSS 后，删了 TabBar 的 `dot` 映射；搜索框换图标后，删了 SearchPanel 的 `SEARCH_COLOR` import）

---

## 5. 验证流程

改完必跑（顺序即依赖：测试先于构建，构建先于人工验布局）：
1. `npm run test` —— vitest 跑前端单测（`src/**/__tests__/*.test.ts`）；改了 `stores/`、`composables/` 必跑
2. `cargo test` —— **在仓库根跑**（Cargo workspace 全量，覆盖 bytetide-core / bytetide-cli / src-tauri 的内联 `#[cfg(test)]` 模块）；改了 Rust 必跑
3. `cargo run -p bytetide-cli -- --help` —— CLI 冒烟检查；改了 `crates/**` 必跑
4. `npm run build` —— `vue-tsc --noEmit` 类型检查 + `vite build`，两者都过才算改完
5. `npm run dev`（端口 1420，strictPort）浏览器看布局；接真实串口用 `npm run tauri dev`
6. 浏览器无 Tauri 后端时，`invoke`/`listen` 会失败，但空状态 UI 仍应正常渲染、不崩——这是冒烟检查
7. 虚拟滚动行高改了要同步改 `LogView.vue` 的 `:item-size` 与 `.log-row` 的 `height`（当前都是 22px）

### 测试规范（新增/修改功能必须遵守）
- **改逻辑层必须同步新增或更新测试**：`stores/`、`composables/`、`src-tauri/src/` 的纯函数 / 分支 / 边界用例补单测；只改 `.vue` 的 `<template>` 可只跑构建，但触及 store action 或 composable 计算逻辑的改动不算纯 UI
- 前端：被测文件同级 `__tests__/` 目录，命名 `*.test.ts`（例 `src/composables/__tests__/useLogParser.test.ts`）；用 vitest 的 `describe/it/expect`，`npm run test` 跑全量
- 后端：被测模块末尾内联 `#[cfg(test)] mod tests { use super::*; ... }`（例 `bridge.rs` 的帧解码 / 正则过滤用例）；只测私有纯函数，不触网 / 不经 axum / 不建 manager；`cargo test` 跑全量
- 改了逻辑但没补测试 = 未完成；测试红了不准靠“眼看没问题”跳过，先修测试或修代码再走构建

图标源图在 `scripts/brand-icon.png`（`make_icon.py` 生成 -> `npx tauri icon scripts/brand-icon.png` 出全套）。README 预览图（原 `docs/preview.png`）已应作者要求移除，待其自行补充新截图后恢复引用。
