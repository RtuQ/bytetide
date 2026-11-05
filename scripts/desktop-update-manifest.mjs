// 发布清单仅引用已经签名的桌面产物，固定到不可混淆的 v* 标签。
import { readFile, stat, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

/** 从已签名的 Windows 便携 EXE 和 macOS 应用包生成 Tauri 更新清单。 */
export async function createManifest(directory, tag) {
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag)) {
    throw new Error('expected a stable desktop v* tag')
  }
  const platforms = {}
  for (const [target, filename] of [
    ['windows-x86_64', 'ByteTide.exe'],
    ['darwin-aarch64', 'ByteTide.app.tar.gz'],
  ]) {
    const info = await stat(path.join(directory, filename))
    if (!info.isFile() || info.size === 0) throw new Error(`missing update artifact: ${filename}`)
    const signature = (await readFile(path.join(directory, filename + '.sig'), 'utf8')).trim()
    if (!signature || !/^[A-Za-z0-9+/]+={0,2}$/.test(signature)) throw new Error(`invalid signature: ${filename}`)
    platforms[target] = {
      signature,
      url: `https://github.com/RtuQ/bytetide/releases/download/${tag}/${filename}`,
    }
  }
  return { version: tag.slice(1), platforms }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const directory = process.argv[2] ?? 'dist'
  const tag = process.argv[3] ?? process.env.GITHUB_REF_NAME ?? ''
  const manifest = await createManifest(directory, tag)
  await writeFile(path.join(directory, 'desktop-update.json'), JSON.stringify(manifest, null, 2) + '\n')
}
