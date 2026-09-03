# 万能 Hex 解析引擎 — V1 实现计划

> 状态：已评审待实施。**2026-09-03 评审修订**：切帧移回主线程、新增声明式字段层、
> 按批看门狗 + 错误率熔断、CRC/长度域语义钉死。
> 交互原型见 `demo/parser-ui-demo.html`（空态导入 → 试运行 → 启用 →
> 解码列表/行内徽章 → 查看/卸载 全流程可点）。
> 本文是 V1 的唯一实施依据；落地后 ABI 部分拆为 `docs/parser-spec.md` 长期维护。

## 0. 背景与定位

串口/网络调试中大量场景是「设备发十六进制帧，人脑查协议文档翻译字段」。本功能让用户
导入一份解析脚本（通常由 AI 依据协议文档线下生成），软件按脚本把 RX/TX 帧实时翻译为
结构化字段 + 自然语言句子。

分工原则：**引擎管切帧与常规翻译，脚本只兜底**。粘包/断帧/长度域/校验由内置引擎处理
（人写、久经考验）；80% 协议的「读字段 → 说人话」用**声明式字段表 + 文本模板**表达
（零代码执行、可 schema 校验、AI 生成可靠）；只有条件布局等复杂协议才写 JS `parse`
（Worker 沙箱执行）。出错面最小，且声明式层到 V3 可直接进 Rust。

分期：

| 期 | 内容 | 状态 |
|---|---|---|
| V1（本计划） | 本地导入脚本（声明式为主 / JS 兜底）+ 主线程切帧引擎 + Worker 脚本层 + 协议解析面板 + 规范文档（含 AI prompt 模板，线下用） | 待实施 |
| V2 | 应用内 AI 生成解析器（REST 桥调 LLM + 真机数据试运行闭环） | 未排期 |
| V3 | Rust 侧运行时（rquickjs 跑 JS 层；声明式脚本直接 serde + 字段抽取）、告警规则引用解码字段、encode 构包（声明式字段表反向填值）、多脚本槽 | 未排期 |

V1 明确不做：AI 应用内生成、TX 按字段构包、Rust 侧解码（**零后端改动**）。

## 1. 脚本 ABI（`bytetide.parser v1`）

两层结构：**声明式层（推荐）+ 脚本层（兜底）**。`parse` 存在时引擎走脚本层并忽略
fields/text；两层都没有 = 合法的**纯切帧器**（只切帧展示原始 hex，零执行、零 Worker）。

```js
export default {
  meta: { name: '温控协议', version: '2.1', author?, description? },

  // 切帧声明，引擎执行；脚本不处理粘包/断帧
  framing: {
    source: 'binary' | 'ascii-hex',   // 与 PlotSource（src/types/index.ts）同字面量
    sync:  'AA 55',                   // 同步字，可空
    length:
      { kind: 'fixed', value: 12 }            // 定长
    | { kind: 'field', at: 3, fmt: 'u8'|'u16'|'u32', endian?, add: 4 } // 长度域
    | { kind: 'until', tail: '0D 0A' }        // 分隔符结尾
    | { kind: 'line' },                       // 文本协议整行一帧
    crc: { algo: 'sum8'|'xor8'|'crc16-modbus'|'crc16-ccitt-false'
         |'crc16-xmodem'|'crc16-kermit'|'crc32',
           at: 'tail:N', endian? },  // 可空；校验失败帧计入警告不进翻译
    maxSize: 4096,                   // 超大帧保护（默认 4096）
  },

  // —— 声明式层（零代码执行，主线程引擎直接求值）——
  type:   { at: 2, fmt: 'u8', map: { 1: '状态上报', 2: '设置响应' } }, // 可选，缺省 = meta.name
  fields: [
    { label: '温度', at: 4, fmt: 'i16', scale: 0.1, unit: '℃' },
    { label: '模式', at: 6, fmt: 'u8', map: { 0: '待机', 1: '制冷' } },
  ],
  text: '温度 {温度}℃，模式{模式}',   // 可选；缺省自动拼接 label=value(unit)

  // —— 脚本层（兜底，Worker 沙箱执行）：完整帧字节 → 解码结果 ——
  parse(frame: Uint8Array, ctx): ParseResult | null
  // ctx = { ts, epochMillis, dir: 'rx'|'tx', sessionName, bt }
  // bt = 读数 u8/u16/u32/i8/i16/i32/f32、crc(algo, bytes)（与 framing 同算法集）、hex
  // ParseResult = { type, text, fields?: [{label, value, unit?, raw?}], warn? }
  // 抛错 / 返回 null → 该帧计入「解析错误」统计，管线不崩
}
```

