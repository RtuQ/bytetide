ByteTide v0.2.0：把监控能力带出桌面。全新的无头 CLI 让服务器和 ARM 板也能挂上同一套串口 / 网络监控；同时数据流整体重构为拉模型，高吞吐长时间挂机不再越跑越卡，日志视图开始"看懂"设备输出的 ANSI 颜色。

## 🚀 新功能

**无头监控 CLI（`bytetide`）**
- 与桌面版共用同一核心的命令行工具：串口（波特率 / 数据位 / 校验 / 停止位 / 流控全参数）、TCP 客户端 / 服务端、UDP 监听，`bytetide list` 列出可用串口
- 交互式使用：stdin 为终端时可直接发送（ASCII / HEX 随时切换），Ctrl-C 或 `/quit` 退出；数据走 stdout、提示走 stderr，方便重定向与管道
- `--record` 按桌面端同款时间戳模板落盘录制，`--ts / --ts-format` 逐行时间戳，`--retry` 断线自动重连
- 提供 musl 静态编译产物（x86_64 / aarch64），解压即用、无动态库依赖，适合服务器 / 树莓派等开发板无头部署

**ANSI 颜色渲染**
- 日志视图直接渲染设备输出的 ANSI 颜色：SGR 加粗、16 色 / 256 色 / 真彩（RGB）透传，嵌入式日志原样上屏
- 搜索命中与关键词高亮优先于 ANSI 色显示；过滤、导出、HEX 仍基于原文，行为不变

**REST 桥 `/follow` 服务端过滤**
- 长轮询接口支持与前端一致的全套过滤字段（含 filterLimit 上限保护），AI 技能可以只拉匹配的行，不再整流转发

## ⚡ 性能与架构

- **数据流重构为拉模型**：后端环形缓冲成为唯一真相，前端每 200ms 按游标拉增量，替代原先 40ms 高频事件推送。旧架构在 Windows WebView2 下可能把渲染进程调度饿死（实测出现过 15 分钟积压、1 秒定时器 48 秒才醒），新架构被节流时最坏只滞后一个拉取周期，醒来一次拉齐即收敛
- **自动回复 / 告警迁入 Rust 读线程**：规则匹配与自动回发在 Rust 侧评估，设备交互的正确性不再依赖前端存活；前端只负责通知、提示音与历史展示
- **拆分 bytetide-core**：串口 / 会话 / 规则 / 落盘等纯逻辑从 GUI 壳中拆出（core 不依赖 tauri），桌面版与 CLI 共用同一实现，修一处两端生效

## 🛠 修复

- 消除启动白屏：窗口就绪完成后再显示
- 日志横向滚动条与内容对齐（时间戳纳入列宽估算）
- 清屏后「已丢弃 N 行」提示未随缓冲归零

## 🧹 界面与杂项

- 端口栏重构：新建连接改为弹层，公共配置归入设置；REST 桥配置移至全局区，AI 批注独立为侧栏面板
- 诊断日志统一写入系统日志目录，超 5MB 自动轮转；性能取证探针仅存于 DEV 构建，正式包不再携带
- CI 流水线 workspace 化：打 `v*` tag 一键产出桌面安装包 + CLI musl 全部产物

## 📦 下载

| 文件 | 平台 | 说明 |
| --- | --- | --- |
| `ByteTide.exe` | Windows 10/11 x64 | 免安装单文件，下载即用 |
| `ByteTide_0.2.0_aarch64.dmg` | macOS（Apple Silicon） | 标准磁盘镜像，拖入 Applications 即可 |
| `bytetide-aarch64-apple-darwin.tar.gz` | macOS（Apple Silicon） | 无头 CLI，Apple Silicon 原生二进制，解压即用 |
| `bytetide-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64 | 无头 CLI，musl 静态链接，解压即用 |
| `bytetide-aarch64-unknown-linux-musl.tar.gz` | Linux ARM64 | 无头 CLI，musl 静态链接，适合树莓派 / ARM 开发板 |

> 从 v0.1.0 升级直接替换即可，会话录制与各项预设等用户数据不受影响。

> 手头没设备？仓库自带 [sample-data/plot-demo.log](./sample-data/plot-demo.log)（600 帧双通道正弦波），打开日志载入即可体验全部功能。

## ⚠️ 注意事项

- 当前未做代码签名：
  - **macOS** 首次打开若被 Gatekeeper 拦截，右键 App 选「打开」，或到 系统设置 → 隐私与安全性 里放行
  - **Windows** SmartScreen 提示时选「更多信息 → 仍要运行」
- macOS 桌面版仅提供 Apple Silicon（M 系列）版本，Intel Mac 需[从源码构建](./README.md#构建)
- CLI 提供 Linux（musl 静态）与 macOS（Apple Silicon）产物，其他平台可从源码构建：`cargo build -p bytetide-cli --release`

## 🔨 从源码构建

```bash
npm install
npm run tauri build        # 桌面版，产物在 src-tauri/target/release/bundle/
cargo build -p bytetide-cli --release   # CLI
```

环境要求与细节见 [README](./README.md#构建)。问题反馈请到 [Issues](https://github.com/RtuQ/bytetide/issues)。

---

English: ByteTide v0.2.0 takes monitoring beyond the desktop. A new headless CLI (`bytetide`) shares the same Rust core as the GUI — serial (full port parameters), TCP client/server and UDP monitoring with interactive sending (ASCII/HEX), timestamped TSV recording, auto-reconnect, and fully static musl builds for x86_64/aarch64 Linux servers and SBCs. The data pipeline was reworked to a pull model (backend ring buffer as the single source of truth, 200ms cursor-based deltas instead of 40ms event floods), fixing WebView2 renderer starvation under sustained high-throughput sessions; auto-reply and alert rules are now evaluated in the Rust read thread so device interaction no longer depends on the UI. The log view renders ANSI colors (SGR bold, 16/256/true-color), the REST bridge's `/follow` long-poll endpoint gained server-side filtering, and startup white-flash, horizontal scrollbar alignment and clear-log counter bugs were fixed. Fully offline. See the [English README](./README.en.md) for details.
