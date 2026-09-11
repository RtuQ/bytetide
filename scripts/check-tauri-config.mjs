// Tauri 配置预检：app.windows[].additionalBrowserArgs 里的 Windows 后台节流
// 规避 flag 必须齐全且各自恰好出现一次；--disable-features 的逗号列表里不允许
// 混入独立 flag——chromium 会把它当成名为 "--xxx" 的 feature 去禁用，真正的
// 独立开关反而没生效（后台/锁屏时定时器仍被节流）。
// CI 在打包前跑 `npm run check:tauri-config`，非法即失败。
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'

const REQUIRED_FLAGS = [
  '--disable-background-timer-throttling',
  '--disable-renderer-backgrounding',
  '--disable-backgrounding-occluded-windows',
  '--disable-intensive-wake-up-throttling',
]

/** 校验单个窗口的 additionalBrowserArgs；非法抛 Error（信息指明哪个 flag 有问题）。 */
export function validateAdditionalBrowserArgs(value) {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(
      'additionalBrowserArgs is empty or missing — the Windows background throttle workarounds are required and must not be removed',
    )
  }
  const tokens = value.trim().split(/\s+/)
  for (const token of tokens) {
    if (!token.startsWith('--disable-features=')) continue
    const items = token.slice('--disable-features='.length).split(',')
    for (const item of items) {
      if (item.startsWith('--')) {
        throw new Error(
          `additionalBrowserArgs: --disable-features list item "${item}" starts with -- — standalone flags must not be merged into the comma-separated feature list (chromium would treat it as a feature name and the real switch stays off)`,
        )
      }
    }
  }
  for (const flag of REQUIRED_FLAGS) {
    const count = tokens.filter((t) => t === flag).length
    if (count !== 1) {
      throw new Error(
        `additionalBrowserArgs: flag ${flag} must appear exactly once as a standalone argument, found ${count} time(s)`,
      )
    }
  }
}

async function main() {
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
  const confPath = path.join(root, 'src-tauri', 'tauri.conf.json')
  const conf = JSON.parse(await readFile(confPath, 'utf8'))
  const windows = conf?.app?.windows
  if (!Array.isArray(windows) || windows.length === 0) {
    throw new Error(`no app.windows array found in ${path.relative(root, confPath)}`)
  }
  for (const [i, win] of windows.entries()) {
    validateAdditionalBrowserArgs(win.additionalBrowserArgs)
    console.log(`check:tauri-config ok — window[${i}] "${win.title ?? i}" additionalBrowserArgs valid`)
  }
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invokedDirectly) {
  await main()
}
