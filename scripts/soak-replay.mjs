#!/usr/bin/env node
// 一键复跑回放 soak（Stage 3 Task 8 Step 4）。
//
// 用法：
//   node scripts/soak-replay.mjs [--quick] [--real] [--duration-min N] [--speed N]
//                                [--target-lines N] [--out FILE]
//
// 模式（真实 30 分钟 soak 不进 CI——默认 quick）：
//   --quick（默认）  跑 crates/bytetide-core/tests/replay_soak.rs 的 #[ignore]
//                    虚拟时钟测试：合成流 max_gap_ms=1 + speed=100 → 每行实际
//                    睡眠 0ms 整流瞬放，秒级跑完 30 分钟级虚拟时长（2 圈
//                    110k 行循环，ring 淘汰真实触顶 100k）。CI 同款。
//   --real           真实墙钟 soak：speed=100 + 默认 10s gap 钳制定速回放
//                    --duration-min（默认 30）分钟（≈18 万行），规则全程挂、
//                    Linux 采样 RSS 趋势。只在本地显式跑。
//
// 输出：Rust 侧打印 `SOAK_SUMMARY {json}`，本脚本解析后把最终摘要 JSON 打到
// stdout（--out FILE 时同时落盘）。字段（对齐 plan Task 8 接口约定）：
//   durationMs / producedLines / maxRingLines / completedLoops / errors[]
//   以及 observedWrapClears / alertHitEvents / alertHits / ringCap / speed /
//   rssKbMax / rssKbLast（仅 Linux）。
//
// 说明：前端视图上限（viewBufCap）是渲染进程策略，core soak 覆盖前后端共同的
// 数据源 ring；真实 30 分钟人工复跑时对照 perf-frontend.log / perf-heartbeat.log
// （lib.rs::open_diag_log 目录）核对前端行数与滞后。

import { spawnSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const argv = process.argv.slice(2)
const arg = (name) => {
  const i = argv.indexOf(name)
  return i >= 0 ? argv[i + 1] : undefined
}
const has = (name) => argv.includes(name)

const real = has('--real')
const durationMin = Number(arg('--duration-min') ?? 30)
const speed = Number(arg('--speed') ?? 100)
const targetLines = Number(arg('--target-lines') ?? 220000)
const outFile = arg('--out')
const errors = []

if (!Number.isFinite(durationMin) || durationMin <= 0) errors.push(`--duration-min 非法: ${durationMin}`)
if (!Number.isFinite(speed) || speed <= 0) errors.push(`--speed 非法: ${speed}`)
if (!Number.isFinite(targetLines) || targetLines <= 0) errors.push(`--target-lines 非法: ${targetLines}`)

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

// cargo test 输出：摘要行走 stdout（--nocapture），编译/进度走 stderr→inherit 透传
const env = { ...process.env }
if (real) env.SOAK_REAL = '1'
if (arg('--duration-min') !== undefined) env.SOAK_DURATION_MIN = String(durationMin)
if (arg('--speed') !== undefined) env.SOAK_SPEED = String(speed)
if (arg('--target-lines') !== undefined) env.SOAK_TARGET_LINES = String(targetLines)

const run = spawnSync(
  'cargo',
  ['test', '-p', 'bytetide-core', '--test', 'replay_soak', '--', '--ignored', '--nocapture'],
  { cwd: repoRoot, env, encoding: 'utf8' },
)

let summary = null
const summaryLine = (run.stdout ?? '')
  .split('\n')
  .find((l) => l.includes('SOAK_SUMMARY'))
if (summaryLine) {
  try {
    summary = JSON.parse(summaryLine.slice(summaryLine.indexOf('{')))
  } catch (e) {
    errors.push(`SOAK_SUMMARY 解析失败: ${e.message}`)
  }
} else {
  errors.push('未捕获 SOAK_SUMMARY（测试未跑完或编译失败，见上方 cargo 输出）')
}

if (run.status !== 0) errors.push(`cargo test 退出码 ${run.status}`)
if (run.error) errors.push(`cargo 启动失败: ${run.error.message}`)

const result = {
  mode: real ? 'real' : 'quick',
  durationMin: real ? durationMin : undefined,
  ...(summary ?? {}),
  errors,
}

const json = JSON.stringify(result, null, 2)
console.log(json)
if (outFile) {
  writeFileSync(path.resolve(repoRoot, outFile), json + '\n')
  console.error(`摘要已写入 ${outFile}`)
}
process.exit(errors.length === 0 ? 0 : 1)
