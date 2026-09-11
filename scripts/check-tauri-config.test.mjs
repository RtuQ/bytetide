import test from 'node:test'
import assert from 'node:assert/strict'
import { validateAdditionalBrowserArgs } from './check-tauri-config.mjs'

const CORRECT_ARGS =
  '--disable-features=IntensiveWakeUpThrottling,CalculateNativeWinOcclusion --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows --disable-intensive-wake-up-throttling'

const REQUIRED_FLAGS = [
  '--disable-background-timer-throttling',
  '--disable-renderer-backgrounding',
  '--disable-backgrounding-occluded-windows',
  '--disable-intensive-wake-up-throttling',
]

test('accepts the corrected additionalBrowserArgs', () => {
  assert.doesNotThrow(() => validateAdditionalBrowserArgs(CORRECT_ARGS))
})

test('rejects a standalone flag merged into the --disable-features list', () => {
  const legacy =
    '--disable-features=IntensiveWakeUpThrottling,CalculateNativeWinOcclusion,--disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows --disable-intensive-wake-up-throttling'
  assert.throws(() => validateAdditionalBrowserArgs(legacy), /starts with --/)
})

test('rejects a minimal --disable-features list containing a flag', () => {
  assert.throws(() => validateAdditionalBrowserArgs('--disable-features=A,--b'), /starts with --/)
})

test('each required background flag must appear exactly once (missing throws)', () => {
  for (const flag of REQUIRED_FLAGS) {
    const without = CORRECT_ARGS.replace(flag, '').replace(/\s+/g, ' ').trim()
    assert.throws(() => validateAdditionalBrowserArgs(without), new RegExp(`flag ${flag}[\\s\\S]*found 0`))
  }
})

test('each required background flag must appear exactly once (duplicated throws)', () => {
  for (const flag of REQUIRED_FLAGS) {
    const duplicated = `${CORRECT_ARGS} ${flag}`
    assert.throws(() => validateAdditionalBrowserArgs(duplicated), new RegExp(`flag ${flag}[\\s\\S]*found 2`))
  }
})

test('rejects an empty value', () => {
  assert.throws(() => validateAdditionalBrowserArgs(''), /empty or missing/)
  assert.throws(() => validateAdditionalBrowserArgs('   '), /empty or missing/)
})

test('rejects a missing value', () => {
  assert.throws(() => validateAdditionalBrowserArgs(undefined), /empty or missing/)
})
