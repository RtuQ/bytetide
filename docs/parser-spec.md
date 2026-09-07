# bytetide.parser v1 规范

串口/网络调试中大量场景是「设备发十六进制帧，人脑查协议文档翻译字段」。本规范定义的解析器
让用户**线下**把协议文档喂给任意 AI 生成一份解析脚本，导入后引擎实时把 RX/TX 帧翻译为
结构化字段 + 自然语言句子。分工原则：**引擎管切帧与常规翻译，脚本只兜底**——粘包/断帧/
长度域/校验由内置切帧引擎处理（人写、久经考验）；80% 协议的「读字段 → 说人话」用声明式
字段表 + 文本模板表达（零代码执行）；只有条件布局等复杂协议才写 JS `parse`（Worker 沙箱）。

权威文件：ABI 形状唯一来源 `src/parser/parser-abi.d.ts`（全局命名空间 `BytetideParser`）；
切帧语义落地 `src/parser/framer.ts`；CRC 实现 `src/parser/crc.ts`；声明式字段 `src/parser/fields.ts`；
字节还原 `src/parser/lineBytes.ts`。改 ABI 先改 `.d.ts`，再同步 `src/types/parser.ts` 与本文档，
三处不得各自漂移。计划背景见 `docs/plan-parser-v1.md`（本文即其交付物之一）。

---

## 脚本结构

脚本是一个 **ESM 模块，唯一默认导出一个对象**（`bytetide.parser v1`）。两层结构：

| 层 | 字段 | 执行方式 | 说明 |
|---|---|---|---|
| 声明式层（推荐） | `meta` / `framing` / `type` / `fields` / `text` | 主线程引擎直接求值，**零代码执行、零沙箱面** | 可 schema 静态校验，AI 生成可靠；Worker 挂掉/CSP 受限时仍可用 |
| 脚本层（兜底） | `parse(frame, ctx)` | Worker 沙箱逐帧执行 | 条件布局等复杂协议才用 |

- **`parse` 存在时引擎走脚本层并忽略 `fields`/`text`**（`framing` 始终由引擎执行，任何脚本
  都不处理粘包/断帧）。
- **两层都没有翻译声明 = 合法的纯切帧器**：只切帧展示原始 hex + 帧长 + CRC 状态，零执行、
  零 Worker（「先看看帧长什么样」的调试前中期形态，切帧器本身即交付价值）。
- 导入时做 schema 静态校验：缺 `meta`、坏 `framing`（非法 hex 串/未知算法/`crc.at` 非
  `tail:N`）、非法取值、非 ESM default export 一律导入即拦。

逐字段说明（对照 `parser-abi.d.ts`）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `meta` | `{ name, version, author?, description? }` | 脚本元信息；`name` 同时是无 `type` 声明时的类型名缺省 |
| `framing.source` | `'binary' \| 'ascii-hex'` | 数据源（见语义钉死清单） |
| `framing.sync` | hex 串，可空 | 同步字（如 `'AA 55'`）；有 sync 时坏帧按滑窗重同步 |
| `framing.length` | 四选一 | `{ kind:'fixed', value }` 定长；`{ kind:'field', at, fmt:'u8'\|'u16'\|'u32', endian?, add }` 长度域；`{ kind:'until', tail }` 分隔符结尾（含 tail 本身）；`{ kind:'line' }` 文本整行一帧 |
| `framing.crc` | `{ algo, at:'tail:N', endian? }`，可空 | 校验失败帧计入警告、不进翻译；算法 7 选 1（见参数表） |
| `framing.maxSize` | number，缺省 4096 | 超大帧保护上限字节数 |
| `type` | `{ at, fmt, endian?, map? }`，可选 | 读 `frame[at]` 的值查 `map` 得类型名；无 `type` → `meta.name`；越界 → `未知`；map 未命中 → `0x..` |
| `fields` | `FieldDecl[]`，可选 | `{ label, at, fmt, endian?, scale?, offset?, unit?, map? }`；value = raw×scale+offset |
| `text` | string，可选 | 文本模板，`{label}` 插值（未命中占位符 → `—`）；缺省自动拼接 `label=value(unit)` |
| `parse` | `(frame, ctx) => ParseResult \| null`，可选 | 脚本层入口，见「脚本层（parse）」 |

