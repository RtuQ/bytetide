// check-architecture.mjs 的单测（node:test）。用内存 fixture 树注入
// （options.files + options.read），不落盘——规则语义逐条验证：
// core 纯净性 / 前端 IPC 边界（允许清单）/ 文件行数限额，且一次报出全部违规。
import test from 'node:test'
import assert from 'node:assert/strict'
import { checkArchitecture } from './check-architecture.mjs'

/** 构造注入用的 fixture 树：{ 文件相对路径: 内容 } */
function fixture(files) {
  const map = new Map(Object.entries(files))
  return {
    files: [...map.keys()].sort(),
    read: (rel) => {
      const content = map.get(rel)
      if (content === undefined) throw new Error(`fixture missing: ${rel}`)
      return content
    },
  }
}

const CLEAN_CORE_TOML = `[package]
name = "bytetide-core"

[dependencies]
serde = { workspace = true }
serialport = { workspace = true }
`

const CLEAN_CORE_RS = 'use serde::Serialize;\n\npub fn x() {}\n'

const CLEAN_SRC_TS = "import { ref } from 'vue'\nexport const n = ref(0)\n"

test('core Cargo.toml 含 tokio 依赖被拒', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': `${CLEAN_CORE_TOML}tokio = { version = "1" }\n`,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/app.ts': CLEAN_SRC_TS,
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /crates\/bytetide-core\/Cargo\.toml/)
    assert.match(e.message, /tokio/)
    return true
  })
})

test('core Cargo.toml 含 tauri/axum 依赖同样被拒', () => {
  for (const dep of ['tauri', 'axum']) {
    const f = fixture({
      'crates/bytetide-core/Cargo.toml': `${CLEAN_CORE_TOML}${dep} = "2"\n`,
      'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
      'src/app.ts': CLEAN_SRC_TS,
    })
    assert.throws(() => checkArchitecture('/fixture', f), new RegExp(dep))
  }
})

test('core .rs 出现 use tokio:: 被拒（带文件与行号）', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/foo.rs': 'use serde::Serialize;\nuse tokio::io::AsyncReadExt;\n',
    'src/app.ts': CLEAN_SRC_TS,
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /crates\/bytetide-core\/src\/foo\.rs:2/)
    assert.match(e.message, /use tokio/)
    return true
  })
})

test('core .rs 出现 tauri:: / axum:: 路径字样被拒', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/foo.rs': 'pub fn bad() -> tauri::AppHandle\n',
    'crates/bytetide-core/src/bar.rs': 'let e: axum::Error;\n',
    'src/app.ts': CLEAN_SRC_TS,
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /foo\.rs.*tauri::/)
    assert.match(e.message, /bar\.rs.*axum::/)
    return true
  })
})

test('core .rs 注释行里的 tauri 字样不算违规', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/foo.rs': '// use tauri::AppHandle 旧用法备忘\n/// tauri:: 依赖已移除\npub fn ok() {}\n',
    'src/app.ts': CLEAN_SRC_TS,
  })
  assert.doesNotThrow(() => checkArchitecture('/fixture', f))
})

test('允许清单之外的 src 文件裸 invoke( 被拒', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/components/Some.vue': `<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
await invoke('send_cmd')
</script>
`,
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /src\/components\/Some\.vue/)
    assert.match(e.message, /invoke/)
    assert.match(e.message, /@tauri-apps\/api\/core/)
    return true
  })
})

test('允许清单之外的 src 文件裸 listen( 被拒', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/stray.ts': "import { listen } from '@tauri-apps/api/event'\nawait listen('x', () => {})\n",
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /src\/stray\.ts/)
    assert.match(e.message, /listen/)
    return true
  })
})

test('src/ipc/** 与允许清单内文件的 invoke/listen 放行', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/ipc/client.ts': "import { invoke, listen } from '@tauri-apps/api/core'\nawait invoke('x')\nawait listen('y', () => {})\n",
    'src/composables/useTauriEvents.ts': "import { listen, type UnlistenFn } from '@tauri-apps/api/event'\nimport { invoke } from '@tauri-apps/api/core'\nawait listen('session-status', () => {})\nvoid invoke('ring_lines_no_cmd')\n",
    'src/stores/session.ts': "import { invoke } from '@tauri-apps/api/core'\nawait invoke('send_cmd')\n",
    'src/stores/other.ts': "import { ref } from 'vue'\nexport const r = ref(unlisten)\n",
  })
  assert.doesNotThrow(() => checkArchitecture('/fixture', f))
})

test('非 IPC 的 tauri API（window/app/plugin-*）不受限', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/composables/useTheme.ts': "import { getCurrentWindow } from '@tauri-apps/api/window'\nimport { openUrl } from '@tauri-apps/plugin-opener'\n",
  })
  assert.doesNotThrow(() => checkArchitecture('/fixture', f))
})

test('行数超限文件被拒且错误含文件名与实际行数', () => {
  const big = Array.from({ length: 1201 }, (_, i) => `// line ${i + 1}`).join('\n') + '\n'
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/cli.ts': big,
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /src\/cli\.ts/)
    assert.match(e.message, /1201/)
    return true
  })
})

test('覆盖表里的文件按各自阈值判（阈值内通过、超限拒绝）', () => {
  const lines = (n) => Array.from({ length: n }, (_, i) => `// ${i + 1}`).join('\n') + '\n'
  const base = {
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
  }
  // manager.rs 2750 行在覆盖阈值内、默认 1200 会误报 → 覆盖表生效
  const within = fixture({ ...base, 'crates/bytetide-core/src/serial/manager.rs': lines(2750) })
  assert.doesNotThrow(() => checkArchitecture('/fixture', within))
  const over = fixture({ ...base, 'crates/bytetide-core/src/serial/manager.rs': lines(2751) })
  assert.throws(() => checkArchitecture('/fixture', over), /2751/)
})

test('tests 目录与 *.test.ts 不参与行数限额', () => {
  const big = Array.from({ length: 1300 }, (_, i) => `// ${i + 1}`).join('\n') + '\n'
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'crates/bytetide-core/tests/big_contract.rs': big,
    'src/stores/__tests__/session.test.ts': big,
    'src/lib.test.ts': big,
  })
  assert.doesNotThrow(() => checkArchitecture('/fixture', f))
})

test('多处违规一次全部报出，不遇错即停', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': `${CLEAN_CORE_TOML}tokio = "1"\n`,
    'crates/bytetide-core/src/lib.rs': 'use tokio::sync::mpsc;\n',
    'src/stray.ts': "await invoke('x')\n",
    'src/big.ts': Array.from({ length: 1201 }, (_, i) => `// ${i + 1}`).join('\n') + '\n',
  })
  assert.throws(() => checkArchitecture('/fixture', f), (e) => {
    assert.match(e.message, /Cargo\.toml/)
    assert.match(e.message, /lib\.rs/)
    assert.match(e.message, /stray\.ts/)
    assert.match(e.message, /big\.ts/)
    return true
  })
})

test('干净树通过（返回违规数 0 的摘要）', () => {
  const f = fixture({
    'crates/bytetide-core/Cargo.toml': CLEAN_CORE_TOML,
    'crates/bytetide-core/src/lib.rs': CLEAN_CORE_RS,
    'src/app.ts': CLEAN_SRC_TS,
  })
  const summary = checkArchitecture('/fixture', f)
  assert.equal(summary.violations, 0)
})

test('缺 Cargo.toml/src 目录的最小树也能通过（按文件逐个判定）', () => {
  const f = fixture({ 'src/app.ts': CLEAN_SRC_TS })
  assert.doesNotThrow(() => checkArchitecture('/fixture', f))
})
