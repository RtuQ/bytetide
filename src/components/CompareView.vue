<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useSessionStore } from '../stores/session'
import {
  alignCompareLines,
  diffSpans,
  scopeCompareLines,
  type CompareDirScope,
  type ComparePair,
} from '../composables/useCompare'
import type { LogLine } from '../types'

const store = useSessionStore()

/** 会话选择/对齐设置存组件内（照搬 ComparePanel 的交互与状态来源） */
const aId = ref('')
const bId = ref('')
const tolMs = ref(50)
const dirScope = ref<CompareDirScope>('rx')

/** 显示上限：避免极端日志把表格撑爆（沿用 ComparePanel 的截断策略） */
const SHOW_CAP = 300

// 挂载预选：A=活动会话（无则第一个），B=另一个已打开会话；不足两个留空由空态引导
const initIds = store.sessionList.map((s) => s.id)
const initA = store.activeId && initIds.includes(store.activeId) ? store.activeId : initIds[0] ?? ''
aId.value = initA
bId.value = initIds.find((x) => x !== initA) ?? ''

// 会话关闭后清理失效选择，避免 select 残留死 id（pairs 计算另有守卫兜底）
watch(
  () => store.order.join('\n'),
  () => {
    if (aId.value && !(aId.value in store.sessions)) aId.value = ''
    if (bId.value && !(bId.value in store.sessions)) bId.value = ''
  },
)

const sessionsList = computed(() =>
  store.sessionList.map((s) => ({ id: s.id, name: s.config.name || s.id })),
)
const nameOf = (id: string) => store.sessions[id]?.config.name || id

/** 全量配对仅在两侧会话齐备时计算（守卫早退；本组件只在 compareMode 下挂载） */
const pairs = computed<ComparePair[]>(() => {
  const aS = store.sessions[aId.value]
  const bS = store.sessions[bId.value]
  if (!aS || !bS || aId.value === bId.value) return []
  const tol = Math.max(0, Number(tolMs.value) || 0)
  return alignCompareLines(
    scopeCompareLines(aS.lines, dirScope.value),
    scopeCompareLines(bS.lines, dirScope.value),
    tol,
  )
})
const shown = computed(() => pairs.value.slice(0, SHOW_CAP))

const emptyMsg = computed(() => {
  if (sessionsList.value.length < 2) return '需要至少两个打开的会话才能对比——先新建连接或打开另一个日志'
  if (!aId.value || !bId.value) return '选择会话 A 与 B，两路日志将按时间轴 ± 容差配对显示'
  if (aId.value === bId.value) return '会话 A 与 B 不能是同一个会话'
  if (!pairs.value.length) return '容差内没有可配对的行——可尝试增大容差或切换 RX / 全部'
  return ''
})

/** 按行号跳转到对应会话日志（沿用 ComparePanel 行为：切换活动会话并请求跳转） */
function jump(side: 'a' | 'b', l: LogLine | null) {
  if (!l) return
  const id = side === 'a' ? aId.value : bId.value
  if (store.sessions[id]) {
    store.setActive(id)
    store.requestJump(id, l.no)
  }
}
</script>

