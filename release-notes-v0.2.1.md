ByteTide v0.2.1：全新无头监控 CLI 上线 + 数据流拉模型重构（v0.2.0 功能集重新打包，补齐 macOS ARM CLI 包）。

## ✨ 新功能

- **无头监控 CLI（`bytetide`）**：与桌面版共用同一核心，串口 / TCP / UDP 监控，交互式发送（ASCII / HEX 随时切换）、TSV 录制、断线自动重连；数据走 stdout、提示走 stderr，方便管道与脚本
- **ANSI 颜色渲染**：日志视图直出设备 ANSI 颜色（16 色 / 256 色 / 真彩 + 加粗），搜索 / 关键词高亮优先显示
- **REST 桥 `/follow` 服务端过滤**：长轮询支持全套过滤字段，AI 技能只拉匹配的行

## ⚡ 性能与架构

- **数据流拉模型重构**：后端 ring 唯一真相，前端按游标拉增量，修掉旧事件推送在 WebView2 下的调度饿死问题，高吞吐长时间挂机稳定收敛
- **自动回复 / 告警迁入 Rust 读线程**：设备交互不再依赖前端存活，前端只做通知呈现
- **拆分 bytetide-core**：核心逻辑与 GUI 壳解耦，桌面版与 CLI 共用同一实现

## 🛠 修复

- 启动白屏、日志横向滚动条与内容对齐、清屏后「已丢弃 N 行」提示未归零

## 📦 下载

| 文件 | 平台 |
| --- | --- |
| `ByteTide.exe` | Windows 10/11 x64（免安装） |
| `ByteTide_0.2.0_aarch64.dmg` | macOS Apple Silicon（文件名内嵌版本号仍为 0.2.0，实际为 v0.2.1 构建） |
| `bytetide-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon CLI |
| `bytetide-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64 CLI（musl 静态） |
| `bytetide-aarch64-unknown-linux-musl.tar.gz` | Linux ARM64 CLI（musl 静态） |

> 未签名：macOS 首次打开被拦时右键选「打开」放行；Windows SmartScreen 提示选「仍要运行」。反馈请到 [Issues](https://github.com/RtuQ/bytetide/issues)。