最小完整示例（声明式温控协议，可直接保存为 `temp-control.js` 导入）：

```js
// bytetide.parser v1 — 温控协议（声明式示例）
// 帧布局：AA 55 | type(1B) | len(1B) | payload(len-2 B) | CRC16-modbus(2B, 小端)
// len = payload 字节数 + 2（type 与 len 自身）→ 总帧长 = len + 4
export default {
  meta: {
    name: '温控协议',
    version: '2.1',
    author: 'you@example.com',
    description: 'sync AA 55 + 长度域 + CRC16-modbus 的声明式解析',
  },

  framing: {
    source: 'binary',                                  // 设备发原始字节流
    sync: 'AA 55',                                     // 同步字
    length: { kind: 'field', at: 3, fmt: 'u8', add: 4 }, // 长度域 u8@3，总长 = 值 + 4
    crc: { algo: 'crc16-modbus', at: 'tail:2' },       // 尾部 2 字节 CRC（endian 缺省 little）
    maxSize: 4096,
  },

  type: { at: 2, fmt: 'u8', map: { 1: '状态上报', 2: '设置响应', 129: '设置请求' } },

  fields: [
    { label: '温度', at: 4, fmt: 'i16', scale: 0.1, unit: '℃' },
    { label: '模式', at: 6, fmt: 'u8', map: { 0: '待机', 1: '制冷', 2: '制热' } },
    { label: '湿度', at: 7, fmt: 'u8', unit: '%RH' },
    { label: '报警', at: 8, fmt: 'u8', map: { 0: '正常', 1: '超温' } },
  ],

  text: '温度 {温度}℃，模式{模式}，湿度{湿度}%RH，{报警}',
}
```

---

## 语义钉死清单

### 偏移与 frame 边界

- **所有偏移 0 基**；`frame` = 线上**完整帧**（含 sync、长度域、CRC 字节）。
- `fields.at` / `length.at` / `type.at` 均相对 `frame[0]`。注意协议文档常把 payload 首字节
  记为偏移 0，填写时必须加上 sync 与头部长度。
- `length.kind === 'fixed'` 时 `value` = **含 sync 的总帧长**（例：sync 2 字节 + 头 1 + CRC 2
  的 5 字节最短帧，value = 5）。
- `kind:'until'` 帧含分隔符本身（帧边界到 tail 末字节）；`kind:'line'` 帧**不含行尾 CR/LF**
  （容忍 CRLF，`\r` 不进帧），但 `crc.at = 'tail:N'` 在 line/until 帧上同样以**帧字节**倒数：
  line 帧不含行尾，until 帧含分隔符。

### 长度域

- **总帧长 = 长度域原始值 + `add`**；`add` 补偿长度域没覆盖到的 sync 与头部长度。
- 数值对照例（即上文温控协议）：帧 `AA 55 01 07 P0 P1 P2 P3 P4 C0 C1`，长度域 u8@3 = 0x07
  = payload 5 字节 + type/len 自身 2 字节；按 `add = 4`：
  **总帧长 = 7 + 4 = 11 = 2(sync) + 1(type) + 1(len) + 5(payload) + 2(crc16)** ✓
- 坏长度判定（`frameBounds`）：`total < max(at + fmt宽度, CRC宽度 + 1)` 或 `total > maxSize`
  → 坏长度（长度域 fmt 只有 `u8/u16/u32`，无符号）。
- `length.kind === 'field'` 的 `endian` 缺省 `little`。

### CRC 覆盖范围与位置

