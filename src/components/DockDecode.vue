<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useSessionStore } from '../stores/session'
import { useParserEngine } from '../composables/useParserEngine'
import type { DecodedFrame } from '../types/parser'

/** 底部 dock「解码」页签（plan-parser-v1 §3）：解码帧倒序列表（最新在上），
 *  行头 ts/类型 chip/RX-TX 徽标 + 定位日志原文按钮，点击行展开字段表；
 *  解析未启用时显示引导空态。 */
const store = useSessionStore()
const { ui } = useParserEngine()

const active = computed(() => store.active)
const decoded = computed(() => active.value?.decoded ?? [])
/** 倒序展示：最新帧在最上 */
const reversed = computed(() => [...decoded.value].reverse())

const expandedNo = ref<number | null>(null)
const follow = ref(true)
const listEl = ref<HTMLElement | null>(null)

// 新帧到达且开启跟随时回到顶部（列表倒序，最新在最上）
watch(
  () => decoded.value.length,
  async () => {
    if (!follow.value) return
    await nextTick()
    if (listEl.value) listEl.value.scrollTop = 0
  },
)

/** 类型名 hash → 6 色循环（.dd-c0..dd-c5，行左色条与类型 chip 同色） */
function colorClass(type: string): string {
  let h = 0
  for (let i = 0; i < type.length; i++) h = (h * 31 + type.charCodeAt(i)) >>> 0
  return `dd-c${h % 6}`
}

function toggleExpand(no: number) {
  expandedNo.value = expandedNo.value === no ? null : no
}

function jumpTo(d: DecodedFrame) {
  if (!active.value) return
  store.requestJump(active.value.id, d.no)
}

function clearDecoded() {
  if (active.value) store.resetDecoded(active.value.id)
}
</script>

<template>
  <!-- 未启用：引导空态 -->
  <div v-if="!ui.enabled" class="dock-decode-empty">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="m8 7-5 5 5 5m8-10 5 5-5 5M14 4l-4 16" />
    </svg>
    <div class="dock-decode-title">协议解析未启用</div>
    <div class="dock-decode-desc">
      在侧栏「协议解析」导入脚本并启用后，解码帧将实时显示在这里
    </div>
  </div>

  <!-- 已启用：倒序解码列表 -->
  <div v-else class="dd-wrap">
    <div class="dd-head">
      <span class="dd-title">
        {{ active ? `${active.config.name} · ${decoded.length} 帧` : `共 ${decoded.length} 帧` }}
      </span>
      <span class="dd-spacer"></span>
      <button class="dd-mini" :class="{ on: follow }" title="新帧到达时滚动到最新" @click="follow = !follow">
        跟随
      </button>
      <button class="dd-mini" :disabled="!active" title="清空当前会话的解码帧" @click="clearDecoded">
        清空
      </button>
    </div>
    <div ref="listEl" class="dd-list">
      <div v-if="!decoded.length" class="dd-none">暂无解码帧，等待 RX 数据…</div>
      <div
        v-for="d in reversed"
        :key="d.no"
        class="dd-row"
        :class="[colorClass(d.type), { open: expandedNo === d.no, 'dd-bad': d.crcOk === false }]"
        @click="toggleExpand(d.no)"
      >
        <div class="dd-row-head">
          <span class="dd-ts">{{ d.ts }}</span>
          <span class="dd-type">{{ d.type }}</span>
          <span class="dd-dir" :class="d.dir">{{ d.dir === 'rx' ? 'RX' : 'TX' }}</span>
          <span class="dd-spacer"></span>
          <button class="dd-jump" title="定位日志原文" aria-label="定位日志原文" @click.stop="jumpTo(d)">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="8" /><circle cx="12" cy="12" r="3" /><path d="M12 2v3" /><path d="M12 19v3" /><path d="M2 12h3" /><path d="M19 12h3" /></svg>
          </button>
        </div>
        <div class="dd-text">{{ d.text }}</div>
        <div v-if="d.warn" class="dd-warn">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z" /><path d="M12 9v4" /><path d="M12 17h.01" /></svg>
          <span>{{ d.warn }}</span>
        </div>
        <div v-if="expandedNo === d.no" class="dd-fields">
          <div v-for="(f, i) in d.fields ?? []" :key="i" class="dd-frow">
            <span class="dd-fl">{{ f.label }}</span>
            <span class="dd-fv">{{ f.value }}<span v-if="f.unit" class="dd-fu">{{ f.unit }}</span></span>
            <span v-if="f.raw" class="dd-fr">{{ f.raw }}</span>
          </div>
          <div class="dd-frame">{{ d.frameHex }} · {{ d.frameLen }}B</div>
        </div>
      </div>
    </div>
  </div>
</template>