<template>
  <div class="cmp-view">
    <div class="cmp-v-head">
      <svg
        class="cmp-v-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <rect x="2" y="5" width="9" height="14" rx="1" /><rect x="13" y="5" width="9" height="14" rx="1" /><path d="M11 12h2" />
      </svg>
      <span class="cmp-v-title">对比</span>
      <div class="cmp-v-pick">
        <span class="cmp-v-pick-label">A</span>
        <select class="select cmp-v-select" v-model="aId" title="会话 A" aria-label="会话 A">
          <option value="" disabled>选择会话 A</option>
          <option v-for="s in sessionsList" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
      </div>
      <div class="cmp-v-pick">
        <span class="cmp-v-pick-label">B</span>
        <select class="select cmp-v-select" v-model="bId" title="会话 B" aria-label="会话 B">
          <option value="" disabled>选择会话 B</option>
          <option v-for="s in sessionsList" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
      </div>
      <label class="cmp-v-tol" title="配对时间容差（毫秒）">
        ±<input
          class="al-num"
          type="number"
          min="0"
          step="10"
          v-model.number="tolMs"
          aria-label="配对时间容差（毫秒）"
        />ms 容差
      </label>
      <div class="seg" role="group" aria-label="对比行范围">
        <button class="seg-item" :class="{ active: dirScope === 'rx' }" title="仅比对接收行" @click="dirScope = 'rx'">RX</button>
        <button class="seg-item" :class="{ active: dirScope === 'all' }" title="比对全部收发行" @click="dirScope = 'all'">全部</button>
      </div>
      <span class="cmp-v-hint">按 epochMillis 时间对齐</span>
    </div>

    <div v-if="emptyMsg" class="cmp-v-empty">
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <rect x="2" y="5" width="9" height="14" rx="1" /><rect x="13" y="5" width="9" height="14" rx="1" /><path d="M11 12h2" />
      </svg>
      <span>{{ emptyMsg }}</span>
    </div>

    <div v-else class="cmp-v-table">
      <table>
        <thead>
          <tr>
            <th class="cmp-v-th-time">时间 / Δ</th>
            <th class="cmp-v-th-side" :title="'会话 A：' + nameOf(aId)">A · {{ nameOf(aId) }}</th>
            <th class="cmp-v-th-side" :title="'会话 B：' + nameOf(bId)">B · {{ nameOf(bId) }}</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(pr, i) in shown" :key="i" :class="{ 'cmp-v-solo': !pr.a || !pr.b }">
            <td class="cmp-v-td-time">
              <span class="cmp-v-ts">{{ pr.a?.ts ?? pr.b?.ts ?? '' }}</span>
              <span class="cmp-v-delta" :class="{ hot: pr.delta != null && pr.delta > tolMs }">
                {{ pr.delta != null ? `Δ${pr.delta}ms` : '—' }}
              </span>
            </td>
            <td class="cmp-v-td-side">
              <div v-if="pr.a" class="cmp-v-cell">
                <button
                  class="cmp-v-jump"
                  :title="`跳转到 ${nameOf(aId)} 第 ${pr.a.no} 行`"
                  :aria-label="`跳转到 ${nameOf(aId)} 第 ${pr.a.no} 行`"
                  @click="jump('a', pr.a)"
                >#{{ pr.a.no }}</button>
                <span class="cmp-v-txt">
                  <span v-for="(s2, k) in diffSpans(pr.b?.text, pr.a.text)" :key="k" :class="{ 'cmp-v-hl': s2.hl }">{{ s2.t }}</span>
                </span>
              </div>
              <span v-else class="cmp-v-none" aria-hidden="true">—</span>
            </td>
            <td class="cmp-v-td-side">
              <div v-if="pr.b" class="cmp-v-cell">
                <button
                  class="cmp-v-jump"
                  :title="`跳转到 ${nameOf(bId)} 第 ${pr.b.no} 行`"
                  :aria-label="`跳转到 ${nameOf(bId)} 第 ${pr.b.no} 行`"
                  @click="jump('b', pr.b)"
                >#{{ pr.b.no }}</button>
                <span class="cmp-v-txt">
                  <span v-for="(s2, k) in diffSpans(pr.a?.text, pr.b.text)" :key="k" :class="{ 'cmp-v-hl': s2.hl }">{{ s2.t }}</span>
                </span>
              </div>
              <span v-else class="cmp-v-none" aria-hidden="true">—</span>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="pairs.length > SHOW_CAP" class="cmp-v-cap">
        已显示前 {{ SHOW_CAP }} 行，共 {{ pairs.length }} 对
      </div>
    </div>
  </div>
</template>