- **覆盖范围 = frame 去掉 CRC 字节自身**（含 sync 起的其余整帧）。
- `at` **仅支持 `'tail:N'`**（帧尾倒数 N 字节是 CRC 本身）；N = 算法宽度：sum8/xor8 为 1，
  crc16 为 2，crc32 为 4。存盘 CRC 字节按 `crc.endian` 读为无符号整数与计算值比较
  （缺省 little——modbus 惯例低字节在前）。
- **V1 限制**：不支持中位 CRC（CRC 在帧中间）、不支持子范围覆盖（只校验头部某段）、
  不支持多段拼接覆盖。这类协议 V1 请省略 `crc` 声明（切帧照常，帧不标校验状态），
  或在 `parse` 内用 `ctx.bt.crc` 自行校验。
- 校验失败帧：以 `crcOk = false` 吐出、计入警告、**不进翻译**（引擎包装展示原始 hex）；
  帧不进声明式求值，也不进 `parse`。

### CRC 算法参数表

7 种算法按 reveng catalog 参数钉死（杜绝 `crc16-ccitt` 一名多义的生成歧义——它的
init 0x0000 与 0xFFFF 两个变体在本表里是 `crc16-xmodem` 与 `crc16-ccitt-false` 两个名字）。
check 值 = 标准测试向量 `"123456789"`（hex 31..39）的输出，与 `crc.ts` 测试一致：

| algo | poly | init | refin | refout | xorout | check("123456789") | 宽度 |
|---|---|---|---|---|---|---|---|
| `sum8` | —（累加和取低 8 位） | — | — | — | — | `0xDD` | 1 |
| `xor8` | —（逐字节异或） | — | — | — | — | `0x31` | 1 |
| `crc16-modbus` | `0x8005` | `0xFFFF` | true | true | `0x0000` | `0x4B37` | 2 |
| `crc16-ccitt-false` | `0x1021` | `0xFFFF` | false | false | `0x0000` | `0x29B1` | 2 |
| `crc16-xmodem` | `0x1021` | `0x0000` | false | false | `0x0000` | `0x31C3` | 2 |
| `crc16-kermit` | `0x1021` | `0x0000` | true | true | `0x0000` | `0x2189` | 2 |
| `crc32` | `0x04C11DB7`（实现用反射形式 `0xEDB88320`） | `0xFFFFFFFF` | true | true | `0xFFFFFFFF` | `0xCBF43926` | 4 |

framing.crc 与 Worker 内 `bt.crc(algo, bytes)` 共用同一实现（`crc.ts`）。

### fmt / endian / value 变换

- fmt 7 种：`u8 / u16 / u32 / i8 / i16 / i32 / f32`（f32 为 IEEE-754 单精度）。
- **endian 缺省 `little`**；u8/i8 无端序概念。
- 声明式字段：**value = raw × scale + offset**（scale 缺省 1，offset 缺省 0）；展示去除浮点
  噪声（0.1×250 → `25` 而非 `25.000000000000004`）。
- **`map` 命中显示映射值，raw 保留在位置标注**（如 `i16le@4×0.1=0xFA0`）；map 优先于 scale。
- 越界读取安全返回：不抛错，值显示 `—`，位置标注 `越界@at`。
- 位置标注格式（展示用，`parse` 的 `raw` 建议对齐）：`fmt[le|be]@at[×scale][+offset][=0x..]`，
  如 `i16le@4×0.1`、`u8@6`。
- 文本模板：`{label}` 按字段 label 插值，未命中 → `—`；缺省 `text` 自动拼接
  `label=value(unit)`（中文逗号分隔）。

### 重同步策略（framer 状态机）

