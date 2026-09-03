/**
 * Worker 宿主（脚本层 parse 的执行沙箱，parser V1 · docs/plan-parser-v1.md §2）。
 *
 * - Blob URL + `{ type: 'module' }`（module worker 才能动态 import blob 模块）；
 *   Worker/Blob/URL 任一缺失（node / 受限环境）返回 null，声明式脚本不受影响。
 * - Worker 源码是自包含模板字符串：沙箱覆写（无网络/无嵌套 Worker）+ bt 标准库 +
 *   消息循环。CRC 算法用 Function.toString() 从 crc.ts 内嵌（唯一实现，零手抄）。
 * - 消息协议与 EngineHost 一一对应：
 *   in  {type:'load', reqId, src}    → out {type:'loaded', reqId, decl} | {type:'loadError', reqId, error}
 *   in  {type:'batch', reqId, items} → out {type:'ack', reqId} → {type:'results', reqId, items}
 * - decl 只挑数据字段构造纯对象：函数不可 structured-clone，绝不能把含 parse 的
 *   原对象 postMessage 出去。
 */
import type { EngineBatchResult, EngineHost } from './engine'
import { crcSum8, crcXor8, crc16Generic, crc32, CRC16_PARAMS } from './crc'

interface Pending {
  resolve: (v: never) => void
  reject: (e: Error) => void
}

interface WorkerMsg {
  type: 'loaded' | 'loadError' | 'ack' | 'results'
  reqId: number
  decl?: unknown
  error?: string
  items?: EngineBatchResult[]
}

/** 组装 Worker 源码（自包含、不 import 任何东西） */
function buildWorkerSource(): string {
  // crc.ts 的四个实现是自包含纯函数，专为 toString() 内嵌设计（改函数体不得引入外部依赖）
  const crcFns = [crcSum8, crcXor8, crc16Generic, crc32].map((f) => f.toString()).join('\n')
  return `"use strict";
// === bytetide parser 脚本沙箱 Worker（由 bootstrap.ts 生成，勿手改） ===
// 沙箱覆写：脚本层无网络、无嵌套 Worker（module worker 本无 importScripts）
self.fetch = function () { throw new Error('脚本层禁止使用 fetch') }
self.XMLHttpRequest = function () { throw new Error('脚本层禁止使用 XMLHttpRequest') }
self.WebSocket = function () { throw new Error('脚本层禁止使用 WebSocket') }
self.EventSource = function () { throw new Error('脚本层禁止使用 EventSource') }
self.Worker = function () { throw new Error('脚本层禁止嵌套 Worker') }

// CRC 算法集：主线程 crc.ts 唯一实现，经 toString 内嵌（零手抄）
${crcFns}
var CRC16_PARAMS = ${JSON.stringify(CRC16_PARAMS)}
function btCrc(algo, bytes) {
  switch (algo) {
    case 'sum8': return crcSum8(bytes)
    case 'xor8': return crcXor8(bytes)
    case 'crc16-modbus':
    case 'crc16-ccitt-false':
    case 'crc16-xmodem':
    case 'crc16-kermit': {
      var p = CRC16_PARAMS[algo]
      return crc16Generic(bytes, p[0], p[1], p[2], p[3], p[4])
    }
    case 'crc32': return crc32(bytes)
    default: throw new Error('未知 CRC 算法：' + algo)
  }
}

// bt 标准库：读数 u8..f32（乘法拼装 + DataView f32，端序缺省 little）、hex、crc
function btRd(b, at, n, endian) {
  var v = 0
  if (endian === 'big') { for (var k = 0; k < n; k++) v = v * 256 + b[at + k] }
  else { for (var k2 = 0; k2 < n; k2++) v = v * 256 + b[at + n - 1 - k2] }
  return v
}
var bt = {
  u8: function (b, at) { return b[at] },
  u16: function (b, at, e) { return btRd(b, at, 2, e) },
  u32: function (b, at, e) { return btRd(b, at, 4, e) },
  i8: function (b, at) { var v = b[at]; return v >= 128 ? v - 256 : v },
  i16: function (b, at, e) { var v = btRd(b, at, 2, e); return v >= 32768 ? v - 65536 : v },
  i32: function (b, at, e) { var v = btRd(b, at, 4, e); return v >= 2147483648 ? v - 4294967296 : v },
  f32: function (b, at, e) { return new DataView(b.buffer, b.byteOffset + at, 4).getFloat32(0, e !== 'big') },
  hex: function (b) {
    var s = ''
    for (var i = 0; i < b.length; i++) s += (i > 0 ? ' ' : '') + b[i].toString(16).toUpperCase().padStart(2, '0')
    return s
  },
  crc: btCrc,
}

var userParse = null
self.onmessage = async function (ev) {
  var msg = ev.data
  if (msg.type === 'load') {
    try {
      var url = URL.createObjectURL(new Blob([msg.src], { type: 'text/javascript' }))
      try {
        var mod = await import(url)
        var d = mod && mod.default ? mod.default : {}
        userParse = typeof d.parse === 'function' ? d.parse : null
        // 只挑数据字段构造纯对象 decl（函数不可 structured-clone，含 parse 的原对象绝不出 worker）
        var decl = { meta: d.meta, framing: d.framing, type: d.type, fields: d.fields, text: d.text }
        self.postMessage({ type: 'loaded', reqId: msg.reqId, decl: decl })
      } finally {
        URL.revokeObjectURL(url)
      }
    } catch (e) {
      userParse = null
      self.postMessage({ type: 'loadError', reqId: msg.reqId, error: String(e) })
    }
  } else if (msg.type === 'batch') {
    // 先 ack（主线程看门狗据此起 results 定时器），再逐项执行
    self.postMessage({ type: 'ack', reqId: msg.reqId })
    var out = []
    for (var i = 0; i < msg.items.length; i++) {
      var item = msg.items[i]
      if (typeof userParse !== 'function') { out.push({ no: item.no, ok: false }); continue }
      try {
        var r = userParse(new Uint8Array(item.bytes), {
          ts: item.ts, epochMillis: item.epochMillis, dir: item.dir, sessionName: item.sessionName, bt: bt,
        })
        out.push(r == null ? { no: item.no, ok: false } : { no: item.no, ok: true, result: r })
      } catch (e2) {
        out.push({ no: item.no, ok: false })
      }
    }
    self.postMessage({ type: 'results', reqId: msg.reqId, items: out })
  }
}
`
}

