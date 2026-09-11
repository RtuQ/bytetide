// 发布预检：四份版本来源必须一致（package.json / 根 [workspace.package] /
// src-tauri/Cargo.toml / src-tauri/tauri.conf.json），tag 构建时再对 GITHUB_REF_NAME。
// CI 与 release workflow 在打包前跑 `npm run check:version`，不一致即失败。
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'

/** 读取仓库根下四份清单的版本号，返回 { npm, workspace, desktop, tauri }。 */
export async function readVersions(root) {
  const [npmRaw, workspaceCargo, desktopCargo, tauriRaw] = await Promise.all([
    readFile(path.join(root, 'package.json'), 'utf8'),
    readFile(path.join(root, 'Cargo.toml'), 'utf8'),
    readFile(path.join(root, 'src-tauri', 'Cargo.toml'), 'utf8'),
    readFile(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'),
  ])
  const npm = JSON.parse(npmRaw).version
  const workspace = anchoredCargoVersion(workspaceCargo)
  const tauri = JSON.parse(tauriRaw).version
  // src-tauri 声明 version.workspace = true 时，实际值取自根 [workspace.package]
  const desktop = /^\s*version\s*\.\s*workspace\s*=\s*true\s*$/m.test(desktopCargo)
    ? workspace
    : anchoredCargoVersion(desktopCargo)
  return { npm, workspace, desktop, tauri }
}

/** 锚定的 Cargo version 字段解析：只认 [package]/[workspace.package] 段内的第一处。 */
function anchoredCargoVersion(cargoToml) {
  const m = cargoToml.match(/^\s*version\s*=\s*"([^"]+)"/m)
  if (!m) throw new Error('cannot find version = "..." in Cargo.toml')
  return m[1]
}

/** 四处版本必须一致；tag 以 v 开头时还须与之一致。返回统一版本号。 */
export function checkVersions(v, tag) {
  const distinct = new Set(Object.values(v))
  if (distinct.size !== 1) {
    throw new Error(
      `version mismatch across manifests: ${Object.entries(v)
        .map(([k, val]) => `${k}=${val}`)
        .join(', ')}`,
    )
  }
  const version = Object.values(v)[0]
  if (tag?.startsWith('v') && tag.slice(1) !== version) {
    throw new Error(`tag version ${tag} does not match manifests version ${version}`)
  }
  return version
}

async function main() {
  const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
  const versions = await readVersions(root)
  const tag = process.env.GITHUB_REF_NAME
  const version = checkVersions(versions, tag)
  console.log(`check:version ok — all manifests at ${version}${tag?.startsWith('v') ? ` (tag ${tag})` : ''}`)
}

const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invokedDirectly) {
  await main()
}
