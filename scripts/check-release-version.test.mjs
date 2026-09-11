import test from 'node:test'
import assert from 'node:assert/strict'
import { checkVersions } from './check-release-version.mjs'

test('accepts four equal versions and matching tag', () => {
  assert.equal(checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.5.1', tauri: '0.5.1' }, 'v0.5.1'), '0.5.1')
})

test('rejects a stale desktop crate version', () => {
  assert.throws(() => checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.4.1', tauri: '0.5.1' }), /version mismatch/)
})

test('rejects a tag that differs from the manifests', () => {
  assert.throws(() => checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.5.1', tauri: '0.5.1' }, 'v0.6.0'), /tag version/)
})

test('mismatch message lists every source and value', () => {
  try {
    checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.4.1', tauri: '0.5.1' })
    assert.fail('should throw')
  } catch (err) {
    const msg = String(err.message)
    for (const key of ['npm', 'workspace', 'desktop', 'tauri']) assert.ok(msg.includes(key), `missing ${key} in: ${msg}`)
    assert.ok(msg.includes('0.4.1'), 'missing stale value in message')
  }
})

test('accepts four equal versions without a tag', () => {
  assert.equal(checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.5.1', tauri: '0.5.1' }), '0.5.1')
})

test('ignores a tag that does not start with v', () => {
  assert.equal(checkVersions({ npm: '0.5.1', workspace: '0.5.1', desktop: '0.5.1', tauri: '0.5.1' }, 'cli-v0.1.0'), '0.5.1')
})
