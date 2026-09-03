# 布局重构 V1 — 一步到位

> 状态：已评审待实施。**可交互预览见 `demo/layout-ui-demo.html`**（浏览器直接打开；
> 右上「标注改动」开关可在界面上定位六项改动；支持深/浅主题切换、视图四态切换、
> dock 页签/拖高/收起、侧栏拖宽/收起、日志↔图表分割条拖拽、模拟日志流）。
> 与 `plan-parser-v1.md` 相互独立、无实施顺序依赖；两者的联动点仅在「dock 解码页签」
> （见 §2-3）。

## 0. 背景与目标

现状布局的四个结构性别扭（评审结论）：

1. **顶部三行 chrome**（TitleBar 品牌行 / PortBar 按钮行—中间大面积空白 / TabBar）吃掉
   约 100px 垂直空间，对数据工具是纯损耗
2. **图表与日志互斥**（`plot.enabled` XOR）：看波形就看不到滚动日志，而「边看波形边盯
   日志」是串口调试的典型姿势；**对比视图**挤在 312px 侧栏里，表格天然要宽度
3. **伴随信息埋在侧栏**：告警历史、监控迷你图是「看日志时余光扫」的东西，却在第 9/10
   个折叠面板里；健康信号（丢行、渲染滞后）只在诊断日志里
4. **侧栏 10 面板单列无分组**（parser 落地后 11 个），找面板靠扫标题；开合状态每次
   启动归零

六项改动（一步到位，不分期）：

| # | 改动 | 一句话 |
|---|---|---|
| ① | 顶栏三行收两行 | PortBar 退役；标题行（品牌+全局动作）+ 独立标签行（＋新建/标签/打开日志） |
| ② | 视图四态 | 日志 / 分屏同屏 / 图表 / 对比，取消互斥；对比升中心视图 |
| ③ | 底部 dock | 解码 / 告警历史 / 监控，可拖高可收起；侧栏回归配置职责 |
| ④ | 底部状态栏 | 连接状态 / 速率 / 丢行 / 解析器 / 渲染健康常驻 |
| ⑤ | 侧栏分组粘头 | 查找 / 规则 / 数据 / 库 四组 |
| ⑥ | 开合记忆 | 面板展开状态 localStorage 持久化（默认仍全收起） |

明确不动：分屏多列（`SplitView` / `SessionColumn`，与本计划正交）、虚拟滚动与拉取
循环、后端（**零 Rust 改动**）、发送区折叠条形态。

## 1. 布局总图

```
.app (column, 100vh)
 ├─ TitleBar ← ①标题行：品牌 + 主题/设置 + 版本徽标
 ├─ TabBar   ← ①标签行：＋新建(弹层) + 会话标签(横滚) + 打开日志
 ├─ .app-main (row, flex:1)
 │   ├─ .app-center (column)
 │   │   ├─ LogView 工具栏 ← ②视图 seg（日志|分屏|图表|对比）+ 现有工具项
 │   │   ├─ .center-body (column)
 │   │   │   ├─ LogView 虚拟滚动（按视图模式伸缩）
 │   │   │   ├─ .hsplit 分割条（仅分屏态；拖拽调高度比）
 │   │   │   ├─ PlotView（按视图模式伸缩）
 │   │   │   └─ CompareView（对比态整块替换日志+图表）
 │   │   ├─ DockView ← ③页签（解码|告警历史|监控）+ 拖高 + 收起
 │   │   └─ SendPanel（现状保留）
 │   ├─ .sidebar-handle（拖宽/收起，现状保留）
 │   └─ .app-sidebar ← ⑤分组粘头：
 │        查找（搜索/书签） 规则（关键词/自动回复/告警规则）
 │        数据（图表配置/解析器） 库（预设/AI 批注）
 └─ StatusBar ← ④单行：状态点+参数 | RX/TX 速率 | 丢行 | 解析器 | 渲染滞后 | 行数
```

## 2. 各项规格

### ① 顶栏三行收两行（评审决策：标签独立成行）

- **标题行** TitleBar（38px）：品牌 + spacer + 主题切换 + 设置（SettingsPopover 触发）+
  版本徽标（更新检查现状保留）；drag-region 与 Windows 窗口控件处理现状不动
- **标签行** TabBar（34px）：`＋`（NewConnectionPopover 触发）+ 会话标签（横向滚动、
  状态点/关闭键沿用现状）+ spacer + 打开日志（icon）——「＋」与「打开日志」都会产生
  新标签，归标签行语义最顺；标签独占整行，宽度不再与品牌/动作争位
- `PortBar.vue` 退役：lastPortConfig 记忆、openLog、弹层挂载逻辑按归属并入
  TitleBar/TabBar；主题切换（useTheme）从 PortBar 迁入 TitleBar

### ② 视图四态（中心区）

- 会话级字段 `Session.centerView: 'log' | 'split' | 'plot'`（默认 `'log'`）——**三处
  同步**：`makeSession` 默认值、`reconnectSession` carried 清单、视图切换 action
