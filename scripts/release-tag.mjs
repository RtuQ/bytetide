// 与 Cargo / npm 共用的规范 SemVer：数字标识不允许多余前导零。
const NUMBER = '(?:0|[1-9][0-9]*)'
const IDENTIFIER = `(?:${NUMBER}|[0-9]*[A-Za-z-][0-9A-Za-z-]*)`
const VERSION = `${NUMBER}\\.${NUMBER}\\.${NUMBER}(?:-${IDENTIFIER}(?:\\.${IDENTIFIER})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?`
const RELEASE_TAG = new RegExp(`^(?:cli-v|v)(${VERSION})$`)

/** 从桌面或独立 CLI 发布标签提取版本，拒绝分支名和非规范版本。 */
export function versionFromTag(tag) {
  const match = typeof tag === 'string' ? RELEASE_TAG.exec(tag) : null
  if (!match) throw new Error(`invalid release tag: ${String(tag)}; expected v1.2.3 or cli-v1.2.3`)
  return match[1]
}
