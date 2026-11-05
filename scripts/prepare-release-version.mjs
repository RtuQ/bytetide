// CI 专用版本准备：只修改当前 checkout 的清单与锁文件，不执行任何 git 操作。
import { readFile, writeFile } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { fileURLToPath, pathToFileURL } from 'node:url'
import path from 'node:path'
import { bumpIn } from './bump-version.mjs'
import { checkVersions } from './check-release-version.mjs'
import { versionFromTag } from './release-tag.mjs'

/** 严格刷新 workspace 锁文件；工具缺失或非零退出都阻止发布。 */
function refreshCargoLock(root) {
  const result = spawnSync('cargo', ['update', '--workspace'], { cwd: root, stdio: 'inherit' })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`cargo update --workspace failed: ${result.signal ?? result.status}`)
}

/** 将 tag 同步到构建清单与两份锁文件；Cargo runner 可注入以便离线单测。 */
export async function prepareReleaseVersion(root, tag, refreshLock = refreshCargoLock) {
  const version = versionFromTag(tag)
  const lockPath = path.join(root, 'package-lock.json')
  const lock = JSON.parse(await readFile(lockPath, 'utf8'))
  if (!lock.packages?.[''] || typeof lock.version !== 'string') {
    throw new Error('package-lock.json must contain root version and packages[""] metadata')
  }
  const versions = await bumpIn(root, version, refreshLock)
  checkVersions(versions, tag)
  lock.version = version
  lock.packages[''].version = version
  await writeFile(lockPath, JSON.stringify(lock, null, 2) + '\n')
  return version
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = fileURLToPath(new URL('..', import.meta.url))
  const tag = process.argv[2] ?? process.env.GITHUB_REF_NAME
  const version = await prepareReleaseVersion(root, tag)
  console.log(`release: prepared ${version} from ${tag} in this checkout; no commits or pushes`)
}