- `plot.enabled` 语义保留（= 图表数据解析开关）；切到 `split`/`plot` 时若未启用则
  自动置 true（一次点击即出图，不去图表面板找开关）
- 分屏高度比：全局 `localStorage['serialtool.centerSplit']`（百分比，20–80 钳制），
  拖 `.hsplit` 时实时应用、mouseup 持久化（交互照抄侧栏手柄的 moved-while-down 模式）
- 对比：全局 `compareMode`（与现有 `splitMode` 同级的布局态），工具栏「对比」项切换；
  `CompareView.vue` 占中心区整块，吸收 ComparePanel 的会话选择/对齐设置到头部行，
  `ComparePanel.vue` 退役
- LogView / PlotView 组件内部不重构：App.vue 按 centerView 条件渲染，分屏态两者同屏
  （flex 高度由 centerSplit 驱动）；LogView 虚拟滚动行为零变化

### ③ 底部 dock

- `DockView.vue`：页签（解码 / 告警历史 / 监控）+ 上缘拖高（32px–55% 窗高钳制）+ 收起
  （收起仅留页签条）；状态 `localStorage['serialtool.dock']` { height, collapsed, tab }
- **解码页签**：本计划交付空态卡片（「导入解析脚本」入口提示 → 跳侧栏解析器面板）；
  `plan-parser-v1` 落地后由 decoded 数据填充（**ParserPanel 的解码列表迁至此**，侧栏
  面板保留脚本管理/试运行/统计——该联动已注记进 parser 计划 §3）
- **告警历史页签**：AlertPanel 拆分——侧栏留「告警规则」，历史列表迁入 dock（数据
  同源：告警事件监听 + REST mirror）
- **监控页签**：MatchStats 整体迁入（流量迷你图 / RX·TX 速率 / 行数 / 丢行），
  `MatchStats.vue` 退役（代码迁移非重写）；侧栏第 10 面板消失
- dock 展开时日志区 min-height 120px 保护；dock 内容滚动独立于日志虚拟滚动

### ④ 底部状态栏

- `StatusBar.vue` 单行 25px，跟随活动会话：状态点（CSS 驱动四态色，沿用标签页 dot 语义）
  + 端口/传输参数 | RX/TX 速率（`useRate` 已有）| 丢行计数（`droppedLines`，>0 红）|
  解析器状态（parser 未落地显示「未启用」，落地后点亮——本计划只留位）| 渲染滞后
  （`usePerfWatch` 最新 lag，>100ms 黄）| spacer | RX/TX 累计行数
- 纯展示组件，数据全部来自现有 store/computed，无新数据通道

### ⑤ 侧栏分组粘头

- App.vue 模板分四组，`.group-head` sticky（top:0，`--bg-surface` 底色 + 下边框）：
  **查找**（搜索 / 书签）、**规则**（关键词 / 自动回复 / 告警规则）、**数据**
  （图表配置 / 解析器）、**库**（预设 / AI 批注）
- 面板本体（`<details class="panel">` 约定）零改动；分组只是容器与粘头

### ⑥ 面板开合记忆

- `usePanelState.ts`：`localStorage['serialtool.panels']` = { panelId: open }；默认全
  收起（不写键即默认），异常 JSON 容错回默认
- App.vue 侧栏 `<details>` 改受控：`:open` 绑定 + `@toggle` 回写（toggle 事件内读
  `el.open` 刷新状态）；用户手动展开/收起即持久化

## 3. 数据与状态改动汇总

| 改动 | 位置 | 说明 |
|---|---|---|
| `Session.centerView` | `stores/session.ts` | 会话级，三处同步（makeSession / reconnectSession carried / action） |
| `store.compareMode` | `stores/session.ts` | 全局布局态（splitMode 同级先例） |
| `serialtool.centerSplit` / `.dock` / `.panels` | localStorage | 全局 UI 偏好；键名清单同步 AGENTS.md |
| clearLog 不清 centerView | — | 视图偏好非数据，clearLog 不触碰（重连 carried 携带） |

localStorage 键全表（增量）：`serialtool.centerSplit`、`serialtool.dock`、
`serialtool.panels`（既有 `serialtool.sidebar` 保留）。

## 4. 文件级改动清单

