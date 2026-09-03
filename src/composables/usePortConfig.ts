import { ref } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { useSessionStore } from '../stores/session'
import { feedParser } from './useParserEngine'
import type { PortConfig } from '../types'

/** PortBar 退役后的共享挂点（布局重构 V1）：上次连接参数记忆 + 打开离线日志。
 *  模块级单例——TitleBar（设置弹层）与 TabBar（新建连接/打开日志）共用同一份 cfg。 */

const DEFAULT_CFG: PortConfig = {
  name: '',
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',
  stopBits: '1',
  flowControl: 'none',
}
const STORAGE_KEY = 'serialtool.lastPortConfig'

function loadCfg(): PortConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (raw) return { ...DEFAULT_CFG, ...(JSON.parse(raw) as Partial<PortConfig>) }
  } catch {
    /* ignore */
  }
  return { ...DEFAULT_CFG }
}

function persistCfg(c: PortConfig) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(c))
  } catch {
    /* ignore */
  }
}

/** 载入并补齐网络源字段（旧存档无这些键） */
function normalizedCfg(): PortConfig {
  const c = loadCfg()
  return {
    ...c,
    transport: (c.transport as PortConfig['transport']) ?? 'serial',
    tcpHost: c.tcpHost ?? '',
    tcpPort: c.tcpPort ?? null,
    udpLocalPort: c.udpLocalPort ?? null,
  }
}

const cfg = ref<PortConfig>(normalizedCfg())

export function usePortCfg() {
  return {
    cfg,
    /** 连接成功后记忆当前参数 */
    saveCfg: () => persistCfg(cfg.value),
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
      // 离线会话不走拉取循环（行经 appendLines 一次入表），解码引擎在此喂数
      const s = store.sessions[id]
      if (s) feedParser(id, s.lines)
    } catch (e: unknown) {
      alert(String(e instanceof Error ? e.message : e))
    } finally {
      opening.value = false
    }
  }
  return { opening, openLog }
}