/**
 * 创建脚本层宿主。onAck：批次 ack 通知（引擎看门狗用——ack 到了才起 results 定时器）。
 * 返回 null = 环境不支持（无 Worker/Blob/URL）。
 */
export function createScriptHost(onAck?: (reqId: number) => void): EngineHost | null {
  if (typeof Worker === 'undefined' || typeof Blob === 'undefined' || typeof URL === 'undefined') return null
  const url = URL.createObjectURL(new Blob([buildWorkerSource()], { type: 'text/javascript' }))
  let worker: Worker
  try {
    worker = new Worker(url, { type: 'module' })
  } catch {
    URL.revokeObjectURL(url)
    return null
  }
  const pending = new Map<number, Pending>()
  let seq = 0
  const failAll = (msg: string) => {
    for (const p of pending.values()) p.reject(new Error(msg) as never)
    pending.clear()
  }
  worker.onmessage = (ev: MessageEvent) => {
    const msg = ev.data as WorkerMsg
    const p = pending.get(msg.reqId)
    switch (msg.type) {
      case 'loaded':
        pending.delete(msg.reqId)
        p?.resolve({ decl: msg.decl } as never)
        break
      case 'loadError':
        pending.delete(msg.reqId)
        p?.reject(new Error(msg.error ?? '脚本加载失败') as never)
        break
      case 'ack':
        onAck?.(msg.reqId)
        break
      case 'results':
        pending.delete(msg.reqId)
        p?.resolve({ reqId: msg.reqId, items: msg.items ?? [] } as never)
        break
    }
  }
  // worker 级异常（源码崩了）：所有挂起请求即刻失败，别让引擎看门狗空等
  worker.onerror = () => failAll('脚本 Worker 崩溃')
  return {
    load(src: string) {
      const reqId = ++seq
      return new Promise((resolve, reject) => {
        pending.set(reqId, { resolve, reject })
        worker.postMessage({ type: 'load', reqId, src })
      })
    },
    run(batch) {
      // reqId 由引擎分配并原样回传（看门狗/gen 校验都按它对账）
      return new Promise((resolve, reject) => {
        pending.set(batch.reqId, { resolve, reject })
        worker.postMessage({ type: 'batch', reqId: batch.reqId, items: batch.items })
      })
    },
    dispose() {
      failAll('脚本 Worker 已销毁')
      worker.terminate()
      URL.revokeObjectURL(url)
    },
  }
}