| 配置 | 情形 | 行为 |
|---|---|---|
| 有 sync | 缓冲内找不到 sync | 丢弃垃圾前缀；整段无 sync 时保留末尾 `syncLen-1` 字节防跨批截断，等待更多字节 |
| 有 sync | 长度域坏长度 / CRC 失败 | **前进一字节滑窗找下一个 sync** 重同步（sync 字节计入垃圾）；CRC 失败帧本身仍以 `crcOk=false` 吐出 |
| 有 sync | until/line 分隔符缺失超 maxSize | 复位缓冲 + 计警告（逐字节滑窗对分隔符协议无意义且 O(n²)） |
| 有 sync | 定长 | 定长无「坏长度」概念（帧边界 = sync + 计数）；字节不足等待，CRC 失败按上行滑窗 |
| 无 sync | 长度域坏长度 | **丢弃整段缓冲复位 + 计警告**（坏长度值下无法可靠重同步：任何位置都可能是帧起点，滑窗只会以 O(n²) 代价产出大量脏帧——牺牲已缓冲数据换取确定性） |
| 无 sync | until/line 分隔符缺失超 maxSize | 复位缓冲 + 计警告 |
| 无 sync | CRC 失败 | **消费整帧继续**（长度域自洽即信任边界：帧边界可信，坏的只是内容） |
| 无 sync | 定长 | 等够 value 字节顺序切 |

状态按 **(sessionId, dir) 隔离**由引擎保证（见下「引擎级语义」）。

### maxSize

缺省 4096（非正数/非有限值回退 4096，向下取整）。超限行为：长度域 `total > maxSize` 判
坏长度；until/line 超 maxSize 仍无分隔符判坏；坏长度/超限按上表复位或滑窗，均计警告。

### 数据源 source

三态字节还原（`lineBytes.ts`，日志行 → 字节）：

- `'binary'`：**优先后端携带的原始字节**（`line.bytes`，可还原 0x80+ 孤立字节），否则
  TextEncoder 编码行文本（有效 UTF-8 场景）。
- `'ascii-hex'`：设备发 `"01 00 ..."` 这样的 hex **文本**——从行文本抽取十六进制字节对
  （忽略其它字符）。

### 引擎级语义

- **试运行三分类**：`no-data`（会话无 RX 数据 → 跳过直接启用）/ `suspect`（有数据但 0 完整
  帧或 CRC 全败 → 停在「已导入未启用」+「framing 疑似配置错误」横幅，不自动启用坏脚本）/
  `ok`（有帧且解析通过 → 自动启用）。
- **四道防线**（AI 生成脚本质量参差的兜底）：①声明式 schema 校验（导入即拦）②试运行
  三分类 ③按批看门狗（reqId 超时 terminate 重建 Worker + 卸载脚本 + 面板报错）④错误率
  熔断（滚动 1000 帧解析错误率 > 30% → 自动停用 + 横幅，用户可重新启用）。
- **离线回溯**：导入/启用脚本时对最近 **2000 行**回溯补解码（decoded FIFO 1000 条/会话的
  2 倍上限——更早的行解码了也会被淘汰）；离线 .log 会话走同一条拉取循环，解码自动覆盖。
- **方向隔离**：切帧状态按 `(sessionId, dir)` 独立——TX 回显与 RX 行在日志表交织，单状态机
  会把两路字节流切碎。
- **全局单脚本**：V1 一份脚本 + 全局启停，多会话共享；会话级多脚本槽留 V3。
- 切帧在**主线程**同步执行（线性扫描毫秒级）；Worker 只执行脚本层 `parse`，且只收完整帧
  （背压丢帧 = 丢结果不脏流）；会话 reset 时引擎代际号 gen+1，旧 Worker 在途结果按 gen 丢弃。

---

## 脚本层（parse）

`parse(frame: Uint8Array, ctx): ParseResult | null` —— 完整帧字节 → 解码结果，Worker 沙箱内
逐帧执行。引擎保证：`frame` 为切帧引擎吐出的完整帧，且配置了 `crc` 时已通过校验。

### ctx

| 字段 | 类型 | 说明 |
|---|---|---|
| `ts` | string | 日志行时间戳（显示用） |
| `epochMillis` | number | 毫秒级时间戳 |
| `dir` | `'rx' \| 'tx'` | 帧方向 |
| `sessionName` | string | 会话名 |
| `bt` | BtLib | 引擎内置标准库（见下表） |

