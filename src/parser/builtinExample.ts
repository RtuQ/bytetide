/**
 * 内置示例解析脚本（声明式，零代码执行）——ParserPanel「加载内置示例体验」用。
 * 源码必须是可直接 import() 的合法 ESM（脚本内容由引擎 schema 校验后装载）。
 */
export const BUILTIN_EXAMPLE_SRC: string = `// bytetide.parser v1 · 内置示例：温控协议 v2.1（声明式，无 parse 脚本层）
//
// 帧布局（与 framing 自洽：总帧长 = 长度域@3 的值 + add 4）：
//   [0..1] 同步字 AA 55
//   [2]    消息类型 u8
//   [3]    长度域 u8（值 = 载荷字节数 + 2 字节 CRC）
//   [4..]  载荷（按类型不同，短帧可为空）
//   [尾2]  CRC16/MODBUS（小端 lo hi，覆盖除 CRC 自身外的全部字节）
//
// 各类型帧长对照（示例帧 CRC 为真实值，可直接喂入验证）：
//   0x01 温度上报 9B：AA 55 01 05 [温度 i16be@4] [电量 u8@6] [CRC lo hi]
//       AA 55 01 05 00 FA 57 23 5D → 温度 25.0℃ · 电量 87%
//   0x02 状态上报 9B：AA 55 02 05 [模式 u8@4] [风机 u8@5] [故障码 u8@6] [CRC lo hi]
//       AA 55 02 05 02 02 00 C4 A3 → 模式 制冷 · 风机 2 档
//   0x81 下发查询 6B：AA 55 81 02 [CRC lo hi]（无载荷）
//       AA 55 81 02 D0 7D
//   0x83 告警     7B：AA 55 83 03 [告警码 u8@4] [CRC lo hi]
//       AA 55 83 03 03 9C CD → 告警码 3（传感器开路）
//
// 短帧行为（预期，非缺陷）：声明式字段表是扁平的，同一组 fields 套用到所有帧型。
// 0x81/0x83 这类短帧不带温度/电量载荷——偏移越界的字段安全返回 '—'
// （raw 标注「越界@N」）；恰好落在帧尾校验字节上的字段会显示无意义数值。
// 需要按帧型挑选字段布局的协议请改用 parse 脚本层（Worker 沙箱），
// 见 docs/parser-spec.md。

export default {
  meta: {
    name: '温控协议',
    version: '2.1',
    author: 'ByteTide 内置示例',
    description: 'AA 55 同步 + u8 长度域 + CRC16/MODBUS；温度/状态/查询/告警四类帧',
  },

  framing: {
    source: 'binary',
    sync: 'AA 55',
    length: { kind: 'field', at: 3, fmt: 'u8', add: 4 },
    crc: { algo: 'crc16-modbus', at: 'tail:2' },
    maxSize: 4096,
  },

  type: {
    at: 2,
    fmt: 'u8',
    map: { 1: '温度上报', 2: '状态上报', 129: '下发查询', 131: '告警' },
  },

  fields: [
    { label: '温度', at: 4, fmt: 'i16', endian: 'big', scale: 0.1, unit: '℃' },
    { label: '电量', at: 6, fmt: 'u8', unit: '%' },
    { label: '模式', at: 4, fmt: 'u8', map: { 0: '待机', 1: '制热', 2: '制冷' } },
    {
      label: '告警码',
      at: 4,
      fmt: 'u8',
      map: { 1: '通讯超时', 2: '传感器故障', 3: '传感器开路', 4: '温度超限' },
    },
  ],

  text: '温度 {温度}℃，电量{电量}%',
}
`