钉死语义（决策记录，`parser-spec.md` 逐条展开 + 示例对照）：

- **偏移与 frame 边界**：所有偏移 0 基；`frame` = 线上完整帧（含 sync、长度域、CRC
  字节），`fields.at` / `length.at` 相对 frame[0]
- **长度域**：总帧长 = 长度域值 + `add`（`add` 补偿值未覆盖的 sync + 头部长度）
- **CRC**：覆盖范围 = frame 去掉 CRC 字节自身；`at` 仅支持 `tail:N`（帧尾倒数起算）；
  V1 不支持中位 CRC / 子范围覆盖（spec 写明限制）
- **CRC 命名**：算法别名按 reveng catalog 参数表实现（多项式/初值/refin/refout/xorout
  全钉死，spec 附参数表），杜绝 `crc16-ccitt` 一名多义的生成歧义
- **fmt / endian**：`u8/u16/u32/i8/i16/i32/f32`；endian 缺省 `little`；
  value = raw × scale + offset，`map` 命中时显示映射值（`raw` 保留）
- **framer 重同步策略**：长度域值 + 超限或帧校验失败时——有 sync 则滑窗找下一个
  sync 重同步（沿用 parseFrames「head mismatch 前进一字节」语义）；无 sync 的
  定长/长度域协议丢弃整帧并复位缓冲，计警告（无 sync 时坏长度值无法可靠重同步，
  spec 写明此权衡）

交付物：

- `docs/parser-spec.md`：ABI 规范（含上述语义钉死清单）+ **给 AI 的 prompt 模板**
  （用户拿协议文档 + 模板喂给任意 AI，线下生成脚本——V1 的 AI 协作形态）+ 示例脚本
  两份（声明式 / parse 兜底）
- `src/parser/parser-abi.d.ts`：供脚本作者 / AI 引用的类型定义

## 2. 架构与数据流

```
拉取循环（useTauriEvents 200ms，保持不动）
  → store.appendPulled 入表（现状）
  → parserEngine.feed(sessionId, lines)          主线程同步执行，线性扫描毫秒级
      lineBytes 还原字节 → framer 状态机（Map 按 (sessionId, dir) 隔离）
      ├─ 声明式脚本：fields 抽取 + 模板渲染，同批同步出结果
      └─ 脚本层脚本：完整帧攒批 → worker.parseBatch    fire-and-forget
         ← { type:'results', sessionId, gen, results } 异步回调（gen 不符则丢弃）
  → store.applyDecoded(sessionId, frames)        decoded 独立存储，markRaw + 1000 条 FIFO
```

已定决策（含评审修订）：

- **切帧在主线程，Worker 只执行脚本层 `parse`**。不可控的只有任意 JS；线性字节扫描
  不是风险——先例：`usePlotData` 每 300ms 对全量行重跑 `parseFrames` 一直在主线程
  且健康（当年 WebView2 事故根因是高频事件调度饿死渲染，不是线性 CPU 工作）。收益：
  行序天然由拉取循环保证（历史回溯与实时 feed **无竞态**）、Worker 队列丢的是完整帧
  **不脏流**、看门狗 terminate **不连坐切帧**、重载脚本可拿最近完整帧即时重解。
- **方向隔离**：framer 状态按 (sessionId, dir) 独立——TX 回显与 RX 行在日志表交织，
  单状态机会把两路字节流切碎。
- **全局单脚本**：V1 一份脚本 + 全局启停，多会话共享；会话级多脚本槽留 V3。
- **离线回放免费获得**：离线 .log 会话走同一条拉取循环（`create_offline_session_cmd`
  建真 ring），解码自动覆盖；导入/启用脚本时对**最近 2000 行**回溯补解码（2× FIFO
  上限——更早的行解码了也会被淘汰，省主线程/Worker 时间）。
- **纯模块**：`framer.ts` / `fields.ts` 是主线程纯函数直接跑 vitest，不 mock Worker；
  Worker（`bootstrap.ts`）只是脚本层宿主壳。
- **字节还原共享**：`lineBytes` 三态逻辑（原始 bytes → TextEncoder 回退 →
  ascii-hex 抽取）抽 `src/parser/lineBytes.ts`，`usePlotParser` 改 import。
- **reset 触发点**：`clearLog` 在 action 末尾调注入的 onClear 回调（useParserEngine
  注册，避免 store→engine 循环依赖）；closeTab / reconnect 由 useParserEngine 对
  `order` 做 diff watch 清旧 id 条目。reset 时会话**代际号 gen+1**，旧 Worker 在途
  结果按 gen 丢弃（terminate 重建后的迟到回包不污染新状态）。
