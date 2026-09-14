<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import { commands } from '../ipc/commands'
import { toast } from '../composables/useToast'
import type { ReplayAction, ReplayState, ReplayView } from '../ipc/types'

/**
 * 回放控制工具条（Stage 3 Task 7）：仅 replay 会话渲染（由 LogView 挂载）。
 * 播放/暂停/停止 + 倍速 + 循环 + 行号水位 + 跳转滑块；控制经 ipc 命令适配层，
 * 返回的 ReplayView 与 replay-state 事件同载荷，谁先到都幂等落账（store 纪律）。
 * plan：事件仅在状态/控制变化时发——EOF/Error 等无控制命令的变化由 500ms
 * status 轮询兜底；seek 拖动中只本地更新目标行号，change（松手）才发 IPC 防洪水。
 */
const props = defineProps<{ sessionId: string }>()
const store = useSessionStore()
const session = computed(() => store.sessions[props.sessionId] ?? null)
const replay = computed(() => session.value?.replay ?? null)

const state = computed<ReplayState>(() => replay.value?.state ?? 'ready')
/** 终态（stopped/error）控制面失效；finished 仍可 seek 重播/调参 */
const dead = computed(() => state.value === 'stopped' || state.value === 'error')
const playing = computed(() => state.value === 'running')

const STATE_LABEL: Record<ReplayState, string> = {
  ready: '准备中',
  running: '回放中',
  paused: '已暂停',
  finished: '已播完',
  stopped: '已停止',
  error: '回放错误',
}

const SPEEDS = [0.1, 0.5, 1, 2, 5, 10, 50, 100]

const total = computed(() => session.value?.offlineLineCount ?? 0)
const current = computed(() => replay.value?.line ?? 0)

/** 命令返回的视图落账（与 replay-state 事件同载荷，幂等） */
function apply(view: ReplayView) {
  if (view && view.sessionId) store.setReplayView(view.sessionId, view)
}

async function control(action: ReplayAction, value?: number) {
  try {
    apply(await commands.replayControl(props.sessionId, action, value))
  } catch (e) {
    toast('回放控制失败', 'error', 4000, String(e))
  }
}

/** 播放/暂停/重播：paused→继续；finished→从头重播（seek 复活）；其余 running→暂停 */
function togglePlay() {
  if (dead.value) return
  if (state.value === 'paused') void control('resume')
  else if (state.value === 'finished') void control('seek', 1)
  else void control('pause')
}

function stop() {
  if (dead.value) return
  void control('stop')
}

function onSpeed(e: Event) {
  void control('speed', Number((e.target as HTMLSelectElement).value))
}

function onLoop(e: Event) {
  void control('loop', (e.target as HTMLInputElement).checked ? 1 : 0)
}

// ---- 跳转滑块：拖动中本地更新（显示目标行号），change/end 才发 IPC 防洪水 ----
const dragging = ref(false)
const dragValue = ref(1)
function onSeekInput(e: Event) {
  dragging.value = true
  dragValue.value = Number((e.target as HTMLInputElement).value)
}
function onSeekChange() {
  if (!dragging.value) return
  dragging.value = false
  const target = dragValue.value
  void control('seek', target)
}
const sliderValue = computed(() => (dragging.value ? dragValue.value : Math.max(current.value, 1)))
const sliderMax = computed(() => Math.max(total.value, 1))

// ---- status 轮询兜底：EOF/Error 等无控制命令的状态变化也可见（500ms） ----
let pollTimer: number | null = null
onMounted(() => {
  pollTimer = window.setInterval(async () => {
    try {
      apply(await commands.replayStatus(props.sessionId))
    } catch {
      /* 会话已移除（关闭标签）或无后端（浏览器冒烟）：静默 */
    }
  }, 500)
})
onBeforeUnmount(() => {
  if (pollTimer !== null) window.clearInterval(pollTimer)
})
</script>

<template>
  <div v-if="session" class="replay-bar" role="toolbar" aria-label="回放控制">
    <button
      class="btn btn-ghost btn-sm"
      :disabled="dead"
      :title="playing ? '暂停回放' : state === 'finished' ? '从头重播' : '继续回放'"
      :aria-label="playing ? '暂停回放' : '播放回放'"
      @click="togglePlay"
    >
      <svg
        v-if="playing"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      ><rect x="14" y="4" width="4" height="16" rx="1" /><rect x="6" y="4" width="4" height="16" rx="1" /></svg>
      <svg
        v-else
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      ><polygon points="6 3 20 12 6 21 6 3" /></svg>
      <span>{{ playing ? '暂停' : state === 'paused' ? '继续' : '播放' }}</span>
    </button>
    <button
      class="btn btn-ghost btn-sm"
      :disabled="dead"
      title="停止回放（已播行保留在视图中）"
      aria-label="停止回放"
      @click="stop"
    >
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      ><rect x="5" y="5" width="14" height="14" rx="2" /></svg>
      <span>停止</span>
    </button>
    <span class="replay-state" :class="`is-${state}`">{{ STATE_LABEL[state] }}</span>
    <label class="check" title="循环：到达文件尾后回到第 1 行继续回放">
      <input type="checkbox" :checked="replay?.looped ?? false" :disabled="dead" @change="onLoop" />
      <span class="box">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12" /></svg>
      </span>
      <span>循环</span>
    </label>
    <select
      class="select replay-speed"
      :value="replay?.speed ?? 1"
      :disabled="dead"
      title="回放倍速（缩放相邻行原始时间差）"
      aria-label="回放倍速"
      @change="onSpeed"
    >
      <option v-for="v in SPEEDS" :key="v" :value="v">{{ v }}×</option>
    </select>
    <input
      class="replay-seek"
      type="range"
      min="1"
      :max="sliderMax"
      :value="sliderValue"
      :disabled="dead || total === 0"
      aria-label="跳转到指定行"
      @input="onSeekInput"
      @change="onSeekChange"
    />
    <span class="replay-pos">
      {{ dragging ? `跳至 ${dragValue} 行` : `行 ${current} / ${total.toLocaleString()}` }}
    </span>
  </div>
</template>
