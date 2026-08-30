import { invoke } from '@tauri-apps/api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { watch } from 'vue'
import { useSessionStore, type Session } from '../stores/session'
import { useAlertStore } from '../stores/alerts'
import type { AlertHit } from '../types'

/** REST 桥侧栏数据推送：书签/告警历史是前端状态，后端桥只持有只读镜像，
 *  由本组合式在变化时全量推送（量小、变化低频，全量替换最简单且免对账）。 */

/** 行文本摘录截断长度（桥侧仅作引用定位，不需要整行） */
const CLIP = 200
/** 推送防抖毫秒（书签批量操作/告警风暴时合并为一次） */
const DEBOUNCE_MS = 300

/** 后端 BridgeBookmark（camelCase 对齐） */
interface BridgeBookmarkPayload {
  no: number
  ts: string
  text: string
}

/** 后端 BridgeAlert（camelCase 对齐；sessionName 冗余不推） */
interface BridgeAlertPayload {
  id: string
  ruleId: string
  pattern: string
  level: string
  no: number
  ts: string
  text: string
  at: number
}

function clip(s: string): string {
  return s.length > CLIP ? s.slice(0, CLIP) : s
}

/** 书签快照：UI 行号 + 行文本/时间戳（行被环形淘汰后 text 为空，仍保留行号） */
function bookmarkPayload(s: Session): BridgeBookmarkPayload[] {
  if (s.bookmarks.length === 0) return []
  const byNo = new Map(s.lines.map((l) => [l.no, l]))
  return s.bookmarks.map((no) => {
    const l = byNo.get(no)
    return { no, ts: l?.ts ?? '', text: l ? clip(l.text) : '' }
  })
}

function alertPayload(sessionId: string, hits: AlertHit[]): BridgeAlertPayload[] {
  return hits
    .filter((h) => h.sessionId === sessionId)
    .map((h) => ({
      id: h.id,
      ruleId: h.ruleId,
      pattern: h.pattern,
      level: h.level,
      no: h.no,
      ts: h.ts,
      text: clip(h.text),
      at: h.at,
    }))
}

/** 注册书签/告警 → 后端镜像的推送 watcher；返回停止函数列表（随 setupEvents 一并注销） */
export function setupBridgeSync(): UnlistenFn[] {
  const store = useSessionStore()
  const alerts = useAlertStore()
  const stops: UnlistenFn[] = []

  let timer: ReturnType<typeof setTimeout> | undefined
  const debounce = (fn: () => void) => {
    if (timer !== undefined) clearTimeout(timer)
    timer = setTimeout(fn, DEBOUNCE_MS)
  }

  const pushBookmarks = () => {
    for (const s of Object.values(store.sessions)) {
      invoke('bridge_sync_bookmarks_cmd', { sessionId: s.id, bookmarks: bookmarkPayload(s) }).catch(
        () => {},
      )
    }
  }
  const pushAlerts = () => {
    for (const id of Object.keys(store.sessions)) {
      invoke('bridge_sync_alerts_cmd', { sessionId: id, alerts: alertPayload(id, alerts.hits) }).catch(
        () => {},
      )
    }
  }

  // 书签经 toggleBookmark/clearBookmarks 原地 splice，须 deep 才能捕获
  stops.push(
    watch(
      () => Object.values(store.sessions).map((s) => ({ id: s.id, bookmarks: s.bookmarks })),
      () => debounce(pushBookmarks),
      { deep: true },
    ),
  )
  // 告警历史每次整体替换（this.hits = [...]），浅 watch 即可
  stops.push(
    watch(
      () => alerts.hits,
      () => debounce(pushAlerts),
    ),
  )

  return stops
}
