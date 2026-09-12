// 1GB 离线日志验收探针（Stage 2 Task 8 Step 5，不进 CI）。
//
// 定位：本地手动运行的 **1GB 文件生成器 + 手测指引**——
// 生成临时合成 TSV（约 500 万数据行，写完即测、测完即删），打印字节数/行数，
// 引导用 `npm run tauri dev` 手测流式打开。
//
// 为什么不在 Node 里断言 RSS：process.memoryUsage().rss 只反映本 Node 进程，
// 量不到 Tauri/WebView 侧内存，对「open 后 RSS 增长 <500MB」的验收没有意义。
// 真正的内存/分页断言由 Rust 集成测试的 2M 行等价物覆盖（crates 侧
// offline 集成测试：索引 O(文件) 顺序读、内存只留稀疏偏移+单页）。
//
// 用法：
//   npm run probe:offline            # 生成 1GiB（默认）
//   npm run probe:offline -- 512     # 自定义大小（MiB）
//
// 手测步骤（脚本会再打印一遍）：
//   1. npm run tauri dev
//   2. 设置弹层/标签栏「打开日志」选择本脚本生成的临时文件
//   3. 验收点：打开近即时（索引为 O(文件) 一次顺序读，无进度条属预期）；
//      视口停在文件尾部（与旧全量链路等价）；上滑可翻页回补更早的行；
//      系统监视器观察 bytetide 进程 RSS 增长 < 500MB（无全文过 IPC/WebView）。
//   4. 测完回车，脚本删除临时文件（Ctrl+C 也会清理）。
import { closeSync, fstatSync, openSync, unlinkSync, writeSync } from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { createInterface } from 'node:readline/promises'

const MIB = 1024 * 1024
const targetBytes = (() => {
  const arg = Number(process.argv[2])
  if (Number.isFinite(arg) && arg > 0) return Math.round(arg * MIB)
  return 1024 * MIB // 默认 1GiB
})()

// 固定 200 字节/行：'HH:MM:SS.mmm\tRX\t' = 16B + text 183B + '\n'。
// 1GiB ≈ 5,368,709 行（约 500 万），与 plan 的量级一致。
const LINE_BYTES = 200
const TEXT_BYTES = LINE_BYTES - 16 - 1
const CHUNK_LINES = 4096 // ≈ 0.8MB/次写

const pad = (n, w) => String(n).padStart(w, '0')

/** 第 n 个数据行（n 从 0 计）的完整 TSV 文本（ASCII，字节长=字符长） */
function makeLine(n) {
  // 时间随行推进循环一天，供 Δt/图表类视图有真实形状
  const t = (n * 7) % 86_400_000
  const h = Math.floor(t / 3_600_000)
  const m = Math.floor((t % 3_600_000) / 60_000)
  const s = Math.floor((t % 60_000) / 1000)
  const ms = t % 1000
  const dir = n % 10 === 0 ? 'TX' : 'RX'
  const text = `probe-line-${pad(n, 8)} `.padEnd(TEXT_BYTES, '.')
  return `${pad(h, 2)}:${pad(m, 2)}:${pad(s, 2)}.${pad(ms, 3)}\t${dir}\t${text}\n`
}

const tmpPath = path.join(os.tmpdir(), `bytetide-probe-offline-${process.pid}-${Date.now()}.log`)
let fd = null

function cleanup() {
  if (fd !== null) {
    try {
      closeSync(fd)
    } catch {
      /* 已关闭 */
    }
    fd = null
  }
  try {
    unlinkSync(tmpPath)
  } catch {
    /* 已删/未创建 */
  }
}

for (const sig of ['SIGINT', 'SIGTERM']) {
  process.on(sig, () => {
    cleanup()
    process.exit(130)
  })
}

try {
  fd = openSync(tmpPath, 'w')
  writeSync(fd, `# bytetide offline probe · generated ${new Date().toISOString()}\n`)
  let written = LINE_BYTES + 1 // 含头注释行
  let lines = 0
  let chunk = []
  let chunkBytes = 0
  let nextReport = 256 * MIB
  while (written + LINE_BYTES <= targetBytes) {
    chunk.push(makeLine(lines))
    chunkBytes += LINE_BYTES
    written += LINE_BYTES
    lines += 1
    if (chunk.length >= CHUNK_LINES) {
      writeSync(fd, Buffer.from(chunk.join(''), 'utf8'))
      chunk = []
      chunkBytes = 0
      if (written >= nextReport) {
        console.log(`  … ${Math.floor(written / MIB)} MiB / ${Math.round(targetBytes / MIB)} MiB`)
        nextReport += 256 * MIB
      }
    }
  }
  if (chunk.length > 0) writeSync(fd, Buffer.from(chunk.join(''), 'utf8'))
  const size = fstatSync(fd).size
  console.log(`已生成临时文件：${tmpPath}`)
  console.log(`  字节数：${size.toLocaleString()}（${(size / MIB).toFixed(1)} MiB）`)
  console.log(`  数据行：${lines.toLocaleString()}（另有 1 行 # 头注释，解析器跳过）`)
  console.log(`  行语义：后端 offline 行 no = 文件内第 N 数据行（1 起连续）`)
  console.log('')
  console.log('手测步骤：')
  console.log('  1. npm run tauri dev')
  console.log('  2. 「打开日志」选择上面这个临时文件')
  console.log('  3. 验收：打开近即时（索引 O(文件)，无进度条属预期）；视口停在文件尾；')
  console.log('     上滑翻页回补可逐页取回更早的行；bytetide 进程 RSS 增长 < 500MB')
  console.log('     （无全文过 IPC/WebView；Node 侧不测 RSS——真断言在 Rust 2M 行集成测试）')
  console.log('')
  if (process.stdin.isTTY) {
    const rl = createInterface({ input: process.stdin, output: process.stdout })
    await rl.question('测完按回车删除临时文件（Ctrl+C 退出同样会清理）…')
    rl.close()
  } else {
    console.log('（非 TTY 环境：直接清理）')
  }
} finally {
  cleanup()
}
console.log('临时文件已删除。')