### bt 标准库

| 方法 | 说明 |
|---|---|
| `u8(bytes, at)` / `u16(bytes, at, endian?)` / `u32(bytes, at, endian?)` | 无符号读数 |
| `i8(bytes, at)` / `i16(bytes, at, endian?)` / `i32(bytes, at, endian?)` | 有符号读数 |
| `f32(bytes, at, endian?)` | IEEE-754 单精度读数 |
| `crc(algo, bytes)` | 与 framing 同一算法集（7 种，见参数表），返回数值 |
| `hex(bytes)` | 字节 → hex 串（空格分隔大写，如 `'AA 55'`），展示用 |

所有读数方法 endian 缺省 `little`，与声明式一致。

### ParseResult

```ts
{ type?: string, text: string, fields?: { label, value, unit?, raw? }[], warn?: string }
```

- `text` 必填（自然语言句子）；`type` 缺省回落 `meta.name`（与声明式一致）。
- `fields.value` 为 number 或 string；`raw` 为位置标注（建议 `i16le@4×0.1` 风格，格式不校验）。
- `warn` 非空时该帧带警告展示（帧本身仍是成功解析）。
- **抛错或返回 null → 该帧计入「解析错误」统计，管线不崩**（不确定的帧返回带 `warn` 的
  最小结果即可保留信息；`null` 适合「这不是我的帧」）。

### 沙箱与信任模型

- Worker 环境无 DOM/BOM；引擎引导时覆写 `fetch` / `XMLHttpRequest` / `WebSocket` /
  `EventSource` / `Worker`，脚本**无法发起网络**。module worker 本无 `importScripts`，
  覆写无害。
- **按批看门狗**：每批带递增 reqId，超时无 ack 即 terminate 重建 Worker + 卸载脚本 + 面板
  报错（防 AI 脚本死循环）。
- **错误率熔断**：滚动 1000 帧解析错误率 > 30% → 自动停用 + 横幅。
- 背压：Worker 队列上限 1000 帧/批，超出丢最旧并计警告——丢的是完整帧，**不脏流**。
- **信任模型 =「只加载审过码的脚本」**：V1 不做来源校验，沙箱是纵深防御而非安全边界；
  不要导入来路不明的脚本。

---

## 给 AI 的 prompt 模板

用户拿下面模板 + 协议文档喂给任意 AI，线下生成脚本（V1 的 AI 协作形态）：

