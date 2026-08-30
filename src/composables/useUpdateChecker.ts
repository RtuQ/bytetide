import { ref } from 'vue'
import { getVersion } from '@tauri-apps/api/app'
import { openUrl } from '@tauri-apps/plugin-opener'

/** 更新检查的 GitHub 仓库；改动需同步 scripts/portable-README.txt 的主页链接 */
const UPDATE_REPO = 'RtuQ/bytetide'

const API_TIMEOUT_MS = 8000
const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000
const AUTO_CHECK_DELAY_MS = 5000
const LAST_CHECK_KEY = 'serialtool.update.lastCheck'
const DISMISSED_KEY = 'serialtool.update.dismissedVersion'

export type UpdateStatus =
  | 'idle' // 初始 / 已忽略
  | 'checking' // 检查中
  | 'available' // 有新版本
  | 'latest' // 已是最新
  | 'error' // 检查失败（网络 / 接口）
  | 'unconfigured' // 未配置更新仓库

export interface UpdateInfo {
  /** 原始 tag，如 'v0.2.0' */
  tagName: string
  /** 纯版本号，如 '0.2.0' */
  version: string
  /** Release 说明（纯文本展示） */
  notes: string
  /** Release 页面直链 */
  url: string
}

/** 去掉 tag 的 v 前缀：'v0.2.0' -> '0.2.0' */
export function normalizeTag(tag: string): string {
  return tag.replace(/^v/i, '')
}

/** 逐段数值比较（'0.10.0' > '0.9.0'），长度不齐按 0 补齐（'0.1' 与 '0.1.0' 等价） */
export function isNewer(current: string, latest: string): boolean {
  const a = normalizeTag(current).split('.').map(Number)
  const b = normalizeTag(latest).split('.').map(Number)
  const len = Math.max(a.length, b.length)
  for (let i = 0; i < len; i++) {
    const x = a[i] ?? 0
    const y = b[i] ?? 0
    if (x !== y) return y > x
  }
  return false
}

/** 距上次检查是否已到间隔（lastCheck 为 null 表示从未查过） */
export function isCheckDue(now: number, lastCheck: number | null, intervalMs: number): boolean {
  return lastCheck === null || now - lastCheck >= intervalMs
}

/** 该版本是否已被用户忽略 */
export function isDismissed(tagName: string, dismissedTag: string | null): boolean {
  return dismissedTag !== null && dismissedTag === tagName
}

/** 从 GitHub releases/latest 响应中提取所需字段，关键字段缺失视为无效 */
export function parseRelease(json: unknown): UpdateInfo | null {
  if (typeof json !== 'object' || json === null) return null
  const r = json as Record<string, unknown>
  if (typeof r.tag_name !== 'string' || typeof r.html_url !== 'string') return null
  return {
    tagName: r.tag_name,
    version: normalizeTag(r.tag_name),
    notes: typeof r.body === 'string' ? r.body : '',
    url: r.html_url,
  }
}

function readLastCheck(): number | null {
  try {
    const raw = localStorage.getItem(LAST_CHECK_KEY)
    if (raw === null) return null
    const n = Number(raw)
    return Number.isFinite(n) ? n : null
  } catch {
    /* ignore */
  }
  return null
}

function writeLastCheck(now: number): void {
  try {
    localStorage.setItem(LAST_CHECK_KEY, String(now))
  } catch {
    /* ignore */
  }
}

function readDismissedTag(): string | null {
  try {
    return localStorage.getItem(DISMISSED_KEY)
  } catch {
    /* ignore */
  }
  return null
}

/** 模块级单例状态（跨组件共享） */
const status = ref<UpdateStatus>('idle')
const updateInfo = ref<UpdateInfo | null>(null)
const currentVersion = ref('')
const errorMsg = ref('')
let inited = false

/**
 * 检查更新。force=false 为静默检查（24h 节流、被忽略的版本不再提示）；
 * force=true 为手动检查（绕过节流，重新显示已忽略版本）。
 */
async function checkNow(force = false): Promise<void> {
  if (status.value === 'checking') return
  if (!force && !isCheckDue(Date.now(), readLastCheck(), CHECK_INTERVAL_MS)) return

  if (!UPDATE_REPO) {
    if (force) status.value = 'unconfigured'
    return
  }

  // 失败也记录间隔：避免离线时每次启动都干等超时
  writeLastCheck(Date.now())
  status.value = 'checking'
  errorMsg.value = ''
  try {
    const res = await fetch(`https://api.github.com/repos/${UPDATE_REPO}/releases/latest`, {
      headers: { Accept: 'application/vnd.github+json' },
      signal: AbortSignal.timeout(API_TIMEOUT_MS),
    })
    if (!res.ok) {
      throw new Error(res.status === 404 ? '仓库还没有发布版本' : `GitHub API 返回 ${res.status}`)
    }
    const info = parseRelease(await res.json())
    if (!info) throw new Error('更新响应格式异常')
    updateInfo.value = info
    if (!isNewer(currentVersion.value, info.version)) {
      status.value = 'latest'
    } else if (!force && isDismissed(info.tagName, readDismissedTag())) {
      status.value = 'idle'
    } else {
      status.value = 'available'
    }
  } catch (e) {
    status.value = 'error'
    errorMsg.value = e instanceof Error ? e.message : String(e)
  }
}

/** 应用启动时调用一次：读版本号，延迟数秒后静默检查（浏览器无后端则整体跳过） */
function init(): void {
  if (inited) return
  inited = true
  getVersion()
    .then(async (v) => {
      currentVersion.value = v
      await new Promise((r) => setTimeout(r, AUTO_CHECK_DELAY_MS))
      await checkNow(false)
    })
    .catch(() => {
      /* 非 Tauri 环境 */
    })
}

/** 忽略当前新版本：本版本不再主动提示（手动检查仍可见） */
function dismiss(): void {
  const info = updateInfo.value
  if (!info) return
  try {
    localStorage.setItem(DISMISSED_KEY, info.tagName)
  } catch {
    /* ignore */
  }
  if (status.value === 'available') status.value = 'idle'
}

/** 打开 Release 下载页（浏览器环境回退 window.open） */
async function openReleasePage(): Promise<void> {
  const url =
    updateInfo.value?.url ?? (UPDATE_REPO ? `https://github.com/${UPDATE_REPO}/releases/latest` : '')
  if (!url) return
  try {
    await openUrl(url)
  } catch {
    window.open(url, '_blank', 'noopener')
  }
}

export function useUpdateChecker() {
  return {
    status,
    updateInfo,
    currentVersion,
    errorMsg,
    init,
    checkNow,
    dismiss,
    openReleasePage,
  }
}
