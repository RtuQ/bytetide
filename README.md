# ByteTide · 字节潮

[简体中文](./README.md) | [English](./README.en.md)

面向嵌入式开发的串口 / 网络日志调试台。设备经串口、TCP 或 UDP 输出的每一行数据，都可在同一界面内实时查看、搜索、过滤、绘制波形，并可通过内置 REST 桥交由 AI 进行协议分析。

技术栈为 Tauri 2 + Vue 3 + Rust：安装包体积小、启动快、内存占用低，全部功能本地运行，不依赖任何云服务。原名 Serial Tool。

## 预览

<p align="center">
  <img src="docs/preview-light.png" alt="ByteTide 浅色主题" width="49%" />
  <img src="docs/preview-dark.png" alt="ByteTide 深色主题" width="49%" />
</p>

---

## 典型使用场景

- **传感器 / 模组调试**：设备周期性输出 `V: 220, C: 10, P: 200` 一类数据时，配置帧格式即可转为实时多通道波形，悬停任意数据点可查看接收时间、原始帧与各通道解析值。
- **协议分析**：通过内置 REST 桥（本机 HTTP 服务 + 令牌鉴权）将实时日志开放给 AI CLI，自动统计帧头 / 帧尾候选、校验方式与建议帧长，辅助协议逆向。
- **长时间压测监控**：对 `ERROR`、`ASSERT` 等关键字配置告警规则，命中即触发系统通知（可选提示音），支持次数聚合与冷却抑制，无需盯屏。
- **双设备行为对比**：两路会话按时间戳 ± 容差配对，左右并排展示，差异高亮、超差标红、双向跳转。
- **离线复盘**：日志自动落盘为 TSV；事后打开 `.log` 文件即为离线会话，除收发外全部功能可用。

## 快速开始

