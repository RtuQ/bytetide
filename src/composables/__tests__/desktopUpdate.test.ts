import { beforeEach, describe, expect, it, vi } from 'vitest'

const ipc = vi.hoisted(() => ({
  info: vi.fn(), download: vi.fn(), install: vi.fn(), listen: vi.fn(), dispose: vi.fn(),
}))
vi.mock('../../ipc/commands', () => ({ commands: {
  desktopUpdateInfo: ipc.info, desktopUpdateDownload: ipc.download, desktopUpdateInstall: ipc.install,
} }))
vi.mock('../../ipc/events', () => ({ onDesktopUpdateProgress: ipc.listen }))

async function setup() {
  const { useUpdateChecker } = await import('../useUpdateChecker')
  const checker = useUpdateChecker()
  await checker.refreshSupport()
  checker.updateInfo.value = { tagName: 'v1.0.0', version: '1.0.0', notes: '', url: 'https://example.com' }
  checker.status.value = 'available'
  return checker
}

describe('explicit desktop update lifecycle', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.clearAllMocks()
    ipc.info.mockResolvedValue({ supported: true })
    ipc.listen.mockResolvedValue(ipc.dispose)
    ipc.download.mockResolvedValue({ version: '1.0.0' })
    ipc.install.mockResolvedValue(undefined)
  })

  it('subscribes before download, reports progress, prevents duplicate downloads and releases listener', async () => {
    let finish!: (value: { version: string }) => void
    ipc.download.mockImplementation(() => new Promise((resolve) => { finish = resolve }))
    const checker = await setup()
    const task = checker.downloadUpdate()
    expect(checker.status.value).toBe('downloading')
    await Promise.resolve()
    expect(ipc.listen.mock.invocationCallOrder[0]).toBeLessThan(ipc.download.mock.invocationCallOrder[0]!)
    ipc.listen.mock.calls[0]![0]({ downloaded: 25, total: 100 })
    expect(checker.progressPercent.value).toBe(25)
    ipc.listen.mock.calls[0]![0]({ downloaded: 50, total: null })
    expect(checker.progressPercent.value).toBeNull()
    await checker.downloadUpdate()
    expect(ipc.download).toHaveBeenCalledTimes(1)
    finish({ version: '1.0.0' })
    await task
    expect(checker.status.value).toBe('ready')
    expect(ipc.dispose).toHaveBeenCalledTimes(1)
    expect(ipc.install).not.toHaveBeenCalled()
    await checker.installUpdate(() => false)
    expect(checker.status.value).toBe('ready')
    expect(ipc.install).not.toHaveBeenCalled()
    await checker.installUpdate(() => true)
    expect(ipc.install).toHaveBeenCalledTimes(1)
  })

  it('does not start a download if progress registration fails', async () => {
    ipc.listen.mockRejectedValueOnce(new Error('listener unavailable'))
    const checker = await setup()
    await checker.downloadUpdate()
    expect(checker.status.value).toBe('error')
    expect(ipc.download).not.toHaveBeenCalled()
  })

  it('keeps download failures retryable and disposes the listener', async () => {
    ipc.download.mockRejectedValueOnce('desktop_update_failed|signature mismatch')
    const checker = await setup()
    await checker.downloadUpdate()
    expect(checker.status.value).toBe('error')
    expect(checker.errorAction.value).toBe('download')
    expect(checker.errorMsg.value).toContain('signature mismatch')
    expect(ipc.dispose).toHaveBeenCalledTimes(1)
    await checker.downloadUpdate()
    expect(checker.status.value).toBe('ready')
  })

  it('does not download if unsupported and does not install without staged bytes', async () => {
    ipc.info.mockResolvedValue({ supported: false })
    const checker = await setup()
    await checker.downloadUpdate()
    const confirmation = vi.fn(() => true)
    await checker.installUpdate(confirmation)
    expect(ipc.download).not.toHaveBeenCalled()
    expect(confirmation).not.toHaveBeenCalled()
    expect(ipc.install).not.toHaveBeenCalled()
  })

  it('filters CLI releases and paginates to the desktop release', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce({ ok: true, json: async () => Array.from({ length: 100 }, () => ({ tag_name: 'cli-v9.0.0', html_url: 'https://example.com' })) })
      .mockResolvedValueOnce({ ok: true, json: async () => [{ tag_name: 'v2.0.0', html_url: 'https://example.com' }] })
    vi.stubGlobal('fetch', fetchMock)
    try {
      const checker = await setup()
      await checker.checkNow(true)
      expect(checker.updateInfo.value?.tagName).toBe('v2.0.0')
      expect(fetchMock).toHaveBeenCalledTimes(2)
      expect(fetchMock.mock.calls[1]![0]).toContain('page=2')
      expect(ipc.download).not.toHaveBeenCalled()
      expect(ipc.install).not.toHaveBeenCalled()
    } finally { vi.unstubAllGlobals() }
  })

  it('retains staged update for a confirmed retry after install failure', async () => {
    ipc.install.mockRejectedValueOnce('desktop_update_failed|permission denied')
    const checker = await setup()
    await checker.downloadUpdate()
    await checker.installUpdate(() => true)
    expect(checker.status.value).toBe('error')
    expect(checker.errorAction.value).toBe('install')
    await checker.installUpdate(() => true)
    expect(ipc.install).toHaveBeenCalledTimes(2)
  })
})
