#!/usr/bin/env node
/**
 * ByteTide 截图演示数据源：本地 TCP 双流，配合 README/docs 截图（docs/demo-data.md）。
 *
 *   :50101  ASCII 设备控制台——多模块多级别遥测日志（ANSI 着色，含 WARN/ERROR 波动），
 *           适合日志视图 / 关键词高亮 / 查找 / 自动回复 / 告警的截图。
 *   :50102  电源监控二进制帧（2Hz）——AA 55 | 温度 i16BE | 电压 u16BE | 电流 u16BE | sum8(数据段)，
 *           与「绘图」（累加和校验）和「协议解析」（sample-data/parser-power.js）同一条流兼容。
 *
 * 用法：node scripts/demo-feed.mjs   （Ctrl+C 退出；无第三方依赖）
 */
import net from 'node:net'

const PORT_ASCII = 50101
const PORT_BIN = 50102

/** 客户端集合广播 */
function fanout(clients, chunk) {
  for (const sock of clients) {
    if (!sock.destroyed) sock.write(chunk)
  }
}

function onServer(sockets, name, backfill) {
  return (sock) => {
    sockets.add(sock)
    console.log(`[demo-feed] ${name} 客户端接入 ${sock.remoteAddress}:${sock.remotePort}`)
    backfill(sock)
    sock.on('close', () => sockets.delete(sock))
    sock.on('error', () => sockets.delete(sock))
  }
}

// ============ 流一：ASCII 设备控制台（:50101） ============

const SGR = { DEBUG: '36', INFO: '32', WARN: '33', ERROR: '31' } // 级别 token 颜色