- **脚本持久化**：`localStorage['serialtool.parserScript']`（源码 + meta + 启用态），
  启动静默恢复；键名同步进 AGENTS.md 清单。
- **试运行三分类**：会话无 RX 数据 → 跳过直接启用；有数据但 0 完整帧或 CRC 全败 →
  停在「已导入未启用」+「framing 疑似配置错误」横幅（不自动启用坏脚本）；有帧且解析
  通过 → 自动启用。
- **看门狗（按批）**：每批带递增 reqId，超时无 ack 即 terminate 重建 Worker + 卸载
  脚本 + 面板报错（防 AI 脚本死循环）。
- **错误率熔断**：滚动 1000 帧解析错误率 > 30% → 自动停用 + 横幅（用户可重新启用）。
  防「能跑但对 90% 帧抛错」的 AI 半坏脚本。
- **CSP 耦合记录**：`tauri.conf.json` 现为 `csp: null`，blob Worker + 动态 `import()`
  成立；**将来若引入 CSP 必须含 `script-src blob:` 与 `worker-src blob:`**，否则脚本层
  静默挂掉（声明式层不受影响）——此条写进 AGENTS.md 红线。

## 3. UI 规格

按 `demo/parser-ui-demo.html` 落地：

- `ParserPanel.vue`：`<details class="panel">` 约定（icon + 标题 + 帧数 badge + chevron）
  - 空态：选择脚本文件（`.js`） / 拖拽导入（面板虚线高亮） / 内置示例脚本
  - 脚本卡片：名称/版本、framing 摘要、声明式脚本附字段数、启用开关、查看 / 重载 / 卸载
  - 三格统计：总帧数 / 解析成功率 / 消息类型数
  - 解码列表：自然语言句子 + 类型色条；点行展开字段表（中文名/值/单位/`raw` 位置标注）；
    ◎ 按钮定位日志原文行（`decoded.no → LogLine.no` 直查）并闪烁；跟随/清空
    （联动注记：`docs/plan-layout-v1.md` 底部 dock 落地后，解码列表迁至 dock「解码」
    页签，ParserPanel 保留脚本管理/试运行/统计）
  - 纯切帧模式：无 fields/parse 的脚本，列表显示原始帧 hex + 帧长 + CRC 状态（「先看看
    帧长什么样」的调试前中期形态，切帧器本身即交付价值）
  - 查看器弹层：只读脚本预览 + 试运行报告（正式版可编辑，V1 只读）
- LogView 行内类型徽章：decoded 异步到达，用节流后的 `shallowRef Map<no, tag>` 驱动
  可见行重渲染；**Map 从 decoded FIFO 派生**（淘汰一致性免同步）。**可砍项**：若引发
  可见行全量重算则 V1 砍掉，挪 V1.1。
- 挂载：`App.vue` 侧栏 KeywordPanel 之后（template only）。

## 4. 文件级改动清单

| 类型 | 文件 | 内容 |
|---|---|---|
| 新增 | `src/types/parser.ts` | `DecodedFrame` / ABI 类型（含 fields/type 声明） |
| 新增 | `src/parser/lineBytes.ts` | 从 `usePlotParser` 抽出（三态字节还原） |
| 新增 | `src/parser/framer.ts` | 主线程流式切帧状态机纯函数（sync/定长/长度域/until/line + 7 种校验算法 + maxSize + 重同步策略 + (session,dir) 隔离） |
| 新增 | `src/parser/fields.ts` | 声明式字段抽取 + 模板渲染 + type 字段（纯函数） |
| 新增 | `src/parser/bootstrap.ts` | Worker 引导（bt 标准库、沙箱覆写 fetch/XHR/WebSocket/EventSource/Worker、消息循环；importScripts 在 module worker 本不存在，覆写无害） |
| 新增 | `src/parser/engine.ts` | 主线程编排：framer Map / 声明式求值 / Worker 生命周期 / reqId 看门狗 / gen 代际 / 熔断（协议处理为可测纯函数） |
| 新增 | `src/composables/useParserEngine.ts` | 加载/卸载/启停/订阅/localStorage 恢复/reset 挂载（onClear 注入 + order diff watch） |
| 新增 | `src/components/ParserPanel.vue` | 面板（含查看器弹层） |
| 新增 | `docs/parser-spec.md` | ABI 规范（语义钉死清单）+ AI prompt 模板 + 示例脚本两份 |
| 修改 | `src/stores/session.ts` | Session 增 `decoded`；**三处同步**：`makeSession` 默认值、`reconnectSession` 迁移清单（decoded 不跨会话携带，重连为空）、`applyDecoded` action、`clearLog` 清 decoded + 调 onClear 回调 |
| 修改 | `src/composables/useTauriEvents.ts` | 拉取循环尾部 feed（几行） |
| 修改 | `src/composables/usePlotParser.ts` | `lineBytes` 改 import（行为零变化） |
| 修改 | `src/App.vue` | 侧栏挂 `ParserPanel`（template only） |
| 修改 | `AGENTS.md` | 布局图 / localStorage 键 / 红线清单（含 CSP blob: 耦合）同步 |