```text
你是一名嵌入式串口协议专家。请依据我提供的协议文档，为串口调试工具 ByteTide 编写
一个协议解析脚本（bytetide.parser v1 ABI）。

硬性要求：
1. 输出一个 ESM 模块，唯一默认导出一个对象（export default { ... }）；不要 export 其它
   符号，不要输出任何解释文字——回复必须可直接保存为 .js 文件导入。
2. 优先只用声明式层：meta / framing / type / fields / text（零代码执行，最可靠）。只有当
   不同消息类型的字段布局不同、或字段取值依赖条件逻辑时才写 parse 函数兜底；parse 存在时
   fields/text 会被忽略。两种都不要 = 纯切帧器（仅当协议无结构化字段时）。
3. framing 必须严格依据协议文档填写：
   - source：设备发原始字节流选 'binary'；设备发 "AA 55 ..." 这样的 hex 文本选 'ascii-hex'
   - sync：同步字 hex 串（如 'AA 55'），协议没有同步字就省略
   - length 四选一：
     定长   { kind: 'fixed', value: N }        // N = 含 sync 与 CRC 的总帧长
     长度域 { kind: 'field', at, fmt: 'u8'|'u16'|'u32', endian?, add }
     分隔符 { kind: 'until', tail: '0D 0A' }   // 帧含分隔符本身
     文本行 { kind: 'line' }                   // 整行一帧，不含行尾 CR/LF
   - crc：{ algo, at: 'tail:N' }；algo 只能从这 7 种选（参数按 reveng catalog 钉死）：
     sum8 / xor8 / crc16-modbus / crc16-ccitt-false / crc16-xmodem / crc16-kermit / crc32
     注意 'crc16-ccitt' 一名多义，先与文档核对 init 值再选 ccitt-false(init 0xFFFF) 或
     xmodem(init 0x0000)；N = 算法宽度：sum8/xor8 为 1，crc16 为 2，crc32 为 4
4. 数值语义自检——写完必须逐条心算核对，错了脚本直接不可用：
   - 所有偏移 0 基，且相对【完整帧首字节（含 sync）】。协议文档常把 payload 首字节记为
     偏移 0，必须加上 sync 与头部长度后再填 at
   - 长度域：总帧长 = 长度域原始值 + add。add = 帧里长度域值没覆盖到的字节数
     （典型 = sync 字节数 + CRC 字节数 + 其它头部长度）。请用文档里的真实帧例算一遍总长
   - CRC 覆盖范围 = 除 CRC 字节外的整帧（含 sync）；本 ABI 不支持中位 CRC / 子范围覆盖，
     协议若如此定义请省略 crc 声明
   - endian 缺省 little；字段 value = raw × scale + offset；map 键写数字即可（引擎按
     String(raw) 查找）；协议字段为有负数选 i8/i16/i32，浮点选 f32
5. fields 的 at + 字段宽度不得超出帧长；拿不准的字段宁可不写。声明式字段表对所有消息类型
   统一生效，不同类型帧长差异大时请改用 parse 按类型分别解码。
6. 只允许纯计算：禁止 fetch / XHR / WebSocket / DOM / 动态 import 等任何环境调用。
7. 请先输出一段帧布局表（偏移/长度/字段），自我核对后再输出脚本。

协议文档如下：
<在此粘贴协议文档>
```

---

## 示例脚本两份

两份均与 `parser-abi.d.ts` 完全一致、可直接保存为 `.js` 导入。协议同「脚本结构」示例：
`AA 55 | type(1B) | len(1B) | payload | CRC16-modbus(2B, 小端)`，len = payload + 2，
总帧长 = len + 4（算式见「长度域」）。

### 示例 1：声明式温控协议

```js
// bytetide.parser v1 — 温控协议（声明式）
export default {
  meta: {
    name: '温控协议',
    version: '2.1',
    author: 'you@example.com',
    description: 'sync AA 55 + 长度域 + CRC16-modbus 的声明式解析',
  },

  framing: {
    source: 'binary',
    sync: 'AA 55',
    length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
    crc: { algo: 'crc16-modbus', at: 'tail:2' },
    maxSize: 4096,
  },

  type: { at: 2, fmt: 'u8', map: { 1: '状态上报', 2: '设置响应', 129: '设置请求' } },

  fields: [
    { label: '温度', at: 4, fmt: 'i16', scale: 0.1, unit: '℃' },
    { label: '模式', at: 6, fmt: 'u8', map: { 0: '待机', 1: '制冷', 2: '制热' } },
    { label: '湿度', at: 7, fmt: 'u8', unit: '%RH' },
    { label: '报警', at: 8, fmt: 'u8', map: { 0: '正常', 1: '超温' } },
  ],

  text: '温度 {温度}℃，模式{模式}，湿度{湿度}%RH，{报警}',
}
```

已知限制（V1）：声明式字段表对所有类型统一生效——`设置响应` 帧只有 1 字节 payload 时，
`湿度@7` / `报警@8` 安全显示 `—`（越界标注），不会报错；按类型条件布局见示例 2。

### 示例 2：parse 兜底（同一协议，手写解码）

