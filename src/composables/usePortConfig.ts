import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { useSessionStore } from '../stores/session'
import { feedParser } from './useParserEngine'
import { makeCodec } from '../persistence/schema'
import { loadValue, saveStored } from '../persistence/storage'
import type { PortConfig } from '../types'

/** PortBar 退役后的共享挂点（布局重构 V1）：上次连接参数记忆 + 打开离线日志。
 *  模块级单例——TitleBar（设置弹层）与 TabBar（新建连接/打开日志）共用同一份 cfg。
 *  持久化走 src/persistence（v1 信封 + 旧裸 JSON 自动迁移回填网络源字段）。 */

const DEFAULT_CFG: PortConfig = {
  name: '',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
}
const STORAGE_KEY = 'serialtool.lastPortConfig'

const cfgCodec = makeCodec<PortConfig>(
  'lastPortConfig',
  (raw) => {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('invalid port cfg')
    // 旧存档缺网络源字段：合并默认后补齐（transport/tcpHost/tcpPort/udpLocalPort）
    const c = { ...DEFAULT_CFG, ...(raw as Partial<PortConfig>) }
    return {
      ...c,
      transport: (c.transport as PortConfig['transport']) ?? 'serial',
      tcpHost: c.tcpHost ?? '',
      tcpPort: c.tcpPort ?? null,
      udpLocalPort: c.udpLocalPort ?? null,
    }
  },
)

const cfg = ref<PortConfig>(loadValue(STORAGE_KEY, cfgCodec, { ...DEFAULT_CFG }))

export function usePortCfg() {
  return {
    cfg,
    /** 连接成功后记忆当前参数 */
    saveCfg: () => saveStored(STORAGE_KEY, cfgCodec.schema, cfg.value),
    /** 从预设回填到待连接表单 */
    applyPreset: (c: PortConfig) => {
      cfg.value = { ...c }
    },
  }
}

// 打开日志文件离线分析：对话框选文件 -> 后端读取 -> 解析载入为离线标签页
const opening = ref(false)
export function useOpenLog() {
  async function openLog() {
    if (opening.value) return
    opening.value = true
    try {
      const sel = await open({
        multiple: false,
        filters: [{ name: 'Log', extensions: ['log', 'txt', 'tsv', 'csv'] }],
      })
      const path = typeof sel === 'string' ? sel : Array.isArray(sel) ? sel[0] : null
      if (!path) return
      const store = useSessionStore()
      const id = await store.loadOfflineSession(path)
      // 离线会话不走拉取循环（初始尾窗经 appendPulled 一次入表），解码引擎在此喂数：
      // 只喂最近 2000 行（Task 8——行本身经 offline_lines_after_cmd 分页取得，
      // 不再 readTextFile 全文；对齐引擎 setEnabled 回溯的 2000 行惯例）
      const s = store.sessions[id]
      if (s) feedParser(id, s.lines.slice(-2000))
    } catch (e: unknown) {
      alert(String(e instanceof Error ? e.message : e))
    } finally {
      opening.value = false
    }
  }
  return { opening, openLog }
}
