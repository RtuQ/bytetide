import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { createManifest } from './desktop-update-manifest.mjs'

async function fixture(run) {
  const directory = await mkdtemp(path.join(tmpdir(), 'bytetide-manifest-'))
  try {
    for (const name of ['ByteTide.exe', 'ByteTide.app.tar.gz']) {
      await writeFile(path.join(directory, name), 'artifact')
      await writeFile(path.join(directory, name + '.sig'), Buffer.from('test signature').toString('base64') + '\n')
    }
    await run(directory)
  } finally { await rm(directory, { recursive: true, force: true }) }
}

test('creates platform-specific immutable release URLs and inline signatures', async () => {
  await fixture(async (directory) => {
    const result = await createManifest(directory, 'v0.8.0')
    assert.equal(result.version, '0.8.0')
    assert.deepEqual(Object.keys(result.platforms).sort(), ['darwin-aarch64', 'windows-x86_64'])
    assert.equal(result.platforms['windows-x86_64'].url, 'https://github.com/RtuQ/bytetide/releases/download/v0.8.0/ByteTide.exe')
    assert.equal(result.platforms['darwin-aarch64'].signature, Buffer.from('test signature').toString('base64'))
  })
})

test('rejects CLI tags and prereleases', async () => {
  await fixture(async (directory) => {
    for (const tag of ['cli-v0.8.0', 'v0.8.0-rc.1', 'v01.0.0', 'v0.8.0/evil']) {
      await assert.rejects(createManifest(directory, tag), /stable desktop/)
    }
  })
})

test('does not emit a manifest with missing or empty signatures', async () => {
  await fixture(async (directory) => {
    await rm(path.join(directory, 'ByteTide.exe.sig'))
    await assert.rejects(createManifest(directory, 'v0.8.0'))
    await writeFile(path.join(directory, 'ByteTide.exe.sig'), '')
    await assert.rejects(createManifest(directory, 'v0.8.0'), /signature/)
  })
})
