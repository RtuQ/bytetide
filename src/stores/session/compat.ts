import { defineStore } from 'pinia'
import { commands } from '../../ipc/commands'
import { ipcErrorDetail } from '../../ipc/errors'
import { parseLogFile } from '../../composables/useLogParser'
import { toast } from '../../composables/useToast'
import { openPath } from '@tauri-apps/plugin-opener'
import type { DecodedFrame } from '../../types/parser'
import type {
  AiAnnotation, AlertRule, AutoReplyRule, CaptureCfg, CaptureMeta, FilterStage, Keyword,
  LogConfig, LogLine, PlotConfig, PortConfig, PortInfo, PresetCategory, RawLogLine,
  SearchState, SendSequence,
} from '../../types'
import { createSession, type CenterView, type Session } from './model'
import { carrySessionForReconnect, clearSessionData } from './lifecycle'
import {
  dropPending, flushPendingTo, getActive, listSessions, recordError, recordStatus,
  registerSession, removeSession, replaceSession, type RegistryState,
} from './registry'
import {
  appendLinesInto, appendPulledInto, applyDecodedInto, prependBackfillInto, resetDecodedOf,
  takeBackfilledFrom, takeEvictedFrom, tallyBytesInto,
} from './log'
import {
  addAlertRuleTo, addAutoReplyRuleTo, addFilterStageTo, addKeywordTo, applyCapturePatch,
  buildLiveRulesPayload, removeAlertRuleIn, removeAutoReplyRuleIn, removeFilterStageIn,
  removeKeywordIn, setAlertsEnabledIn, setAutoReplyEnabledIn, updateAlertRuleIn,
  updateAutoReplyRuleIn, updateFilterStageIn, updateKeywordIn,
} from './rules'
import {
  addColumnTo, applyAdoptBridgePlot, applySetCenterView, applySetPlotEnabled, applyUpdatePlot,
  autoExitCompareAfterClose, enterSplit, exitSplit, removeBookmarkIn, removeColumnFrom,
  requestJumpIn, setColumnSessionIn, toggleBookmarkIn, toggleCompareMode, type LayoutState,
} from './view'
import {
  addPortPreset, applyConfigPresetToSession, insertConfigPreset, loadConfigPresets,
  loadLogConfig, loadPresets, loadSearchHistory, loadSendPresets, loadSendSequences,
  mergeConfigPresetImport, mergeLogConfig, pushSearchHistory, removeConfigPresetById,
  removePortPreset, removeSearchHistory, removeSendPresetById, removeSendSequenceById,
  renamePortPreset, runSequenceSteps, saveConfigPresets, saveLogConfig, savePresets,
  saveSearchHistory, saveSendPresets, saveSendSequences, updateSendHistory, upsertSendPreset,
  upsertSendSequence, type SeqRunState,
} from './presets'

export type { CenterView, Session } from './model'
export type { SeqRunState } from './presets'

/** 建账竞态缓冲与解析引擎清屏钩子的宿主见 registry.ts / registerParserOnClear */

/** 解析引擎的 clearLog 钩子（useParserEngine 注册）：清屏时复位 framer 并 gen+1，
 *  经注册注入避免 store→engine 循环依赖 */
let parserOnClear: ((id: string) => void) | null = null
export function registerParserOnClear(fn: (id: string) => void) {
  parserOnClear = fn
}

/** 当前运行的停止旗标（模块级：无需响应式，run 循环内轮询；停止走 presets.sleepInterruptible） */
let seqStopFlag: { stopped: boolean } | null = null

/**
 * useSessionStore（Task 6 兼容门面）：对外签名/响应式语义与拆分前完全一致，
 * 实现委托给 session/{model,lifecycle,registry,log,rules,view,presets} 纯函数
 * 模块；异步命令编排与建账竞态缓冲留在本文件。
 */