```js
// bytetide.parser v1 — 温控协议（parse 兜底示例：ctx.bt 读数 + map 翻译 + 按类型布局）
export default {
  meta: {
    name: '温控协议（parse 兜底）',
    version: '2.1',
    description: '演示 parse 层：按消息类型条件解码、bt 标准库、map 翻译',
  },

  framing: {
    source: 'binary',
    sync: 'AA 55',
    length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
    crc: { algo: 'crc16-modbus', at: 'tail:2' },
    maxSize: 4096,
  },

  // parse 存在 → fields/text 被忽略；引擎保证 frame 已通过 CRC 校验
  parse(frame, ctx) {
    const TYPE_NAMES = { 1: '状态上报', 2: '设置响应', 129: '设置请求' }
    const hex2 = (n) => '0x' + n.toString(16).toUpperCase().padStart(2, '0')
    const type = ctx.bt.u8(frame, 2)
    const typeName = TYPE_NAMES[type] ?? hex2(type)

    if (type === 1) {
      // 状态上报：温度 i16le@4 ×0.1、模式 u8@6、湿度 u8@7、报警 u8@8
      const temp = ctx.bt.i16(frame, 4) * 0.1
      const modeRaw = ctx.bt.u8(frame, 6)
      const humidity = ctx.bt.u8(frame, 7)
      const alarmRaw = ctx.bt.u8(frame, 8)
      const MODES = { 0: '待机', 1: '制冷', 2: '制热' }
      const mode = MODES[modeRaw] ?? hex2(modeRaw)
      const alarm = alarmRaw === 0 ? '正常' : '超温'
      return {
        type: typeName,
        text: `温度 ${temp}℃，模式${mode}，湿度${humidity}%RH，${alarm}`,
        fields: [
          { label: '温度', value: temp, unit: '℃', raw: 'i16le@4×0.1' },
          { label: '模式', value: mode, raw: `u8@6=${hex2(modeRaw)}` },
          { label: '湿度', value: humidity, unit: '%RH', raw: 'u8@7' },
          { label: '报警', value: alarm, raw: `u8@8=${hex2(alarmRaw)}` },
        ],
      }
    }

    if (type === 2) {
      // 设置响应：结果 u8@4（0 = 成功），失败时附错误码 u8@5
      const code = ctx.bt.u8(frame, 4)
      if (code === 0) {
        return {
          type: typeName,
          text: '设置成功',
          fields: [{ label: '结果', value: '成功', raw: 'u8@4=0x00' }],
        }
      }
      return {
        type: typeName,
        text: `设置失败（错误码 ${ctx.bt.u8(frame, 5)}）`,
        fields: [{ label: '结果', value: '失败', raw: `u8@4=${hex2(code)}` }],
        warn: '设备返回非零错误码',
      }
    }

    // 未定义类型：给最小结果保留现场（返回 null 则计入解析错误统计，二选一）
    return {
      type: typeName,
      text: `${typeName}，${frame.length} 字节：${ctx.bt.hex(frame)}`,
      warn: '协议文档未定义的类型码',
    }
  },
}
```

---

## 版本与兼容

- **v1 ABI 冻结**：`src/parser/parser-abi.d.ts` 是形状唯一权威；改 ABI 先改它，再同步
  `src/types/parser.ts` 与本文档，三处不得各自漂移。
- V2 展望：应用内 AI 生成解析器（REST 桥调 LLM + 真机数据试运行闭环）。
- V3 展望：Rust 侧运行时（rquickjs 跑 JS 层，声明式脚本直接 serde + 字段抽取）、encode
  构包（声明式字段表反向填值）、多脚本槽（会话级）、告警规则引用解码字段。
- V1 明确不做：AI 应用内生成、TX 按字段构包、Rust 侧解码（零后端改动）。

## 实现备忘

- **CSP 红线**：脚本层依赖 blob-URL module Worker + 动态 `import()`，成立前提是
  `tauri.conf.json` 的 `csp` 为 `null`（现状）。**将来若引入 CSP，必须包含
  `script-src blob:` 与 `worker-src blob:`**，否则脚本层会静默挂掉（声明式层不依赖
  Worker，不受影响）。
- **脚本持久化**：`localStorage['serialtool.parserScript']`（源码 + meta + 启用态），启动
  静默恢复。
