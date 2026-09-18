/**
 * 自适应拉取节奏的纯控制律（活动感知两档迟滞）。
 *
 * 拉取循环（useTauriEvents）按会话维护节奏状态，每次 drain 结束把
 * PullOutcome 喂给 nextPullInterval 得到下一拍间隔：
 * - 拉到新行 / 翻页拉满仍落后 → 立即贴地板（活跃期上屏延迟 ≤25ms）
 * - 连续空拉超过宽限拍数 → 乘性爬回天花板（空闲节奏与旧固定 200ms 持平）
 *
 * 天花板刻意不高于旧固定值：静默后发一条命令等回显正是空闲态，放松空闲期
 * 会让「第一行」比固定 200ms 时代更慢。自适应只收紧活跃期，不放松空闲期。
 */

/** 活跃期地板：串口交互最坏 25ms 上屏（约一帧 40fps）；空拉在后端是一次
 *  读锁+二分（微秒级），40 次/秒的 IPC 开销可忽略。16–33ms 均可行，取余量较稳值 */
export const PULL_FLOOR_MS = 25
/** 空闲期天花板 = 旧固定拉取间隔，静默期行为零回归 */
export const PULL_CEIL_MS = 200
/** 空拉宽限拍数：地板采样下行间空隙 ≤75ms 都按「仍活跃」处理，防稀疏流横跳 */
export const PULL_GRACE_TICKS = 3
/** 空闲爬升因子：25→45→81→146→200，约 5 拍（~0.5s）回天花板 */
export const PULL_GROW = 1.8

/** 一次 drain 的结果（喂控制律的样本） */
export interface PullOutcome {
  /** 去重后实际入表的新行总数 */
  got: number
  /** 翻满 PULL_MAX_PAGES 仍整页返回（明确落后于 ring 产能） */
  capped: boolean
}

/** 每会话节奏状态；intervalMs 即下一拍间隔（初始=天花板，首拍有数据即贴地板） */
export interface PacingState {
  intervalMs: number
  idleStreak: number
}

export function initialPacing(): PacingState {
  return { intervalMs: PULL_CEIL_MS, idleStreak: 0 }
}

/** 控制律：就地更新 state 并返回下一拍间隔 */
export function nextPullInterval(state: PacingState, outcome: PullOutcome): number {
  if (outcome.got > 0 || outcome.capped) {
    state.idleStreak = 0
    state.intervalMs = PULL_FLOOR_MS
    return state.intervalMs
  }
  state.idleStreak += 1
  if (state.idleStreak <= PULL_GRACE_TICKS) return state.intervalMs
  state.intervalMs = Math.min(PULL_CEIL_MS, Math.round(state.intervalMs * PULL_GROW))
  return state.intervalMs
}
