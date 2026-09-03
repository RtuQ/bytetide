ByteTide v0.3.0：界面布局大重构——顶栏两行 / 日志图表分屏 / 底部 dock / 常驻状态栏。

## ✨ 新布局（一步到位）

- **顶栏三行收两行**：标题行（品牌 + 设置 + 版本）+ 独立标签行（＋新建 / 会话标签 / 打开日志），省出一整行给数据区
- **视图四态**：日志 / 分屏 / 图表 / 对比 自由切换——**看波形的同时不再丢掉滚动日志**（可拖分割条调高度，记忆比例）；双会话时间对齐对比升为中心视图，不再是窄侧栏表格
- **底部 dock**：解码（为解析器功能预留）/ 告警历史 / 监控 三页签，可拖高、可收起——伴随信息不再埋在侧栏深处
- **常驻状态栏**：连接状态、RX/TX 速率、丢行计数、渲染健康一屏可见
- **侧栏分组**：查找 / 规则 / 数据 / 库 四组粘性分组头；面板开合状态重启记忆（默认仍全收起）

## ⚡ 行为细节

- 显式启用图表自动切到图表视图、关闭回落日志——开关始终看得见效果
- 布局偏好（侧栏宽度/收起、分屏比例、dock 高度/页签、面板开合）全部本地记忆
- 多列分屏、虚拟滚动、拉取数据流零改动；本版本零后端改动

## 📦 下载

| 文件 | 平台 |
| --- | --- |
| `ByteTide.exe` | Windows 10/11 x64（免安装） |
| `ByteTide_0.3.0_aarch64.dmg` | macOS Apple Silicon |
| `bytetide-aarch64-apple-darwin.tar.gz` | macOS Apple Silicon CLI |
| `bytetide-x86_64-unknown-linux-musl.tar.gz` | Linux x86_64 CLI（musl 静态） |
| `bytetide-aarch64-unknown-linux-musl.tar.gz` | Linux ARM64 CLI（musl 静态） |

> 未签名：macOS 首次打开被拦时右键选「打开」放行；Windows SmartScreen 提示选「仍要运行」。反馈请到 [Issues](https://github.com/RtuQ/bytetide/issues)。
