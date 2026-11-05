/** @vitest-environment jsdom */
import { mount } from '@vue/test-utils'
import { ref } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => null }))
vi.mock('../../composables/usePortConfig', () => ({
  usePortCfg: () => ({ cfg: {}, applyPreset: vi.fn() }),
  useOpenLog: () => ({ openLog: vi.fn() }),
}))
const state = vi.hoisted(() => ({ install: vi.fn(), download: vi.fn(), confirm: vi.fn() }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: state.confirm }))
vi.mock('../../composables/useUpdateChecker', () => ({ useUpdateChecker: () => ({
  status: ref('ready'), updateInfo: ref(null), currentVersion: ref('1.0.0'), errorMsg: ref(''),
  supported: ref(true), downloaded: ref(100), total: ref(100), progressPercent: ref(100),
  errorAction: ref('install'), downloadUpdate: state.download, installUpdate: state.install,
  init: vi.fn(), checkNow: vi.fn(), dismiss: vi.fn(), openReleasePage: vi.fn(),
}) }))
import TitleBar from '../TitleBar.vue'

afterEach(() => vi.restoreAllMocks())

describe('TitleBar restart consent', () => {
  it('warns about stopping connections and saving recordings and forwards the explicit decision', async () => {
    const confirm = state.confirm.mockResolvedValue(false)
    state.install.mockImplementation(async (getConsent: () => Promise<boolean>) => getConsent())
    const wrapper = mount(TitleBar, { global: { stubs: { SettingsPopover: true } } })
    await wrapper.get('.titlebar-ver').trigger('click')
    expect(state.install).not.toHaveBeenCalled()
    const button = wrapper.findAll('button').find((button) => button.text() === '安装并重启')!
    await button.trigger('click')
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('所有连接'), expect.objectContaining({ kind: 'warning' }))
    expect(confirm).toHaveBeenCalledWith(expect.stringContaining('保存录制'), expect.any(Object))
    expect(await state.install.mock.results[0]!.value).toBe(false)
    confirm.mockResolvedValue(true)
    await button.trigger('click')
    expect(await state.install.mock.results[1]!.value).toBe(true)
    wrapper.unmount()
  })
})
