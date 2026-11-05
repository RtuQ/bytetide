import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises'
import { spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { versionFromTag } from './release-tag.mjs'
import { prepareReleaseVersion } from './prepare-release-version.mjs'
import { readVersions, checkVersions } from './check-release-version.mjs'

async function fixture(t) {
  const root = await mkdtemp(path.join(tmpdir(), 'bytetide-release-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const files = {
    'package.json': JSON.stringify({ name: 'demo', version: '0.1.0', private: true }),
    'package-lock.json': JSON.stringify({ name: 'demo', version: '0.1.0', lockfileVersion: 3, packages: { '': { name: 'demo', version: '0.1.0' }, 'node_modules/example': { version: '2.0.0', integrity: 'unchanged' } } }),
    'Cargo.toml': '[workspace]\nresolver = "2"\nmembers = ["crates/core", "crates/cli", "src-tauri"]\n[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n',
    'src-tauri/tauri.conf.json': JSON.stringify({ version: '0.1.0' }),
  }
  // Existing bump helpers preserve line formatting and expect manifest version keys on their own line.
  for (const file of ['package.json', 'src-tauri/tauri.conf.json']) files[file] = JSON.stringify(JSON.parse(files[file]), null, 2) + '\n'
  for (const [directory, name] of [['crates/core', 'fixture-core'], ['crates/cli', 'fixture-cli'], ['src-tauri', 'fixture-desktop']]) {
    files[directory + '/Cargo.toml'] = `[package]\nname = "${name}"\nversion.workspace = true\nedition.workspace = true\n`
    files[directory + '/src/lib.rs'] = ''
  }
  for (const [file, contents] of Object.entries(files)) {
    await mkdir(path.dirname(path.join(root, file)), { recursive: true })
    await writeFile(path.join(root, file), contents)
  }
  return root
}

function lockVersions(text) {
  return [...text.matchAll(/\[\[package\]\]\nname = "fixture-[^"]+"\nversion = "([^"]+)"/g)].map(match => match[1])
}

test('extracts canonical desktop and CLI SemVer tags', () => {
  for (const prefix of ['v', 'cli-v']) {
    for (const version of ['1.2.3', '0.8.0-rc.1', '1.2.3+build.42']) {
      assert.equal(versionFromTag(prefix + version), version)
    }
  }
  for (const tag of ['', 'main', '0.8.0', 'v01.2.3', 'cli-v1.2.3-01', 'v1.2.3/evil', 'v1.2', 'v1.2.3-', 'v1.2.3+']) {
    assert.throws(() => versionFromTag(tag), /release tag/)
  }
})

test('syncs manifests and npm root lock metadata without changing dependencies', async (t) => {
  const root = await fixture(t)
  let calls = 0
  const before = JSON.parse(await readFile(path.join(root, 'package-lock.json'), 'utf8'))
  const version = await prepareReleaseVersion(root, 'cli-v0.8.0', () => { calls++ })
  assert.equal(version, '0.8.0')
  assert.equal(calls, 1)
  assert.equal(checkVersions(await readVersions(root), 'cli-v0.8.0'), '0.8.0')
  const after = JSON.parse(await readFile(path.join(root, 'package-lock.json'), 'utf8'))
  assert.equal(after.version, '0.8.0')
  assert.equal(after.packages[''].version, '0.8.0')
  assert.deepEqual(after.packages['node_modules/example'], before.packages['node_modules/example'])
  const first = await readFile(path.join(root, 'package-lock.json'), 'utf8')
  await prepareReleaseVersion(root, 'cli-v0.8.0', () => {})
  assert.equal(await readFile(path.join(root, 'package-lock.json'), 'utf8'), first)
})

test('invalid tags fail before modifying manifests', async (t) => {
  const root = await fixture(t)
  const before = await readFile(path.join(root, 'package.json'), 'utf8')
  await assert.rejects(prepareReleaseVersion(root, 'main', () => assert.fail('cargo must not run')), /release tag/)
  assert.equal(await readFile(path.join(root, 'package.json'), 'utf8'), before)
})

test('Cargo lock refresh failure aborts release preparation', async (t) => {
  const root = await fixture(t)
  await assert.rejects(prepareReleaseVersion(root, 'v0.8.0', () => { throw new Error('cargo failed') }), /cargo failed/)
})

test('missing npm lock root fails before modifying manifests', async (t) => {
  const root = await fixture(t)
  await writeFile(path.join(root, 'package-lock.json'), '{}')
  await assert.rejects(prepareReleaseVersion(root, 'v0.8.0', () => {}), /lock/)
  assert.equal((await readVersions(root)).npm, '0.1.0')
})

const cargoAvailable = !spawnSync('cargo', ['--version']).error
test('real Cargo updates all workspace lock versions from each tag namespace', { skip: !cargoAvailable }, async (t) => {
  const root = await fixture(t)
  // An empty dependency graph makes this a fully offline integration test.
  for (const tag of ['v0.8.0', 'cli-v0.9.0-rc.1']) {
    const version = await prepareReleaseVersion(root, tag)
    assert.deepEqual(lockVersions(await readFile(path.join(root, 'Cargo.lock'), 'utf8')), [version, version, version])
    assert.equal(checkVersions(await readVersions(root), tag), version)
    assert.equal(spawnSync('cargo', ['metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'], { cwd: root }).status, 0)
    assert.equal((await readFile(path.join(root, 'src-tauri/Cargo.toml'), 'utf8')).includes('version.workspace = true'), true)
  }
})
