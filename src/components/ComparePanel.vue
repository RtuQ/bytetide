<script setup lang="ts">
import { computed, ref } from 'vue'
import { useSessionStore } from '../stores/session'
import type { LogLine } from '../types'

const store = useSessionStore()

const aId = ref<string>('')
const bId = ref<string>('')
const tolMs = ref(50)
const dirScope = ref<'rx' | 'all'>('rx')

/** 显示上限：避免极端日志把面板撑爆 */
const SHOW_CAP = 300

interface Pair {
  a: LogLine | null
  b: LogLine | null
  /** null 表示该侧为孤立行 */
  delta: number | null
}

/** 时间近邻配对（贪心双指针，各自保持原顺序；b 过旧且无法再匹配时淘汰） */
function align(aArr: LogLine[], bArr: LogLine[], tol: number): Pair[] {
  const out: Pair[] = []
  let j = 0
  for (const a of aArr) {
    while (j < bArr.length && bArr[j]!.epochMillis < a.epochMillis - tol) {
      out.push({ a: null, b: bArr[j]!, delta: null })
      j += 1
    }
    if (j < bArr.length && Math.abs(bArr[j]!.epochMillis - a.epochMillis) <= tol) {
      let best = j
      // 在容差窗口内挑时间最近的一个 b
      let limit = Math.min(bArr.length, j + 8)
      for (let k = j; k < limit; k++) {
        if (
          Math.abs(bArr[k]!.epochMillis - a.epochMillis) <
          Math.abs(bArr[best]!.epochMillis - a.epochMillis)
        ) {
          best = k
        }
      }
      const bb = bArr[best]!
      out.push({ a, b: bb, delta: Math.abs(bb.epochMillis - a.epochMillis) })
      // 吞掉被跳过的 b 作为孤立行
      for (let k = j; k < best; k++) out.push({ a: null, b: bArr[k]!, delta: null })
      j = best + 1
    } else {
      out.push({ a, b: null, delta: null })
    }
  }
  return out
}

const scoped = (lines: LogLine[]) =>
  lines.filter((l) => (dirScope.value === 'rx' ? l.dir === 'rx' : true))

const pairs = computed<Pair[]>(() => {
  const aS = store.sessions[aId.value]
  const bS = store.sessions[bId.value]
  if (!aS || !bS || aId.value === bId.value) return []
  return align(scoped(aS.lines), scoped(bS.lines), Math.max(0, Number(tolMs.value) || 0))
})
const shown = computed(() => pairs.value.slice(0, SHOW_CAP))
/** 按行号跳转用：点击侧与目标会话 */
function jump(side: 'a' | 'b', l: LogLine | null) {
  if (!l) return
  const id = side === 'a' ? aId.value : bId.value
  if (store.sessions[id]) {
    store.setActive(id)
    store.requestJump(id, l.no)
  }
}
/** 与对侧文本的公共前后缀修剪，中间差异段高亮 */
function diffSpans(text: string | undefined, other: string | undefined): { t: string; hl: boolean }[] {
  const s = text ?? ''
  const o = other ?? ''
  let p = 0
  const minLen = Math.min(s.length, o.length)
  while (p < minLen && s[p] === o[p]) p += 1
  let suf = 0
  while (suf < minLen - p && s[s.length - 1 - suf] === o[o.length - 1 - suf]) suf += 1
  const midEnd = s.length - suf
  const out: { t: string; hl: boolean }[] = []
  if (p > 0) out.push({ t: s.slice(0, p), hl: false })
  if (midEnd > p) out.push({ t: s.slice(p, midEnd), hl: true })
  if (suf > 0) out.push({ t: s.slice(midEnd), hl: false })
  return out.length ? out : [{ t: '', hl: false }]
}
const sessionsList = computed(() =>
  store.sessionList.map((s) => ({ id: s.id, name: s.config.name || s.id })),
)
</script>

<template>
  <details class="panel">
    <summary class="panel-head">
      <svg class="panel-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="5" width="9" height="14" rx="1"/><rect x="13" y="5" width="9" height="14" rx="1"/><path d="M11 12h2"/></svg>
      <span class="panel-title">对比</span>
      <span v-if="pairs.length" class="badge">{{ pairs.length }}</span>
      <svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>
    </summary>
    <div class="panel-body">
      <div class="cmp-ctl">
        <select class="select select-sm" v-model="aId" title="会话 A">
          <option value="" disabled>A 会话</option>
          <option v-for="s in sessionsList" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <select class="select select-sm" v-model="bId" title="会话 B">
          <option value="" disabled>B 会话</option>
          <option v-for="s in sessionsList" :key="s.id" :value="s.id">{{ s.name }}</option>
        </select>
        <label class="al-num-field">
          ±<input class="al-num" type="number" min="0" step="10" v-model.number="tolMs" />ms 容差
        </label>
        <div class="seg">
          <button class="seg-item" :class="{ active: dirScope === 'rx' }" @click="dirScope = 'rx'">RX</button>
          <button class="seg-item" :class="{ active: dirScope === 'all' }" @click="dirScope = 'all'">全部</button>
        </div>
      </div>

      <div v-if="!aId || !bId" class="cp-empty-msg">选择两个会话后按时间轴 ± 容差配对显示</div>
      <div v-else-if="aId === bId" class="cp-empty-msg">不能选择同一会话</div>
      <template v-else>
        <div class="cmp-row cmp-head">
          <span>#A</span><span>tA</span><span class="cmp-txt">文本 A</span>
          <span class="cmp-delta">Δ</span>
          <span>#B</span><span>tB</span><span class="cmp-txt">文本 B</span>
        </div>
        <div v-for="(pr, i) in shown" :key="i" class="cmp-row" :class="{ solo: !pr.a || !pr.b }">
          <span class="cmp-no" :class="{ clickable: !!pr.a }" @click="jump('a', pr.a)">{{ pr.a?.no ?? '·' }}</span>
          <span class="cmp-ts">{{ pr.a?.ts ?? '' }}</span>
          <span class="cmp-txt">
            <template v-if="pr.a"><span v-for="(s2, k) in diffSpans(pr.b?.text, pr.a.text)" :key="k" :class="{ 'cmp-hl': s2.hl }">{{ s2.t }}</span></template>
            <span v-else-if="!pr.b" class="dim">仅A</span>
          </span>
          <span class="cmp-delta" :class="{ hot: pr.delta != null && pr.delta > tolMs }">{{ pr.delta != null ? pr.delta + 'ms' : '—' }}</span>
          <span class="cmp-no" :class="{ clickable: !!pr.b }" @click="jump('b', pr.b)">{{ pr.b?.no ?? '·' }}</span>
          <span class="cmp-ts">{{ pr.b?.ts ?? '' }}</span>
          <span class="cmp-txt">
            <template v-if="pr.b"><span v-for="(s2, k) in diffSpans(pr.a?.text, pr.b.text)" :key="k" :class="{ 'cmp-hl': s2.hl }">{{ s2.t }}</span></template>
            <span v-else class="dim">仅B</span>
          </span>
        </div>
        <div v-if="pairs.length > SHOW_CAP" class="cp-empty-msg">
          已显示前 {{ SHOW_CAP }} 行，共 {{ pairs.length }} 对
        </div>
        <div v-if="!pairs.length" class="cp-empty-msg">容差内没有可配对的行</div>
      </template>
    </div>
  </details>
</template>
