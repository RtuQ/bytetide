import { defineStore } from 'pinia'
import type { AlertHit } from '../types'

const MAX_HITS = 100
const SOUND_KEY = 'serialtool.alertSound'

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
      try {
        this.sound = localStorage.getItem(SOUND_KEY) === '1'
      } catch {
        /* ignore */
      }
    },
    setSound(v: boolean) {
      this.sound = v
      try {
        localStorage.setItem(SOUND_KEY, v ? '1' : '0')
      } catch {
        /* ignore */
      }
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
