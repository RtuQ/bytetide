import { shallowRef } from 'vue'

export type ToastKind = 'info' | 'success' | 'warning' | 'error'
export interface ToastItem {
  id: number
  message: string
  detail?: string
  kind: ToastKind
  /** 自动关闭时长（ms）；进度条渲染与暂停/恢复都用它 */
  duration: number
  /** 业务标记（如 'disconnect'）：重连成功时按 tag 收掉未过期的断开提示 */
  tag?: string
}

export const toasts = shallowRef<ToastItem[]>([])
let nextId = 1
/** 自动关闭计时记录：timeout 句柄 + 剩余时间（悬停暂停/恢复的记账）。 */
interface TimerRec {
  timeout: ReturnType<typeof setTimeout>
  remaining: number
  startedAt: number
}
const timers = new Map<number, TimerRec>()

/** 同屏上限：超出挤掉最旧（告警风暴/长跑防堆满） */
const TOASTS_CAP = 5
/** 同文案 3s 节流：session-error 这类可能连续触发的源防刷屏 */
const DEDUP_MS = 3000
let lastMessage = ''
let lastAt = 0

export function toast(
  message: string,
  kind: ToastKind = 'info',
  duration = 2800,
  detail?: string,
  tag?: string,
) {
  const now = Date.now()
  if (message === lastMessage && now - lastAt < DEDUP_MS) return -1
  lastMessage = message
  lastAt = now
  const list = toasts.value
  if (list.length >= TOASTS_CAP) {
    for (const item of list.slice(0, list.length - TOASTS_CAP + 1)) dismissToast(item.id)
  }
  const id = nextId++
  toasts.value = [...toasts.value, { id, message, detail, kind, duration, tag }]
  if (duration > 0) armTimer(id, duration)
  return id
}

function armTimer(id: number, ms: number) {
  timers.set(id, {
    timeout: setTimeout(() => dismissToast(id), ms),
    remaining: ms,
    startedAt: Date.now(),
  })
}

/** 悬停暂停倒计时：停 timeout、记剩余；进度条的暂停由 ToastHost 同步做（WAAPI） */
export function pauseToast(id: number) {
  const rec = timers.get(id)
  if (!rec) return
  clearTimeout(rec.timeout)
  rec.remaining -= Date.now() - rec.startedAt
}

/** 移出恢复倒计时（剩余时间下限 200ms，防 0ms 抖动） */
export function resumeToast(id: number) {
  const rec = timers.get(id)
  if (!rec) return
  rec.startedAt = Date.now()
  rec.timeout = setTimeout(() => dismissToast(id), Math.max(200, rec.remaining))
}

export function dismissToast(id: number) {
  const timer = timers.get(id)
  if (timer) clearTimeout(timer.timeout)
  timers.delete(id)
  toasts.value = toasts.value.filter((item) => item.id !== id)
}

/** 按 tag 收掉同标记的未过期通知（重连成功 → 收掉断开提示） */
export function dismissByTag(tag: string) {
  for (const item of toasts.value) {
    if (item.tag === tag) dismissToast(item.id)
  }
}

export function _resetToasts() {
  timers.forEach((timer) => clearTimeout(timer.timeout))
  timers.clear()
  toasts.value = []
  lastMessage = ''
  lastAt = 0
}

/** 连接报错 → 人话标题 + 建议动作。正常断连返回 null（由 disconnected 状态提示负责，避免误报）。 */
export function connectionErrorHint(message: string): { title: string; action: string } | null {
  const text = message.toLowerCase()
  if (text.includes('断开') || text.includes('disconnected')) {
    return null
  }
  if (
    text.includes('占用') ||
    text.includes('busy') ||
    text.includes('access is denied') ||
    text.includes('拒绝访问')
  ) {
    return { title: '端口可能已被占用', action: '请关闭其他串口工具后重试' }
  }
  if (text.includes('不存在') || text.includes('not found') || text.includes('no such file')) {
    return { title: '端口已不可用', action: '刷新端口列表后重新选择' }
  }
  if (text.includes('timeout') || text.includes('超时') || text.includes('连接失败')) {
    return { title: '连接没有建立', action: '检查设备、电缆或网络地址后重试' }
  }
  return { title: '连接失败', action: '检查参数后重试' }
}