function stamp(d = new Date()) {
  const p = (n, w = 2) => String(n).padStart(w, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`
}

/** [时:分:秒.毫秒] [级别] [模块] 内容（级别 token 着 ANSI 色，正文保持默认） */
function consoleLine(level, mod, msg, at = new Date(), ansi = true) {
  const sgr = SGR[level]
  const tok = `[${level.padEnd(5)}]`
  const rest = ` [${mod}] ${msg}`
  const body = ansi && sgr ? `[${stamp(at)}] \x1b[${sgr}m${tok}\x1b[0m${rest}` : `[${stamp(at)}] ${tok}${rest}`
  return body + '\n'
}

const asciiClients = new Set()

let seq = 1024
let soc = 87
let errorStorm = 0 // >0 表示传感器 ERROR 风暴期间（下个 tick 恢复）

/** 与时间相关的慢漂移传感器读数（截图里数值要有起伏） */
function sensor(tMs) {
  const t = tMs / 1000
  return {
    temp: 26 + 6 * Math.sin(t * 0.11) + (Math.random() - 0.5) * 0.4,
    hum: 58 + 9 * Math.sin(t * 0.07 + 2) + (Math.random() - 0.5) * 1.2,
    rssi: Math.round(-64 + 6 * Math.sin(t * 0.05 + 1) + (Math.random() - 0.5) * 4),
  }
}

/** 逐 tick 轮转的日志剧本：传感器为主，心跳/电源/网络穿插，WARN/ERROR 低频出没 */
function nextAsciiLine(tick, tMs) {
  const at = new Date(tMs)
  const s = sensor(tMs)
  const m = tick % 12
  if (errorStorm > 0) {
    errorStorm--
    if (errorStorm === 0) return consoleLine('INFO', 'system', '传感器复位完成 恢复采样', at)
    return consoleLine('ERROR', 'sensor', `读取超时 dev=mlx90614 已重试 3 次，跳过本周期`, at)
  }
  if (m === 11 && Math.random() < 0.55) {
    errorStorm = 2
    return consoleLine('ERROR', 'sensor', `读取超时 dev=mlx90614 已重试 3 次，跳过本周期`, at)
  }
  if (m === 9 && Math.random() < 0.7) {
    return consoleLine('WARN', 'comm', `信号质量下降 rssi=${s.rssi - 14}dBm 已重试=${1 + (tick % 3)}`, at)
  }
  if (m % 4 === 1) {
    seq += 1 + (tick % 3)
    return consoleLine('DEBUG', 'comm', `心跳 seq=${seq} rssi=${s.rssi}dBm 缓冲=low`, at)
  }
  if (m === 6) {
    soc = soc > 12 ? soc - 1 : 87
    return consoleLine('INFO', 'power', `电池 ${soc}% ${(3.9 + Math.random() * 0.08).toFixed(2)}V ${soc > 80 ? '浮充' : '放电中'}`, at)
  }
  if (m === 3 && tick % 24 === 3) {
    return consoleLine('INFO', 'net', 'MQTT 已连接 broker=emqx.local:1883 keepalive=30s', at)
  }
  return consoleLine('INFO', 'sensor', `温度=${s.temp.toFixed(1)}℃ 湿度=${s.hum.toFixed(1)}%RH 气压=${(1013.1 + Math.random()).toFixed(1)}hPa`, at)
}

const asciiServer = net.createServer(
  onServer(asciiClients, 'ASCII:50101', (sock) => {
    // 接入即回填 8 行历史，首屏不空
    for (let i = 8; i > 0; i--) sock.write(nextAsciiLine(i * 7, Date.now() - i * 700))
  }),
)
asciiServer.listen(PORT_ASCII, '127.0.0.1', () => console.log(`[demo-feed] ASCII 控制台流 tcp://127.0.0.1:${PORT_ASCII}`))

let asciiTick = 0
setInterval(() => {
  asciiTick++
  fanout(asciiClients, nextAsciiLine(asciiTick, Date.now()))
}, 900)

// ============ 流二：电源监控二进制帧（:50102） ============

const binClients = new Set()

/**
 * 与 sample-data/parser-power.js、绘图配置三方对齐的帧布局：
 * AA 55 | 温度 i16BE ×0.1℃ | 电压 u16BE ×0.01V | 电流 u16BE ×0.001A | sum8（仅 6B 数据段，不含帧头/自身）
 * 帧后附 \n：后端按行切分，一行一帧（绘图/HEX 视图/解析三方输入口径一致）。
 */
function powerFrame(tMs) {
  const t = tMs / 1000
  const temp = 26 + 6 * Math.sin(t * 0.31) + (Math.random() - 0.5) * 0.3
  const volt = 3.70 + 0.22 * Math.sin(t * 0.83 + 1.2) + (Math.random() - 0.5) * 0.02
  const curr = 0.45 + 0.38 * Math.sin(t * 1.9 + 0.5) + (Math.random() - 0.5) * 0.03
  const buf = Buffer.alloc(9)
  buf[0] = 0xaa
  buf[1] = 0x55
  buf.writeInt16BE(Math.round(temp * 10), 2)
  buf.writeUInt16BE(Math.max(0, Math.round(volt * 100)), 4)
  buf.writeUInt16BE(Math.max(0, Math.round(curr * 1000)), 6)
  let sum = 0
  for (let i = 2; i < 8; i++) sum = (sum + buf[i]) & 0xff
  buf[8] = sum
  return buf
}

const binServer = net.createServer(
  onServer(binClients, 'BIN:50102', (sock) => {
    // 接入即回填 60 帧（≈30s 历史），波形首屏即有形
    for (let i = 60; i > 0; i--) {
      sock.write(powerFrame(Date.now() - i * 500))
      sock.write('\n')
    }
  }),
)
binServer.listen(PORT_BIN, '127.0.0.1', () => console.log(`[demo-feed] 电源监控帧流 tcp://127.0.0.1:${PORT_BIN}`))

setInterval(() => {
  const frame = powerFrame(Date.now())
  fanout(binClients, Buffer.concat([frame, Buffer.from('\n')]))
}, 500)

process.on('SIGINT', () => process.exit(0))
