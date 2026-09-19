// 一键升版本：npm run bump -- 0.8.0
// 同步四处版本清单（package.json / 根 [workspace.package] / src-tauri 版本继承 /
// tauri.conf.json——release workflow 打 DMG 仍读后者，不可删字段）并跑
// cargo update --workspace 刷新 Cargo.lock。只改文件不碰 git，结尾打印发版后续
// 命令；四处一致性最终由 CI `check:version` 门禁兜底。
import { readFile, writeFile } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'
import { readVersions, checkVersions } from './check-release-version.mjs'

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/

/** 替换首个锚定的 version = "..."（根 Cargo.toml 里首处即 [workspace.package] 段）。 */
export function applyCargoVersion(cargoToml, version) {
  let matched = false
  const out = cargoToml.replace(/^(\s*version\s*=\s*")[^"]*(")/m, (_m, p1, p2) => {
    matched = true
    return `${p1}${version}${p2}`
  })
  if (!matched) throw new Error('cannot find version = "..." to bump in Cargo.toml')
  return out
}

/** 替换 JSON 文本首个顶层 "version": "..."（package.json / tauri.conf.json 均在文件头部）。 */
export function applyJsonVersion(jsonText, version) {
  let matched = false
  const out = jsonText.replace(/^(\s*"version"\s*:\s*")[^"]*(")/m, (_m, p1, p2) => {
    matched = true
    return `${p1}${version}${p2}`
  })
  if (!matched) throw new Error('cannot find "version" key to bump in JSON manifest')
  return out
}

/** 落盘三份手写清单（src-tauri/Cargo.toml 经 workspace 继承无需改），跑 cargo
 *  刷锁后自检四处一致。runCargo 可注入便于测试。 */
export async function bumpIn(root, version, runCargo = runCargoUpdate) {
  const targets = [
    ['package.json', applyJsonVersion],
    ['Cargo.toml', applyCargoVersion],
    ['src-tauri/tauri.conf.json', applyJsonVersion],
  ]
  for (const [rel, apply] of targets) {
    const p = path.join(root, rel)
    await writeFile(p, apply(await readFile(p, 'utf8'), version))
  }
  runCargo(root)
  const v = await readVersions(root)
  checkVersions(v)
  return v
}

function runCargoUpdate(root) {
  const res = spawnSync('cargo', ['update', '--workspace'], { cwd: root, stdio: 'inherit' })
  if (res.error) {
    console.warn('warn: cargo 不可用，请手动执行 `cargo update --workspace` 刷新 Cargo.lock')
  }
}

async function main() {
  const target = process.argv[2]
  if (!target || !SEMVER.test(target)) {
    console.error('usage: npm run bump -- 0.8.0   # 语义化版本号，不带 v 前缀')
    process.exit(2)
  }
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
  const current = await readVersions(root)
  const distinct = new Set(Object.values(current))
  if (distinct.size === 1 && current.npm === target) {
    console.log(`bump: 四处清单已是 ${target}，无需改动`)
    return
  }
  if (distinct.size !== 1) {
    console.warn(`warn: 当前清单版本不一致（${Object.entries(current).map(([k, x]) => `${k}=${x}`).join(', ')}），本次统一改为 ${target}`)
  }
  await bumpIn(root, target)
  console.log(`bump: 四处版本清单已同步至 ${target}，Cargo.lock 已刷新`)
  console.log(`发版后续步骤：
  git add Cargo.toml Cargo.lock package.json src-tauri/tauri.conf.json
  git commit -m "chore(release): 版本升至 ${target}"
  git tag -a v${target} -m "ByteTide ${target}"
  git push origin main v${target}   # 推 tag 触发 release workflow（出草稿 Release）
  # 最后在 GitHub 上补 Release 说明并发布草稿`)
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invokedDirectly) {
  await main()
}
