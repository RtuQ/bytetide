import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { applyCargoVersion, applyJsonVersion, bumpIn } from './bump-version.mjs'
import { readVersions, checkVersions } from './check-release-version.mjs'

const FIXTURES = {
  'package.json': '{\n  "name": "demo",\n  "version": "0.1.0"\n}\n',
  'Cargo.toml':
    '# 注释\n[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n\n[workspace.dependencies.serde]\nversion = "1.0"\n',
  'src-tauri/Cargo.toml': '[package]\nname = "demo"\nversion.workspace = true\n',
  'src-tauri/tauri.conf.json': '{\n  "productName": "demo",\n  "version": "0.1.0"\n}\n',
}

async function fixtureRoot() {
  const root = await mkdtemp(path.join(tmpdir(), 'bump-'))
  for (const [rel, content] of Object.entries(FIXTURES)) {
    await mkdir(path.dirname(path.join(root, rel)), { recursive: true })
    await writeFile(path.join(root, rel), content)
  }
  return root
}

test('applyCargoVersion 只改首个锚定 version，不动依赖表里的 version', () => {
  const out = applyCargoVersion(FIXTURES['Cargo.toml'], '0.2.0')
  assert.ok(out.includes('version = "0.2.0"\nedition'), 'workspace.package 版本未替换')
  assert.ok(out.endsWith('[workspace.dependencies.serde]\nversion = "1.0"\n'), '依赖表 version 被误改')
})

test('applyCargoVersion 无 version 字段时抛错', () => {
  assert.throws(() => applyCargoVersion('[workspace]\nresolver = "2"\n', '0.2.0'), /cannot find/)
})

test('applyJsonVersion 只替换 version 键且保持其余格式不变', () => {
  const out = applyJsonVersion(FIXTURES['package.json'], '0.2.0')
  assert.equal(out, '{\n  "name": "demo",\n  "version": "0.2.0"\n}\n')
})

test('applyJsonVersion 不误伤含 version 字样的其他键', () => {
  const src = '{\n  "version": "0.1.0",\n  "minVersionNote": "keep me"\n}\n'
  assert.ok(applyJsonVersion(src, '0.2.0').includes('"minVersionNote": "keep me"'))
})

test('applyJsonVersion 无 version 键时抛错', () => {
  assert.throws(() => applyJsonVersion('{\n  "name": "demo"\n}\n', '0.2.0'), /cannot find/)
})

test('bumpIn 四处清单同步至新版本并调用 cargo 刷锁', async (t) => {
  const root = await fixtureRoot()
  t.after(() => rm(root, { recursive: true, force: true }))
  const calls = []
  const v = await bumpIn(root, '0.2.0', () => calls.push('cargo'))
  assert.equal(calls.length, 1, 'cargo 刷锁应恰被调用一次')
  assert.equal(checkVersions(v), '0.2.0')
  assert.equal(checkVersions(await readVersions(root)), '0.2.0')
})

test('bumpIn 重复升到同一版本幂等', async (t) => {
  const root = await fixtureRoot()
  t.after(() => rm(root, { recursive: true, force: true }))
  const noop = () => {}
  await bumpIn(root, '0.2.0', noop)
  await bumpIn(root, '0.2.0', noop)
  assert.equal(checkVersions(await readVersions(root)), '0.2.0')
})

test('bumpIn 修正四处不一致的存量版本', async (t) => {
  const root = await fixtureRoot()
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(
    path.join(root, 'src-tauri', 'tauri.conf.json'),
    FIXTURES['src-tauri/tauri.conf.json'].replace('0.1.0', '0.0.9'),
  )
  await bumpIn(root, '0.2.0', () => {})
  assert.equal(checkVersions(await readVersions(root)), '0.2.0')
  const raw = await readFile(path.join(root, 'src-tauri', 'tauri.conf.json'), 'utf8')
  assert.ok(raw.includes('"version": "0.2.0"'), 'tauri.conf.json 落后的版本应被改齐')
})
