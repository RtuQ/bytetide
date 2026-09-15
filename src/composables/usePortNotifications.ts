import type { PortInfo } from '../types'

/**
 * 串口热插拔通知的纯逻辑（通知重设计 v2）：后端 hotplug.rs 每秒轮询 list_ports
 * 做名字 diff 后 emit `port-changed`（完整端口列表、不区分插入/拔出方向），
 * 方向由前端对前后两次列表求差得到。首帧只建基线不弹通知——应用启动时
 * 已插着的端口不应刷一屏「已接入」。
 */

/** 前后两次端口列表的双向 diff（按 name 匹配；同名端口元数据变化不算插拔） */
export function diffPorts(prev: PortInfo[], next: PortInfo[]): { arrived: PortInfo[]; removed: PortInfo[] } {
  const prevNames = new Set(prev.map((p) => p.name))
  const nextNames = new Set(next.map((p) => p.name))
  return {
    arrived: next.filter((p) => !prevNames.has(p.name)),
    removed: prev.filter((p) => !nextNames.has(p.name)),
  }
}

/** 通知副文：产品名优先，其次厂商，再次序列号；全缺则退到传输类型（如 'serial'/'usb'） */
export function describePort(p: PortInfo): string {
  const parts = [p.product, p.vendor].filter((s): s is string => typeof s === 'string' && s.trim() !== '')
  if (parts.length === 0 && typeof p.serial === 'string' && p.serial.trim() !== '') parts.push(`SN ${p.serial}`)
  return parts.join(' · ') || p.portType
}

// ---- 消费端基线（模块级单例）：首帧建基线，之后每次返回与上次的 diff ----
let last: PortInfo[] | null = null

export function consumePortDiff(ports: PortInfo[]): { arrived: PortInfo[]; removed: PortInfo[] } | null {
  const prev = last
  last = ports
  if (!prev) return null
  return diffPorts(prev, ports)
}

/** 测试辅助：清空基线 */
export function _resetPortWatchForTest() {
  last = null
}
