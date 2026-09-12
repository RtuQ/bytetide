import { KEYWORD_PALETTE } from '../../types'
import type {
  AlertRule,
  AutoReplyRule,
  CaptureCfg,
  FilterStage,
  Keyword,
} from '../../types'
import type { Session } from './model'

/**
 * 规则域纯函数（Task 6）：关键词高亮 / 过滤链 / 自动回复 / 告警 / 现场捕获的
 * 增删改逻辑。显式传入会话对象原地变更，不内部调用 useSessionStore；变更后的
 * 后端推送（pushLiveRules/_pushPlot）由门面编排。
 */

let kwSeq = 0
export function newKeywordId(): string {
  kwSeq += 1
  return `k${Date.now().toString(36)}${kwSeq}`
}

let ruleSeq = 0
export function newRuleId(): string {
  ruleSeq += 1
  return `r${Date.now().toString(36)}${ruleSeq}`
}

// ===================== 高亮关键词 =====================

export function addKeywordTo(s: Session): void {
  const color = KEYWORD_PALETTE[s.keywords.length % KEYWORD_PALETTE.length]
  s.keywords.push({
    id: newKeywordId(),
    pattern: '',
    color,
    useRegex: false,
    caseSensitive: false,
    wholeWord: false,
  })
}

export function updateKeywordIn(s: Session, kid: string, patch: Partial<Keyword>): void {
  const k = s.keywords.find((x) => x.id === kid)
  if (k) Object.assign(k, patch)
}

export function removeKeywordIn(s: Session, kid: string): void {
  s.keywords = s.keywords.filter((x) => x.id !== kid)
}

// ===================== 过滤链 =====================

/** 添加一级（默认 include 单行匹配） */
export function addFilterStageTo(s: Session): void {
  s.filters.push({
    id: newRuleId(),
    text: '',
    mode: 'include',
    dir: 'any',
    useRegex: false,
    caseSensitive: false,
    wholeWord: false,
    enabled: true,
  })
}

export function updateFilterStageIn(s: Session, fid: string, patch: Partial<FilterStage>): void {
  const f = s.filters.find((x) => x.id === fid)
  if (f) Object.assign(f, patch)
}

export function removeFilterStageIn(s: Session, fid: string): void {
  s.filters = s.filters.filter((x) => x.id !== fid)
}

export function clearFilterStagesOf(s: Session): void {
  s.filters = []
}

// ===================== 自动回复 =====================

export function setAutoReplyEnabledIn(s: Session, v: boolean): void {
  s.autoReply.enabled = v
}

export function addAutoReplyRuleTo(s: Session): void {
  s.autoReply.rules.push({
    id: newRuleId(),
    trigger: '',
    reply: '',
    useRegex: false,
    caseSensitive: false,
    wholeWord: false,
    appendNewline: false,
    replyMode: 'ascii',
    enabled: true,
  })
}

export function updateAutoReplyRuleIn(s: Session, rid: string, patch: Partial<AutoReplyRule>): void {
  const r = s.autoReply.rules.find((x) => x.id === rid)
  if (r) Object.assign(r, patch)
}

export function removeAutoReplyRuleIn(s: Session, rid: string): void {
  s.autoReply.rules = s.autoReply.rules.filter((x) => x.id !== rid)
}

// ===================== 告警 =====================

export function setAlertsEnabledIn(s: Session, v: boolean): void {
  s.alerts.enabled = v
}

export function addAlertRuleTo(s: Session): void {
  s.alerts.rules.push({
    id: newRuleId(),
    pattern: '',
    useRegex: false,
    caseSensitive: false,
    wholeWord: false,
    minCount: 1,
    windowSec: 0,
    cooldownSec: 30,
    level: 'warn',
    enabled: true,
  })
}

export function updateAlertRuleIn(s: Session, rid: string, patch: Partial<AlertRule>): void {
  const r = s.alerts.rules.find((x) => x.id === rid)
  if (r) Object.assign(r, patch)
}

export function removeAlertRuleIn(s: Session, rid: string): void {
  s.alerts.rules = s.alerts.rules.filter((x) => x.id !== rid)
}

// ===================== 后端推送负载 =====================

/** set_live_rules_cmd 的整包负载（拉模型：评估在后端读线程，变更/重连后整体覆盖） */
export function buildLiveRulesPayload(s: Session) {
  return {
    autoReply: { enabled: s.autoReply.enabled, rules: s.autoReply.rules },
    alerts: { enabled: s.alerts.enabled, rules: s.alerts.rules },
    capture: { ...s.capture },
  }
}

/** 更新现场捕获配置（会话级）：本地合并，推送由门面 pushLiveRules 负责 */
export function applyCapturePatch(s: Session, patch: Partial<CaptureCfg>): void {
  s.capture = { ...s.capture, ...patch }
}