export const useSessionStore = defineStore('session', {
  state: () => ({
    sessions: {} as Record<string, Session>,
    order: [] as string[],
    activeId: null as string | null,
    ports: [] as PortInfo[],
    logConfig: loadLogConfig(),
    searchHistory: loadSearchHistory(),
    configPresets: loadConfigPresets(),
    presets: loadPresets(),
    sendPresets: loadSendPresets(),
    sendSequences: loadSendSequences(),
    seqRun: null as SeqRunState | null,
    /** 现场档案列表（sessions/captures；loadCaptures/capture-saved 事件刷新） */
    captures: [] as CaptureMeta[],
    /** 现场捕获 armed 状态（会话 id → 触发规则；capture-active 到达置位、capture-saved 解除） */
    captureActive: {} as Record<string, string>,
    splitMode: false,
    compareMode: false,
    columns: [] as (string | null)[],
  }),
  getters: {
    active(state): Session | null {
      return getActive(state as unknown as RegistryState)
    },
    sessionList(state): Session[] {
      return listSessions(state as unknown as RegistryState)
    },
  },
  actions: {
    setPorts(ports: PortInfo[]) {
      this.ports = ports
    },
    setLogConfig(patch: Partial<LogConfig>) {
      this.logConfig = mergeLogConfig(this.logConfig, patch)
      saveLogConfig(this.logConfig)
    },
    addPreset(name: string, config: PortConfig) {
      const next = addPortPreset(this.presets, name, config)
      if (!next) return
      this.presets = next
      savePresets(next)
    },
    removePreset(id: string) {
      this.presets = removePortPreset(this.presets, id)
      savePresets(this.presets)
    },
    renamePreset(id: string, name: string) {
      const next = renamePortPreset(this.presets, id, name)
      if (!next) return
      this.presets = next
      savePresets(next)
    },
    /** 保存快捷帧：带 id 为改名/改内容，否则新增（超出上限丢最旧） */
    saveSendPreset(input: { id?: string; name: string; payload: string; mode: 'ascii' | 'hex' }) {
      const next = upsertSendPreset(this.sendPresets, input)
      if (!next) return
      this.sendPresets = next
      saveSendPresets(next)
    },
    removeSendPreset(id: string) {
      this.sendPresets = removeSendPresetById(this.sendPresets, id)
      saveSendPresets(this.sendPresets)
    },
    /** 保存发送序列（整体覆盖同 id；intervalMs 钳制 ≥50ms 防定时器风暴） */
    saveSendSequence(seq: SendSequence) {
      const next = upsertSendSequence(this.sendSequences, seq)
      if (!next) return
      this.sendSequences = next
      saveSendSequences(next)
    },
    removeSendSequence(id: string) {
      this.sendSequences = removeSendSequenceById(this.sendSequences, id)
      saveSendSequences(this.sendSequences)
    },
    /** DTR/RTS 置位：仅 live 会话；网络源由后端报“无信号线”，错误抛给调用方提示 */
    async setSignal(id: string, pin: 'dtr' | 'rts', level: boolean) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      await commands.setSignal(id, pin, level)
    },
    /** 运行发送序列：步骤按序执行（发送/延时/信号），循环模式轮间隔后重复。
     *  断开、切换目标会话状态失效或 stopSequence 即中止；同一时刻仅一个序列 */
    async runSequence(id: string, seqId: string) {
      const seq = this.sendSequences.find((x) => x.id === seqId)
      if (!seq || this.seqRun) return
      const start = this.sessions[id]
      if (!start || start.kind !== 'live' || start.status !== 'connected') return
      const flag = { stopped: false }
      seqStopFlag = flag
      this.seqRun = { sessionId: id, seqId, round: 0, step: -1 }
      try {
        await runSequenceSteps(id, seq, this.seqRun, flag, {
          isAlive: () => {
            const cur = this.sessions[id]
            return !!cur && cur.kind === 'live' && cur.status === 'connected'
          },
          send: (sid, payload, mode) => this.send(sid, payload, mode),
          setSignal: (sid, pin, level) => this.setSignal(sid, pin, level),
        })
      } finally {
        if (seqStopFlag === flag) {
          seqStopFlag = null
          this.seqRun = null
        }
      }
    },
    stopSequence() {
      if (seqStopFlag) seqStopFlag.stopped = true
    },
    pushSearchHistory(pattern: string) {
      const next = pushSearchHistory(this.searchHistory, pattern)
      if (!next) return
      this.searchHistory = next
      saveSearchHistory(next)
    },
    removeSearchHistory(pattern: string) {
      this.searchHistory = removeSearchHistory(this.searchHistory, pattern)
      saveSearchHistory(this.searchHistory)
    },
    /** 进入分屏：用已打开会话填充前两列（不足则留空），列数 2~4 */
    enterSplit() {
      enterSplit(this as unknown as LayoutState, this.order)
    },
    exitSplit() {
      exitSplit(this as unknown as LayoutState)
    },
    /** 双会话时间对齐对比：全局布局态，占中心区（与 splitMode 同级）；
        会话不足 2 个时拒绝进入（退出不受限，供关闭会话后的自动退出兜底） */
    toggleCompareMode() {
      toggleCompareMode(this as unknown as LayoutState, this.order.length)
    },
    /** 设置某列绑定的会话；若该会话已在别列，则两列互换（避免同会话出现两次） */
    setColumnSession(i: number, id: string | null) {
      setColumnSessionIn(this as unknown as LayoutState, i, id)
    },
    /** 增加一列：优先填充未占用的已打开会话，最多 4 列 */
    addColumn() {
      addColumnTo(this as unknown as LayoutState, this.order)
    },
    /** 删除一列：至少保留 2 列 */
    removeColumn(i: number) {
      removeColumnFrom(this as unknown as LayoutState, i)
    },
    async refreshPorts() {
      try {
        this.ports = await commands.listPorts()
      } catch {
        this.ports = []
      }
    },
    async openTab(config: PortConfig) {
      const id = await commands.connect(config, this.logConfig)
      const s = createSession(id, config)
      registerSession(this as unknown as RegistryState, s)
      this.flushPending(id)
      this.pushLiveRules(id)
      return id
    },
    /** 建本地会话（不经 invoke/后端）：测试与无后端冒烟环境用。
     *  只创建前端态，不启动任何读线程；id 由调用方指定避免与后端 s{N}/o{N} 冲突。 */
    createLocalSession(id: string, config: PortConfig): string {
      if (this.sessions[id]) return id
      const s = createSession(id, config)
      s.status = 'offline'
      registerSession(this as unknown as RegistryState, s)
      this.flushPending(id)
      return id
    },
    /** 从日志文件离线载入：后端建 ring 会话（o{N}），前端仍灌 UI ring；REST 桥可见 */
    async loadOfflineSession(path: string) {
      const content = await commands.readTextFile(path)
      const parsed = parseLogFile(content)
      const baseName = path.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') || '离线日志'
      const config: PortConfig = {
        name: baseName,
        baudRate: 0,
        dataBits: 8,
        parity: 'none',
        stopBits: '1',
        flowControl: 'none',
      }
      const id = await commands.createOfflineSession(config, path, parsed.lines)
      const s = createSession(id, config)
      s.kind = 'offline'
      s.status = 'offline'
      registerSession(this as unknown as RegistryState, s)
      if (parsed.lines.length) this.appendLines(id, parsed.lines)
      return id
    },
    async closeTab(id: string) {
      try {
        await commands.disconnect(id)
      } catch {
        /* ignore */
      }
      removeSession(this as unknown as RegistryState, id)
      // 对比依赖 ≥2 会话：关到只剩一个时自动退出对比态，别把用户困在死界面里
      autoExitCompareAfterClose(this as unknown as LayoutState, this.order.length)
    },
    /** 断开串口但保留标签页与日志，便于稍后重连（区别于 closeTab 的彻底关闭） */
    async stopSession(id: string) {
      const s = this.sessions[id]
      if (!s) return
      if (s.kind === 'offline') return
      try {
        await commands.disconnect(id)
      } catch {
        /* ignore */
      }
      s.status = 'disconnected'
      s.error = ''
    },
    /** 用原配置重连：后端生成新会话 id，前端把原会话数据迁移到新 id 下 */
    async reconnectSession(id: string) {
      const s = this.sessions[id]
      if (!s) return
      const config = s.config
      let newId: string
      try {
        newId = await commands.connect(config, this.logConfig)
      } catch (e: unknown) {
        s.error = ipcErrorDetail(e)
        s.status = 'error'
        return
      }
      const carried = carrySessionForReconnect(s, newId)
      replaceSession(this as unknown as RegistryState, id, carried)
      // 旧 id 的积压事件已无意义，新 id 回放建账竞态期间的状态
      dropPending(id)
      this.flushPending(newId)
      this.pushLiveRules(newId)
      // 新后端会话连接时已默认开录制；沿用原会话的暂停状态
      if (!carried.recOn) this.setRec(newId, false)
    },
    async send(id: string, text: string, mode: 'ascii' | 'hex') {
      const s = this.sessions[id]
      if (!s) return
      await commands.send(id, mode, text)
      s.sendHistory = updateSendHistory(s.sendHistory, text)
    },
    async clearLog(id: string) {
      const s = this.sessions[id]
      if (!s) return
      clearSessionData(s)
      commands.syncAnnotations(id, []).catch(() => {})
      try {
        await commands.clearLog(id)
      } catch {
        /* ignore */
      }
      parserOnClear?.(id)
    },
    /** 推送实时规则到后端（拉模型：评估在读线程，规则变更/重连后整体覆盖） */
    pushLiveRules(id: string) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      commands.setLiveRules(id, buildLiveRulesPayload(s)).catch(() => {})
    },
    /** 更新现场捕获配置（会话级，重连迁移）：本地合并后整包推送后端读线程 */
    updateCapture(id: string, patch: Partial<CaptureCfg>) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      applyCapturePatch(s, patch)
      this.pushLiveRules(id)
    },
    /** 现场档案列表（全局，非会话级）：连接/收到 capture-saved 后刷新 */
    async loadCaptures() {
      try {
        this.captures = await commands.listCaptures()
      } catch {
        /* 浏览器冒烟无后端：静默 */
      }
    },
    async deleteCapture(path: string) {
      try {
        await commands.deleteCapture(path)
      } catch (e: unknown) {
        alert(ipcErrorDetail(e))
      }
      await this.loadCaptures()
    },
    /** 捕获 armed 状态置位/解除（useTauriEvents 两个捕获事件共用） */
    setCaptureActive(id: string, rule: string | null) {
      if (rule) this.captureActive[id] = rule
      else delete this.captureActive[id]
    },
    /** 「打开日志」：取当前分段路径并用系统默认程序打开；空路径/失败走 toast 报错 */
    async openLogPath(id: string) {
      let p = ''
      try {
        p = await commands.sessionLogPath(id)
      } catch (e) {
        toast('无法打开日志文件', 'error', 4000, String(e))
        return
      }
      if (!p) {
        toast('无法打开日志文件', 'error', 4000, '当前会话尚未生成日志文件')
        return
      }
      try {
        await openPath(p)
      } catch (e) {
        toast('无法打开日志文件', 'error', 4000, String(e))
      }
    },
    /** 落盘录制开关：关=暂停写日志文件（数据仍进日志视图）；开=另起新分段文件继续录制。
     *  本地乐观置位，后端失败回滚并提示 */
    async setRec(id: string, on: boolean) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      s.recOn = on
      try {
        await commands.setRecording(id, on)
      } catch (e) {
        s.recOn = !on
        alert(String(e))
      }
    },
    /** 日志分段：关闭当前文件，从当前时刻另起 `基准名-YYYYMMDD-HHMMSS.log`
     *  新文件继续录制（旧文件保留）；录制暂停中调用会顺带恢复录制 */
    async rotateLog(id: string) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      try {
        await commands.rotateLog(id)
      } catch (e) {
        alert(String(e))
      }
    },
    /** 追加日志并返回本次带行号的新行（供告警/回复等后续处理拿到 no）。
     *  行 markRaw 纪律见 log.ts；200ms 拉取编排留在 useTauriEvents。 */
    appendLines(id: string, raw: RawLogLine[]): LogLine[] {
      const s = this.sessions[id]
      if (!s || raw.length === 0) return []
      return appendLinesInto(s, raw, Math.max(1, this.logConfig.viewBufCap))
    },
    /** 拉模型摄取：后端 ring 按 `no` 游标拉到的行一次性入表（游标语义见 log.ts） */
    appendPulled(
      id: string,
      lines: (RawLogLine & { ringNo: number })[],
    ): LogLine[] {
      const s = this.sessions[id]
      if (!s || lines.length === 0) return []
      return appendPulledInto(s, lines, Math.max(1, this.logConfig.viewBufCap))
    },
    /** 视口锚定补偿：返回该会话累计的被裁行数并清零（未知会话返回 0） */
    takeEvicted(id: string): number {
      const s = this.sessions[id]
      if (!s) return 0
      return takeEvictedFrom(s)
    },
    /** 翻页补旧行（方案 B）：上滑时把仍在 ring 窗口内的被裁行回补到头部（语义见 log.ts） */
    prependBackfill(
      id: string,
      lines: (RawLogLine & { ringNo: number })[],
    ): LogLine[] {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live' || lines.length === 0) return []
      return prependBackfillInto(s, lines)
    },
    /** 视口锚定补偿（头部插入方向）：返回本批回补的行并清空暂存（未知会话返回 []） */
    takeBackfilled(id: string): LogLine[] {
      const s = this.sessions[id]
      if (!s) return []
      return takeBackfilledFrom(s)
    },
    /** 解析引擎落表：解码帧追加（markRaw + FIFO；replace=true 回溯整表替换） */
    applyDecoded(id: string, frames: DecodedFrame[], replace = false) {
      const s = this.sessions[id]
      if (!s) return
      applyDecodedInto(s, frames, replace)
    },
    /** 清空解码帧（卸载/停用回溯前重置） */
    resetDecoded(id: string) {
      const s = this.sessions[id]
      if (s) resetDecodedOf(s)
    },
    /** 累计 RX/TX 字节与行数（lifetime，随缓冲裁剪不回退）；与 appendLines 分离 */
    tallyBytes(id: string, raw: RawLogLine[]) {
      const s = this.sessions[id]
      if (!s) return
      tallyBytesInto(s, raw)
    },
    setStatus(id: string, status: string) {
      recordStatus(this as unknown as RegistryState, id, status)
    },
    setError(id: string, error: string) {
      recordError(this as unknown as RegistryState, id, error)
    },
    /** 会话落账后回放竞态期间积压的连接状态/错误（先状态后错误，error 优先） */
    flushPending(id: string) {
      flushPendingTo(this as unknown as RegistryState, id)
    },
    setActive(id: string) {
      this.activeId = id
    },
    updateSearch(id: string, patch: Partial<SearchState>) {
      const s = this.sessions[id]
      if (s) s.search = { ...s.search, ...patch }
    },
    addKeyword(id: string) {
      const s = this.sessions[id]
      if (!s) return
      addKeywordTo(s)
    },
    updateKeyword(id: string, kid: string, patch: Partial<Keyword>) {
      const s = this.sessions[id]
      if (!s) return
      updateKeywordIn(s, kid, patch)
    },
    removeKeyword(id: string, kid: string) {
      const s = this.sessions[id]
      if (!s) return
      removeKeywordIn(s, kid)
    },
    setFollowTail(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.followTail = v
    },
    setOnlyMatches(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.onlyMatches = v
    },
    setHexView(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.hexView = v
    },
    setShowDelta(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showDelta = v
    },
    setShowLineNo(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showLineNo = v
    },
    setShowDir(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) s.showDir = v
    },
    /** 切换书签：存在则移除，否则按行号升序插入 */
    toggleBookmark(id: string, no: number) {
      const s = this.sessions[id]
      if (!s) return
      toggleBookmarkIn(s, no)
    },
    removeBookmark(id: string, no: number) {
      const s = this.sessions[id]
      if (!s) return
      removeBookmarkIn(s, no)
    },
    clearBookmarks(id: string) {
      const s = this.sessions[id]
      if (s) s.bookmarks = []
    },
    /** 开启绘图：同时强制 HEX 视图；视图耦合（布局重构 V1）语义见 view.ts。
     *  不变式：centerView !== 'log' ⟹ plot.enabled */
    setPlotEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (!s) return
      applySetPlotEnabled(s, v)
      this._pushPlot(id)
    },
    /** 切换中心区视图模式；进入 split/plot 时若图表未启用则顺带启用（一次点击即出图） */
    setCenterView(id: string, view: CenterView) {
      const s = this.sessions[id]
      if (!s) return
      if (applySetCenterView(s, view)) this._pushPlot(id)
    },
    /** 更新绘图解析配置（帧头/帧尾/校验/通道等），enabled 经 setPlotEnabled 单独控制 */
    updatePlot(id: string, patch: Partial<PlotConfig>) {
      const s = this.sessions[id]
      if (!s) return
      applyUpdatePlot(s, patch)
      this._pushPlot(id)
    },
    /** 仅 live 会话：把绘图配置同步到后端供 REST `/decode` 复用；失败静默，不打断前端绘图 */
    _pushPlot(id: string) {
      const s = this.sessions[id]
      if (!s || s.kind !== 'live') return
      commands.setPlotConfig(id, { ...s.plot }).catch(() => {})
    },
    /** 采纳 REST 桥写回的绘图文法（bridge-annotations-updated 同源通道）：整包替换本地状态。
     *  后端 manager 已持有该配置，无需回推 _pushPlot；缺省字段用默认值回填。 */
    adoptBridgePlot(id: string, cfg: PlotConfig) {
      const s = this.sessions[id]
      if (!s) return
      applyAdoptBridgePlot(s, cfg)
    },
    /** 采纳 AI 批注（bridge-annotations-updated 事件）：整包替换 */
    applyBridgeAnnotations(id: string, notes: AiAnnotation[]) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = notes
    },
    /** 删除单条 AI 批注并同步后端镜像 */
    removeAiNote(id: string, noteId: string) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = s.aiNotes.filter((n) => n.id !== noteId)
      this._pushAiNotes(id)
    },
    /** 清空 AI 批注并同步后端镜像 */
    clearAiNotes(id: string) {
      const s = this.sessions[id]
      if (!s) return
      s.aiNotes = []
      this._pushAiNotes(id)
    },
    /** 前端 → 后端镜像的整包回写（仅删除/清空方向会用到；AI 写入方向由 bridge.rs 推送） */
    _pushAiNotes(id: string) {
      const s = this.sessions[id]
      if (!s) return
      commands.syncAnnotations(id, [...s.aiNotes]).catch(() => {})
    },
    setAutoReplyEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) setAutoReplyEnabledIn(s, v)
      this.pushLiveRules(id)
    },
    addAutoReplyRule(id: string) {
      const s = this.sessions[id]
      if (!s) return
      addAutoReplyRuleTo(s)
      this.pushLiveRules(id)
    },
    updateAutoReplyRule(id: string, rid: string, patch: Partial<AutoReplyRule>) {
      const s = this.sessions[id]
      if (!s) return
      updateAutoReplyRuleIn(s, rid, patch)
      this.pushLiveRules(id)
    },
    removeAutoReplyRule(id: string, rid: string) {
      const s = this.sessions[id]
      if (!s) return
      removeAutoReplyRuleIn(s, rid)
      this.pushLiveRules(id)
    },
    setAlertsEnabled(id: string, v: boolean) {
      const s = this.sessions[id]
      if (s) setAlertsEnabledIn(s, v)
      this.pushLiveRules(id)
    },
    addAlertRule(id: string) {
      const s = this.sessions[id]
      if (!s) return
      addAlertRuleTo(s)
      this.pushLiveRules(id)
    },
    updateAlertRule(id: string, rid: string, patch: Partial<AlertRule>) {
      const s = this.sessions[id]
      if (!s) return
      updateAlertRuleIn(s, rid, patch)
      this.pushLiveRules(id)
    },
    removeAlertRule(id: string, rid: string) {
      const s = this.sessions[id]
      if (!s) return
      removeAlertRuleIn(s, rid)
      this.pushLiveRules(id)
    },
    /** 过滤链：添加一级（默认 include 单行匹配） */
    addFilterStage(id: string) {
      const s = this.sessions[id]
      if (!s) return
      addFilterStageTo(s)
    },
    updateFilterStage(id: string, fid: string, patch: Partial<FilterStage>) {
      const s = this.sessions[id]
      if (!s) return
      updateFilterStageIn(s, fid, patch)
    },
    removeFilterStage(id: string, fid: string) {
      const s = this.sessions[id]
      if (!s) return
      removeFilterStageIn(s, fid)
    },
    clearFilterStages(id: string) {
      const s = this.sessions[id]
      if (s) s.filters = []
    },
    /** 保存命名预设到库（data 形状由调用方保证），同类超过上限丢弃最旧 */
    saveConfigPreset(category: PresetCategory, name: string, data: unknown) {
      this.configPresets = insertConfigPreset(this.configPresets, category, name, data)
      saveConfigPresets(this.configPresets)
    },
    removeConfigPreset(pid: string) {
      this.configPresets = removeConfigPresetById(this.configPresets, pid)
      saveConfigPresets(this.configPresets)
    },
    /**
     * 套用预设到当前活动会话：按类别写入对应配置。
     * data 来自导入文件时可能畸形，各分支做最小形状校验后再落地。
     */
    applyConfigPreset(pid: string): boolean {
      const p = this.configPresets.find((x) => x.id === pid)
      const activeId = this.activeId
      const s = activeId ? this.sessions[activeId] : null
      if (!p || !s || !activeId) return false
      const ok = applyConfigPresetToSession(s, p)
      if (ok && p.category === 'plots' && s.kind !== 'offline') this._pushPlot(activeId)
      return ok
    },
    /** 导入预设包 JSON（整库合并，id 冲突重生成）；返回导入条数 */
    importConfigPresets(raw: unknown): number {
      const { list, imported } = mergeConfigPresetImport(this.configPresets, raw)
      this.configPresets = list
      saveConfigPresets(list)
      return imported
    },
    /** 请求跳转到指定行号（jump 信号：no + token 防重放，LogView 监听消费） */
    requestJump(id: string, no: number) {
      const s = this.sessions[id]
      if (s) requestJumpIn(s, no)
    },
  },
})
