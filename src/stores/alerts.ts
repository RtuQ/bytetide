import { defineStore } from 'pinia'
import { makeCodec } from '../persistence/schema'
import { loadStored, saveStored } from '../persistence/storage'
import type { AlertHit } from '../types'

const MAX_HITS = 100
const SOUND_KEY = 'serialtool.alertSound'

// 信封 codec：旧值为裸 '1'/'0' 字符串（JSON.parse 后是数字 1/0）。
const soundCodec = makeCodec<boolean>('alertSound', (raw) => {
  if (typeof raw === 'boolean') return raw
  if (raw === 1 || raw === '1') return true
  if (raw === 0 || raw === '0') return false
  throw new Error(`invalid alertSound: ${String(raw)}`)
})

/** 告警历史（内存环形）与全局声音开关；规则本体在 session store 的会话上 */
export const useAlertStore = defineStore('alerts', {
  state: () => ({
    hits: [] as AlertHit[],
    sound: false as boolean,
    _loaded: false,
  }),
  getters: {},
  actions: {
    load() {
      if (this._loaded) return
      this._loaded = true
      const r = loadStored(SOUND_KEY, soundCodec, false)
      this.sound = r.kind === 'ok' || r.kind === 'migrated' ? r.data : false
    },
    setSound(v: boolean) {
      this.sound = v
      saveStored(SOUND_KEY, soundCodec.schema, v)
    },
    push(hit: Omit<AlertHit, 'id'>) {
      const id = `a${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`
      // 新的在前，封顶丢弃最旧
      this.hits = [{ ...hit, id }, ...this.hits].slice(0, MAX_HITS)
    },
    clear() {
      this.hits = []
    },
  },
})