1. **安装**：从 [GitHub Releases](https://github.com/RtuQ/bytetide/releases) 下载安装包，或从源码构建（见[构建与开发](#构建与开发)）：
   ```bash
   npm install
   npm run tauri build   # 产物在 target/release/bundle/
   ```
2. **连接**：顶部端口栏选择数据源（串口 / TCP 客户端 / TCP 服务端 / UDP 监听），设置参数后点击「连接」，日志开始滚动。
3. **常用操作**：搜索框定位关键字；选中行后 `Ctrl+B` 添加书签；侧栏「数据绘图」配置帧格式查看波形；需要 AI 分析时开启「REST 桥接」。

> 无硬件环境时，通过「打开日志」载入示例文件 [sample-data/plot-demo.log](./sample-data/plot-demo.log)（600 帧双通道正弦波），即可体验除收发外的全部功能。

---

## 功能

### 连接管理

- **数据源**：串口（COM）、TCP 客户端 / TCP 服务端 / UDP 监听，三种网络源与串口体验一致
- **多标签页**：同时运行多个会话，互不干扰；分屏视图支持同屏 2–4 列对比观察
- **连接控制**：停止（保留日志与标签页）、重连（沿用原配置，日志 / 搜索 / 告警等会话状态随迁）、关闭（断开并移除标签页）
- 连接参数可保存为**预设**，一键套用

### 日志查看

- 虚拟滚动渲染，内存缓冲保留**最近 50,000 行**，超长行横向滚动完整可读
- 视图开关：跟随尾部、仅显示命中、HEX 视图、行间时间差、行号 / 收发方向列
- 底部状态栏实时显示收发字节数与速率

### 搜索与过滤

- **搜索**：关键字或正则，命中列表位于搜索框下方，点击跳转
- **过滤链**：多条「包含 / 排除」条件串联执行，等价于 `grep | grep -v`
- **关键词高亮**：多关键词独立配色与实时计数，与搜索互不影响

### 数据发送

- ASCII / HEX 双模式，`Ctrl+Enter` 发送，可选追加换行
- **定时循环发送**；发送历史点击回填

### 自动化

- **自动回复**：命中规则后自动回发应答，适用于查询-应答式协议的自动化测试；规则在 Rust 侧评估，不依赖前端界面存活
- **告警**：关键字 / 正则命中触发系统通知与可选提示音，支持窗口内次数聚合与冷却抑制；保留 100 条历史，点击回跳原行
- **行书签**：`Ctrl+F2` / `Ctrl+B` 标记关键行，侧栏列表统一管理
- **配置预设库**：过滤链 / 关键词 / 自动回复 / 帧格式四类配置命名保存，支持 JSON 导入导出，便于团队共享

### 实时监控

- 最近 60 秒行速率柱状图、字节速率曲线、行间隔 min / avg / p95 / max 统计

### 会话对比

- 两路日志按时间戳 ± 容差配对，左右并排展示；差异高亮、超差标红，任一侧点击即可双向跳转

### 数据绘图

将输入字节流按帧切分，解析为多通道数值并实时绘制波形。

帧格式可配置项：帧头 / 帧尾、校验方式（无 / 累加和 / XOR，单字节，作用于数据段）、通道数（1–16）、每通道字节数（1 / 2 / 4）、字节序、有无符号。

```
[帧头][数据: 通道数 × 每通道字节数][校验字节?][帧尾?]
```

示例：帧 `01 00 01 02 12 43`，帧头 `01 00`，2 通道 × 2 字节，大端 →
通道 0 = `0x0102` = **258**，通道 1 = `0x1243` = **4675**。悬停任意数据点可查看接收时间、原始帧与各通道解析值。

支持两种输入：二进制帧（raw 模式），或十六进制文本（如 `"01 00 12 43"`，ASCII hex 模式）。

### REST 分析桥

侧栏开启「REST 桥接」后，应用在本机启动 HTTP 服务（axum 实现，Bearer 令牌鉴权，默认绑定 `127.0.0.1:8765`，可选 `0.0.0.0` 供远程访问；`/health` 免鉴权）。外部程序——例如 AI CLI——即可通过以下接口访问会话数据：

| 端点 | 说明 |
|---|---|
| `GET /health` | 服务状态、版本、缓冲容量 |
| `GET /ports` | 枚举串口 |
| `GET /sessions`、`GET /sessions/:id` | 会话列表 / 详情（配置、状态、统计、落盘路径） |
| `GET /sessions/:id/lines` | 日志读取：按行号 / 区间 / `last` / `sinceNo` / `around` 选择，支持分页与 `format=csv\|tsv` 输出 |
| `GET /sessions/:id/follow` | 长轮询增量读取（`sinceNo` 游标，超时与单批上限可控） |
| `GET /sessions/:id/stats` | 收发行数与字节数统计 |
| `GET /sessions/:id/histogram` | 按时间桶统计行数 |
| `GET /sessions/:id/timing` | 行间隔统计（min / avg / p95 / max，超阈间隔逐条定位） |
| `GET /sessions/:id/infer` | 帧格式推断：帧头 / 帧尾候选、校验方式、建议帧长 |
| `GET /sessions/:id/decode` | 按帧格式（界面配置或参数覆盖）解码数值序列 |
| `GET /sessions/:id/value-hist` | 指定通道取值分布统计 |
| `GET /sessions/:id/bookmarks`、`/alerts` | 界面书签与告警历史（只读镜像） |
| `GET/POST/DELETE /sessions/:id/annotations` | AI 批注读写（幂等合并），写入后界面实时显示 |
| `GET/POST /sessions/:id/plot-config` | 绘图配置读取 / 写回，写回后界面即时生效 |
| `GET /sessions/:id/export` | 全量落盘日志流式导出（含内存缓冲已淘汰的行） |
| `POST /sessions/:id/send`、`/exchange` | 发送 / 发送并等待应答（默认关闭，需在面板中单独开启 allowSend） |

所有读接口共享同一组服务端过滤参数：`dir`（rx/tx）、`q`（子串，逗号分隔多词）、`re`（正则）、`hex` / `mask`（十六进制字节 / `?` 通配掩码）、`exclude`、`sinceMs` / `untilMs`（时间窗）。

典型工作流：开启桥接 → 将界面显示的 URL 与令牌提供给 AI → AI 读取日志并经 `/infer` 推断帧格式、经 `/decode` 验证 → 确认后将配置写回 `/plot-config`，界面即时呈现。

配套 AI 技能包位于 [skills/serial-tool-bridge](./skills/serial-tool-bridge)（标准 SKILL.md 格式），将该目录复制到 AI 助手的技能目录即可使用：ZCode 为 `~/.zcode/skills/`，Claude Code / Codex 等同理。

---

## CLI（bytetide）：无头监控

适用于无法运行桌面环境的服务器、树莓派 / ARM 工控板。GitHub Releases 的桌面版 `v*` 发布与 CLI 专用 `cli-v*` 标签均提供 aarch64 / x86_64 的 **musl 静态二进制** tarball，单文件部署，无运行时依赖；本地从源码构建：`cargo build -p bytetide-cli --release`。

```bash
bytetide list                                  # 列出串口
bytetide monitor                               # 不带源参数：终端里方向键选口
bytetide monitor -p /dev/ttyUSB0 --baud 115200 --ts
bytetide monitor --tcp 192.168.1.50:9000 --retry 5
bytetide monitor --tcp-listen 9000 --json      # 每行一个 JSON，脚本友好
bytetide monitor --udp 5140
```

交互发送：输入一行回车即按 ASCII 发送，`/hex AA 01` 发 HEX，`/mode ascii|hex` 切换模式，`/quit` 退出。

```
$ bytetide monitor
? 选择串口 › /dev/ttyUSB0 (USB Serial)
RX  [Boot] sensor-fw v2.1
TX  get temp           ← 敲回车直接发
RX  T=23.4C
TX  AA 01              ← /hex AA 01
^C
已断开：时长 00:03:12 · RX 1284 行 / 54.2 KB · TX 2 行 / 12 B · 录制 log.tsv
```

数据行走 stdout（`RX  ` 绿 / `TX  ` 琥珀两列），连接信息与 Ctrl-C 统计走 stderr——`| head`、重定向均安全（退出码 0/1/2）；`--no-color` 或 `NO_COLOR` 关闭着色。`-o/--record <路径|模板>` 录制为与桌面端一致的 TSV 格式，可拷回本机用「打开日志」离线复盘；默认不录制、不加时间戳。完整参数见 `bytetide --help`。

---

## 构建与开发

```bash
npm install

npm run tauri dev      # 开发模式：桌面应用 + 热重载
npm run tauri build    # 打安装包，产物在 target/release/bundle/
npm run build          # 仅前端：类型检查 + 生产构建
npm test               # 前端单测（40+ 例）
cargo test             # Rust 单测（仓库根执行，覆盖 core / cli / src-tauri）
cargo run -p bytetide-cli -- --help   # 本地体验 CLI
```

环境要求：Node.js 18+、Rust stable，以及 [Tauri 2 系统依赖](https://tauri.app/start/prerequisites/)。
Rust 侧为 Cargo workspace（`crates/bytetide-core` / `crates/bytetide-cli` / `src-tauri`）：cargo 命令在仓库根执行，锁文件与构建产物统一位于根 `target/`。
GitHub Actions 在 push / PR 时自动执行完整校验。

> 仅运行 `npm run dev`（无 Tauri 后端）可进行界面冒烟检查：空状态正常渲染，收发等功能不可用。

---

## 项目结构

```
bytetide/
├─ crates/               # Cargo workspace 成员
│  ├─ bytetide-core/     # 核心逻辑：串口 / 会话 / 环形缓冲 / 规则 / 落盘（不依赖 Tauri）
│  └─ bytetide-cli/      # bytetide 命令行版（无头监控）
├─ src/                  # 前端（Vue 3 + TypeScript + Pinia）
│  ├─ components/        # 界面组件：日志 / 发送 / 搜索 / 绘图 / 各侧栏面板
│  ├─ composables/       # 无界面逻辑：事件接入、解析器、绘图、告警音等
│  ├─ stores/            # 会话状态机、告警历史
│  └─ types/             # 类型与默认配置（单一事实来源）
├─ src-tauri/            # 桌面端 Rust 壳：invoke 命令、REST 桥、热插拔、事件转发
├─ sample-data/          # 演示日志
├─ docs/                 # 截图等文档资源
├─ skills/               # REST 桥 AI 技能包（复制到 ~/.zcode/skills/ 即可安装）
└─ AGENTS.md             # 代码约定（改代码前必读）
```

## 许可证

MIT，详见 [LICENSE](./LICENSE)。