| 类型 | 文件 | 内容 |
|---|---|---|
| 新增 | `src/components/StatusBar.vue` | 底部状态栏（纯展示） |
| 新增 | `src/components/DockView.vue` | dock 容器：页签/拖高/收起/持久化 |
| 新增 | `src/components/DockDecode.vue` | 解码页签（空态卡片，parser 联动位） |
| 新增 | `src/components/DockAlerts.vue` | 告警历史（自 AlertPanel 迁出） |
| 新增 | `src/components/DockMonitor.vue` | 监控（自 MatchStats 迁移） |
| 新增 | `src/components/CompareView.vue` | 对比中心视图（吸收 ComparePanel 配置） |
| 新增 | `src/composables/usePanelState.ts` | 面板开合持久化 |
| 修改 | `src/App.vue` | 顶栏合一渲染 / 视图四态 / dock / 状态栏 / 分组侧栏挂载（script+template 均动——布局重构属功能变更，越过「纯 UI 只动 template」边界，已在此声明） |
| 修改 | `src/components/TitleBar.vue` | 吸收 PortBar 全局动作（主题/设置/版本），drag-region 保留 |
| 修改 | `src/components/TabBar.vue` | 吸收＋新建（NewConnectionPopover 触发）与打开日志 |
| 修改 | `src/components/LogView.vue` | 工具栏增视图 seg（模板为主） |
| 修改 | `src/components/PlotView.vue` | 分屏态高度自适应（样式为主） |
| 修改 | `src/components/AlertPanel.vue` | 拆出历史列表，保留规则编辑 |
| 修改 | `src/stores/session.ts` | centerView 三处同步 + compareMode |
| 修改 | `src/styles.css` | titlebar / hsplit / dock / statusbar / group-head 样式（全走 token） |
| 删除 | `src/components/PortBar.vue`、`ComparePanel.vue`、`MatchStats.vue` | 职责并入或迁移；删除时同步清 `noUnusedLocals` 报错的 import |
| 修改 | `AGENTS.md` | 布局图重绘（现图已过时：缺 TitleBar/SplitView/侧栏拖宽）+ localStorage 键 + 组件清单 |

## 5. 红线遵守（对照 AGENTS.md）

- 虚拟滚动 / `markRaw` / 拉取循环零触碰；dock 列表（告警/解码）照 decoded/alerts 同款
  FIFO + markRaw 约定（告警历史现有上限逻辑随迁）
- 折叠面板早退红线不变：dock 收起 = 不渲染 pane 内容（`v-if`，非 display:none 挂载态）
- 侧栏分组粘头不改变「面板 body 挂载态」语义，折叠守卫照旧
- 颜色/圆角/过渡全走 token；图标内联 SVG（Lucide 风格）；`prefers-reduced-motion` 下
  禁动画；hover 无 scale 布扰乱；`:focus-visible` 焦点环
- 对比/图表迁移不复制逻辑：CompareView 复用现有对比 store/composable，DockMonitor
  复用 MatchStats 的计算与 sparkline

## 6. 测试计划（改逻辑必带测试）

| 测试 | 覆盖 |
|---|---|
| `session.test.ts` 增 | centerView 默认值 / 重连 carried 迁移 / clearLog 不清 centerView；compareMode 切换 |
| `usePanelState.test.ts` 新 | 默认全收起 / 读写持久化 / 坏 JSON 容错 |
| `layout-persist.test.ts` 新 | centerSplit 与 dock 高度/收起态的钳制与恢复纯函数 |
| 对比/监控迁移 | 现有 ComparePanel / MatchStats 相关既有测试随组件迁移并保持绿色 |

验证顺序照 AGENTS.md：`npm run test` → `cargo test`（回归确认零破坏）→
`npm run build` → `npm run dev` 浏览器空态冒烟（无后端时全布局仍应渲染）→
`npm run tauri dev` 真机核对 drag-region / 窗口控件 / 拖拽手柄。

## 7. 实施顺序（一步到位，内部五步，每步可独立验证）

- [ ] **S1 状态与持久化**：centerView/compareMode/panels/dock/centerSplit + 全部单测
- [ ] **S2 顶栏两行 + 状态栏**：TitleBar 吸收 PortBar 全局动作、TabBar 吸收＋/打开日志、PortBar 退役、StatusBar 挂载
- [ ] **S3 视图四态**：LogView 工具栏 seg + split 同屏 + hsplit + CompareView（ComparePanel 退役）
- [ ] **S4 dock**：DockView 三页签（AlertPanel 拆分 / MatchStats 迁移 / 解码空态）
- [ ] **S5 分组粘头 + 开合记忆 + AGENTS.md 全量同步 + 全链路验证**

## 8. 风险

| 风险 | 评估 |
|---|---|
| TitleBar 控件密集后 drag-region 误触拖拽 | 控件不挂 drag-region 属性即可，现有模式；真机 `tauri dev` 核对 |
| Win 窗口控件与右侧按钮争位 | 保留现有控件预留宽度；按钮改 icon 化省宽 |
| dock 拖高挤压日志虚拟滚动 | min-height 120px + 55% 窗高上限；resize 不触碰行数据 |
| 分屏常驻图表的全量重解析成本 | usePlotData 已 300ms 节流且仅 plot.enabled；与现状「开图表」成本相同 |
| `<details>` 受控 open 的 Vue 兼容 | `:open` + `@toggle` 回写为已知模式；S5 单独验证点 |
| 迁移类删除引发 noUnusedLocals 连锁 | 删组件时同步清 import（AGENTS 已知坑），`npm run build` 兜底 |
