// 架构红线预检：core 纯净性 + 前端 IPC 边界 + 文件行数限额。
// Stage 2（docs/superpowers/plans/2026-09-11-stage-2-architecture-modularization.md）
// Task 1 落地，后续大重构（Task 2/3/4/5/6）逐级收紧阈值与允许清单：
// - core（bytetide-core）永不依赖 tauri/axum/tokio——桌面与 CLI 共用的纯逻辑层
// - 前端直连 Tauri IPC（invoke/listen/core|event import）只允许 src/ipc/** 与
//   初始允许清单（Task 5 建立类型化 IPC 适配层后清空清单）
// - 单文件行数限额防「继续往大文件里塞」——覆盖表是现实基线，收紧路径见注释
// CI 在 Rust 测试前跑 `npm run check:architecture`，违规即失败。
import { readdirSync, readFileSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'

// ---------------------------------------------------------------------------
// 规则 1：core 纯净性
// ---------------------------------------------------------------------------
const FORBIDDEN_CORE_DEPS = ['tauri', 'axum', 'tokio']
// .rs 源码里的禁止字样（注释行豁免——文档里提及历史用法不算依赖）
const FORBIDDEN_CORE_SNIPPETS = ['use tauri', 'use axum', 'use tokio', 'tauri::', 'axum::', 'tokio::']

/** 解析 Cargo.toml [dependencies] 段的依赖名（含 [dependencies.xxx] 子表形式）。 */
export function cargoDependencyNames(cargoToml) {
  const names = new Set()
  let section = null
  for (const raw of cargoToml.split('\n')) {
    const line = raw.trim()
    const header = line.match(/^\[(.+?)\]/)
    if (header) {
      section = header[1].trim()
      continue
    }
    if (!line || line.startsWith('#')) continue
    if (section === 'dependencies') {
      const key = line.match(/^([A-Za-z0-9_-]+)\s*[=.]/)
      if (key) names.add(key[1])
    } else if (section?.startsWith('dependencies.')) {
      // [dependencies.tauri] / [dependencies.tauri.features] 等
      const rest = section.slice('dependencies.'.length)
      names.add(rest.split('.')[0])
    }
  }
  return names
}

// ---------------------------------------------------------------------------
// 规则 2：前端 IPC 边界（Task 5 起：允许清单已清空，直连只允许 src/ipc/**）
// ---------------------------------------------------------------------------
// Task 1 时曾列出全部直连文件作过渡基线；Task 5 建立类型化 IPC 适配层
// （src/ipc/{client,commands,events,types,errors}.ts）并把调用方逐个迁走后，
// 清单清空。新增任何 invoke/listen/@tauri-apps/api(core|event) 直连都算违规：
// 一律经 src/ipc 的命名命令/事件适配层（IpcClient 接口可注入假实现做测试）。
const ALLOWED_IPC_FILES = []

// 裸调用匹配：invoke( / invoke<…>( / listen( ——不匹配 x.invoke(、unlisten(、invokeMock(
const BARE_INVOKE = /(?<![\w$.])invoke\s*[<(]/
const BARE_LISTEN = /(?<![\w$.])listen\s*[<(]/
const IPC_IMPORT = /from\s+['"]@tauri-apps\/api\/(core|event)['"]/

// ---------------------------------------------------------------------------
// 规则 3：文件行数限额
// ---------------------------------------------------------------------------
const DEFAULT_MAX_LINES = 1200
// 显式覆盖 = 当前现实基线（后续任务逐级收紧，路径为 / 分隔的相对路径）：
// - crates/bytetide-core/src/serial/manager.rs 2750 → T2 后 2250 → T3 收到 1000（现 852 行，plan 目标达成）
// - src-tauri/src/bridge.rs 4300 → Task 4 拆为模块后此条目删除/收紧到 1000
// - src/stores/session.ts 1450 → Task 6 收到 700
const MAX_LINES_OVERRIDES = {
  'crates/bytetide-core/src/serial/manager.rs': 1000,
  'src-tauri/src/bridge.rs': 4300,
  'src/stores/session.ts': 1450,
}

/** wc -l 语义的行数（末尾有换行不打虚行；无换行的末行也计 1）。 */
export function countLines(content) {
  if (content === '') return 0
  const lines = content.split('\n')
  if (lines[lines.length - 1] === '') lines.pop()
  return lines.length
}

// ---------------------------------------------------------------------------
// 文件集分类
// ---------------------------------------------------------------------------
const isCoreRs = (rel) => rel.startsWith('crates/bytetide-core/') && rel.endsWith('.rs')
const isCoreCargo = (rel) => rel === 'crates/bytetide-core/Cargo.toml'
const isSrcTsVue = (rel) => rel.startsWith('src/') && /\.(ts|vue)$/.test(rel)
const isTestFile = (rel) => /(^|\/)(__tests__|tests)\//.test(rel) || /\.test\.ts$/.test(rel)
const isLineLimited = (rel) =>
  (rel.startsWith('crates/') && rel.endsWith('.rs')) ||
  (rel.startsWith('src/') && /\.(ts|vue)$/.test(rel)) ||
  (rel.startsWith('src-tauri/src/') && rel.endsWith('.rs'))

const isCommentLine = (line) => {
  const t = line.trim()
  return t.startsWith('//') || t.startsWith('//!')
}

/** 行首出现的禁止字样及行号（注释行跳过）。 */
function coreRsViolations(rel, content) {
  const out = []
  for (const [i, line] of content.split('\n').entries()) {
    if (isCommentLine(line)) continue
    for (const snippet of FORBIDDEN_CORE_SNIPPETS) {
      if (line.includes(snippet)) {
        out.push(`  - core 代码纯净: ${rel}:${i + 1} 出现 "${snippet}"（bytetide-core 禁止依赖 tauri/axum/tokio）`)
      }
    }
  }
  return out
}

function ipcViolations(rel, content) {
  if (rel.startsWith('src/ipc/') || ALLOWED_IPC_FILES.includes(rel)) return []
  const out = []
  for (const [i, line] of content.split('\n').entries()) {
    const imp = line.match(IPC_IMPORT)
    if (imp) {
      out.push(`  - IPC 边界: ${rel}:${i + 1} import from '@tauri-apps/api/${imp[1]}'（只允许 src/ipc/** 或允许清单内）`)
    }
    if (BARE_INVOKE.test(line)) {
      out.push(`  - IPC 边界: ${rel}:${i + 1} 裸 invoke( 调用（只允许 src/ipc/** 或允许清单内）`)
    }
    if (BARE_LISTEN.test(line)) {
      out.push(`  - IPC 边界: ${rel}:${i + 1} 裸 listen( 调用（只允许 src/ipc/** 或允许清单内）`)
    }
  }
  return out
}

function lineLimitViolations(rel, content) {
  if (!isLineLimited(rel) || isTestFile(rel)) return []
  const n = countLines(content)
  const limit = MAX_LINES_OVERRIDES[rel] ?? DEFAULT_MAX_LINES
  if (n <= limit) return []
  return [
    `  - 行数限额: ${rel} 共 ${n} 行，超过上限 ${limit}（默认 ${DEFAULT_MAX_LINES}；覆盖表为现实基线，逐步收紧中）`,
  ]
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------

/**
 * 扫描并校验三条架构红线；返回 { files, violations } 摘要，
 * 有违规时 throw Error（信息含全部违规，一次报全不遇错即停）。
 * options 供测试注入自定义文件树：
 *   - files: 相对路径数组（默认递归走真树，跳过 target/node_modules）
 *   - read:  (relPath) => string 文件内容读取器（默认 readFileSync(root/rel)）
 */
export function checkArchitecture(root, options = {}) {
  const files = options.files ?? walkRoot(root)
  const read = options.read ?? ((rel) => readFileSync(path.join(root, rel), 'utf8'))
  const violations = []

  for (const rel of [...files].sort()) {
    let content
    try {
      content = read(rel)
    } catch {
      continue // 文件读不到（被删/会话竞态）跳过，行数类规则对缺失文件无意义
    }
    if (isCoreCargo(rel)) {
      for (const dep of FORBIDDEN_CORE_DEPS) {
        if (cargoDependencyNames(content).has(dep)) {
          violations.push(
            `  - core 依赖纯净: ${rel} [dependencies] 含禁止依赖 "${dep}"（bytetide-core 禁止 tauri/axum/tokio）`,
          )
        }
      }
      continue
    }
    if (isCoreRs(rel)) violations.push(...coreRsViolations(rel, content))
    if (isSrcTsVue(rel) && !isTestFile(rel)) violations.push(...ipcViolations(rel, content))
    violations.push(...lineLimitViolations(rel, content))
  }

  if (violations.length > 0) {
    throw new Error(`architecture violations (${violations.length}):\n${violations.join('\n')}`)
  }
  return { files, violations: 0 }
}

/** 递归收集仓库源文件（相对 posix 路径）；跳过 target/node_modules/.git。 */
function walkRoot(root) {
  const SKIP_DIRS = new Set(['target', 'node_modules', '.git'])
  const out = []
  const visit = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const abs = path.join(dir, entry.name)
      const rel = path.relative(root, abs).split(path.sep).join('/')
      if (entry.isDirectory()) {
        if (!SKIP_DIRS.has(entry.name)) visit(abs)
      } else if (entry.isFile()) {
        out.push(rel)
      }
    }
  }
  visit(root)
  return out
}

async function main() {
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
  const summary = checkArchitecture(root)
  console.log(
    `check:architecture ok — ${summary.files.length} 个源文件，core 纯净 / IPC 边界（允许清单 ${ALLOWED_IPC_FILES.length} 个文件）/ 行数限额全部通过`,
  )
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invokedDirectly) {
  await main()
}