## 5. 性能与安全红线（对照 AGENTS.md）

- 主线程 feed = 线性批量（每拉取周期一批），无帧时零工作；切帧成本对照 usePlotData
  全量重解析先例可忽略
- Worker 只收**完整帧**（背压丢帧 = 丢结果不脏流）；队列上限 1000 帧/批，超出丢最旧
  并计警告
- decoded 元素 `markRaw`；1000 条/会话 FIFO；`applyDecoded` 200ms 节流批量入表
- 声明式层**零代码执行、零沙箱面**（Worker 挂掉/CSP 受限时仍可用）；脚本层沙箱：
  Worker 无 DOM，bootstrap 覆写 `fetch`/`XMLHttpRequest`/`WebSocket`/`EventSource`/
  `Worker`；按批看门狗 terminate；错误率熔断。信任模型 =「只加载审过码的脚本」，
  文档写明
- 折叠面板照旧早退/停算红线；解码与面板折叠无关（数据一致性优先）

## 6. 测试计划（按仓库规范：改逻辑必带测试）

| 测试 | 覆盖 |
|---|---|
| `parser-framer.test.ts` | 粘包/断帧/跨行帧/垃圾前缀/定长/长度域大小端/until/line/7 种校验算法/坏长度值重同步（有 sync 滑窗 vs 无 sync 复位）/超大帧丢弃/(session,dir) 隔离/丢帧不脏流——核心资产，用例从重 |
| `parser-fields.test.ts` | 字段抽取/模板插值/map 优先于 scale/f32/越界帧安全返回/type 字段缺省回退 |
| `lineBytes.test.ts` | **新写**（usePlotParser.test 现无 lineBytes 覆盖，非搬运）：三态 + TextEncoder 回退分支 |
| `engine-protocol.test.ts` | schema 静态校验（缺 meta/坏 framing/fields 越界/非 ESM default export）/试运行三分类/gen 代际丢弃迟到结果/熔断阈值/reqId 看门狗 |
| `session.test.ts` 增 | applyDecoded / clearLog 清 decoded + onClear 回调 / 重连迁移 decoded 为空 |

验证顺序照 AGENTS.md：`npm run test` → `cargo test`（回归确认零破坏）→
`cargo run -p bytetide-cli -- --help` → `npm run build` → `npm run dev` 浏览器空态冒烟。

## 7. 实施顺序（四步，每步可独立验证）

- [ ] **S1 纯逻辑**：ABI 类型 + framer 状态机 + fields 抽取/模板 + 全套测试
  （不碰任何现有文件）
- [ ] **S2 抽取**：lineBytes 迁出 + usePlotParser 改 import + 新写测试（行为零变化）
- [ ] **S3 引擎与集成**：engine（主线程编排 + Worker 生命周期 + gen/reqId/熔断）+
  bootstrap + useParserEngine + store 三处同步与 onClear + useTauriEvents 挂钩 + 测试
- [ ] **S4 UI 与文档**：ParserPanel + App.vue 挂载 + parser-spec.md（含语义钉死清单）+
  AGENTS.md 同步（含 CSP 红线）+ 全链路验证

## 8. 风险

| 风险 | 评估 |
|---|---|
| blob-URL module Worker（WebView2/WKWebView） | `csp: null` 现状下标准特性成立，浏览器开发模式同为主冒烟路径；且声明式层不依赖 Worker，脚本层挂掉时切帧/字段翻译仍有兜底。真正风险 = 将来加 CSP 忘带 `blob:`——已列 AGENTS.md 红线备忘 |
| LogView 行内徽章响应性成本 | 已标可砍项，实现时以可见行重渲染范围为准 |
| AI 线下生成脚本质量参差 | 四道防线：声明式 schema 校验（导入即拦）+ 试运行三分类 + 按批看门狗 + 错误率熔断 |
