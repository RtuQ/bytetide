// bytetide.parser v1 — 电源监控协议（演示，配合 scripts/demo-feed.mjs :50102 使用）
// 帧布局：AA 55 | 温度 i16(大端,×0.1℃) | 电压 u16(大端,×0.01V) | 电流 u16(大端,×0.001A) | sum8
// sum8 仅覆盖 6 字节数据段（不含帧头与校验自身），与「数据绘图」的累加和校验同口径；
// 本脚本靠 sync + 定长 9B 切帧、不做校验，帧校验交由绘图侧演示。
export default {
  meta: {
    name: '电源监控',
    version: '1.0',
    author: 'ByteTide demo',
    description: 'sync AA 55 + 定长 9B：温度/电压/电流三通道遥测（截图演示流 tcp://127.0.0.1:50102）',
  },

  framing: {
    source: 'binary', // 设备发原始字节流
    sync: 'AA 55', // 同步字
    length: { kind: 'fixed', value: 9 }, // 总帧长 9B（含帧头与校验字节）
    maxSize: 4096,
  },

  fields: [
    { label: '温度', at: 2, fmt: 'i16', endian: 'big', scale: 0.1, unit: '℃' },
    { label: '电压', at: 4, fmt: 'u16', endian: 'big', scale: 0.01, unit: 'V' },
    { label: '电流', at: 6, fmt: 'u16', endian: 'big', scale: 0.001, unit: 'A' },
  ],

  text: '温度 {温度}℃，电压 {电压}V，电流 {电流}A',
}
