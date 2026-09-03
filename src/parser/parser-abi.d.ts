/**
 * bytetide.parser v1 — 脚本作者 / AI 参考类型定义（全局环境声明，无需 import）。
 *
 * 权威语义文档：docs/parser-spec.md（偏移/长度域/CRC/重同步语义逐条钉死 + AI prompt 模板）。
 * 引擎运行时数据类型在 src/types/parser.ts；本文件是脚本 ABI 的唯一形状来源，
 * 两处不得各自漂移：改 ABI 先改这里，再同步 types/parser.ts 与 spec。
 *
 * 两层结构：声明式层（fields/type/text，零代码执行）+ 脚本层（parse，Worker 沙箱执行）。
 * parse 存在时引擎走脚本层并忽略 fields/text；两层都没有 = 合法的纯切帧器。
 */
declare namespace BytetideParser {
  /** 数据源：与 PlotSource（src/types/index.ts）同字面量 */
  type Source = 'binary' | 'ascii-hex'

  /** 端序，缺省 little */
  type Endian = 'big' | 'little'

  /** 字段读数格式；f32 为 IEEE-754 单精度 */
  type Fmt = 'u8' | 'u16' | 'u32' | 'i8' | 'i16' | 'i32' | 'f32'

  /**
   * 校验算法（reveng catalog 参数钉死，多项式/初值/refin/refout/xorout 见 spec 参数表）：
   *   sum8 = 累加和低 8 位          xor8 = 逐字节异或
   *   crc16-modbus       poly 0x8005 init 0xFFFF refin/refout=true
   *   crc16-ccitt-false  poly 0x1021 init 0xFFFF refin/refout=false
   *   crc16-xmodem       poly 0x1021 init 0x0000 refin/refout=false
   *   crc16-kermit       poly 0x1021 init 0x0000 refin/refout=true
   *   crc32              IEEE（zlib 同款）init/xorout 0xFFFFFFFF
   */
  type CrcAlgo =
    | 'sum8'
    | 'xor8'
    | 'crc16-modbus'
    | 'crc16-ccitt-false'
    | 'crc16-xmodem'
    | 'crc16-kermit'
    | 'crc32'

  /** 定长帧：value = 线上总帧长（含 sync 与 CRC 字节） */
  interface LengthFixed {
    kind: 'fixed'
    value: number
  }
  /** 长度域帧：总帧长 = 长度域原始值 + add（add 补偿 sync 与头部长度） */
  interface LengthField {
    kind: 'field'
    at: number
    fmt: 'u8' | 'u16' | 'u32'
    endian?: Endian
    add: number
  }
  /** 分隔符结尾：帧读到 tail（hex 串，如 '0D 0A'）为止（含 tail 本身） */
  interface LengthUntil {
    kind: 'until'
    tail: string
  }
  /** 文本协议整行一帧（以 \n 结尾，容忍前导 \r） */
  interface LengthLine {
    kind: 'line'
  }

  type Length = LengthFixed | LengthField | LengthUntil | LengthLine

  /** CRC 声明：at 仅支持 'tail:N'（帧尾倒数 N 字节为 CRC 本身）；覆盖范围 = frame 去掉 CRC 字节 */
  interface Crc {
    algo: CrcAlgo
    at: string
    endian?: Endian
  }

  interface Framing {
    source: Source
    /** 同步字 hex 串（如 'AA 55'），可空；有 sync 时坏帧按滑窗重同步 */
    sync?: string
    length: Length
    crc?: Crc
    /** 超大帧保护上限字节数（缺省 4096） */
    maxSize?: number
  }

  /** 消息类型声明：读 frame[at] 的 fmt 值查 map 得类型名；缺省整个 type 时用 meta.name */
  interface TypeDecl {
    at: number
    fmt: Fmt
    endian?: Endian
    map?: Record<string, string>
  }

  /** 声明式字段：value = raw × scale(默认1) + offset(默认0)；map 命中显示映射值（raw 保留） */
  interface FieldDecl {
    label: string
    at: number
    fmt: Fmt
    endian?: Endian
    scale?: number
    offset?: number
    unit?: string
    map?: Record<string, string>
  }

  /** parse 的第二参数（Worker 内执行）；bt 为引擎内置标准库 */
  interface ParseCtx {
    ts: string
    epochMillis: number
    dir: 'rx' | 'tx'
    sessionName: string
    bt: BtLib
  }

  /** 引擎内置读数/校验工具（与 framing 同一算法集） */
  interface BtLib {
    u8(bytes: Uint8Array, at: number): number
    u16(bytes: Uint8Array, at: number, endian?: Endian): number
    u32(bytes: Uint8Array, at: number, endian?: Endian): number
    i8(bytes: Uint8Array, at: number): number
    i16(bytes: Uint8Array, at: number, endian?: Endian): number
    i32(bytes: Uint8Array, at: number, endian?: Endian): number
    f32(bytes: Uint8Array, at: number, endian?: Endian): number
    crc(algo: CrcAlgo, bytes: Uint8Array): number
    hex(bytes: Uint8Array): string
  }

  interface DecodedFieldOut {
    label: string
    value: number | string
    unit?: string
    /** 位置标注（如 'i16be@4×0.1'），展示用 */
    raw?: string
  }

  /** parse 返回；null 或抛错 = 该帧计入解析错误统计，管线不崩 */
  interface ParseResult {
    type?: string
    text: string
    fields?: DecodedFieldOut[]
    warn?: string
  }

  interface Meta {
    name: string
    version: string
    author?: string
    description?: string
  }

  interface Script {
    meta: Meta
    framing: Framing
    type?: TypeDecl
    fields?: FieldDecl[]
    /** 文本模板：{label} 插值；缺省自动拼接 label=value(unit) */
    text?: string
    parse?(frame: Uint8Array, ctx: ParseCtx): ParseResult | null
  }
}
