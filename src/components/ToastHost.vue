<script setup lang="ts">
import { dismissToast, pauseToast, resumeToast, toasts } from '../composables/useToast'
import type { ToastKind } from '../composables/useToast'

/** 每种语义一个 Lucide 图标（stroke=currentColor，随 --kind 变色） */
const KIND_ICONS: Record<ToastKind, string> = {
  info: '<circle cx="12" cy="12" r="10"/><path d="M12 16v-4"/><path d="M12 8h.01"/>',
  success: '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  warning:
    '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
  error: '<circle cx="12" cy="12" r="10"/><path d="m15 9-6 6"/><path d="m9 9 6 6"/>',
}

/** 悬停暂停倒计时：计时器（useToast）与进度条动画（WAAPI）同进退 */
function onEnter(id: number, ev: MouseEvent) {
  pauseToast(id)
  animationsOf(ev.currentTarget).forEach((a) => a.pause())
}
function onLeave(id: number, ev: MouseEvent) {
  resumeToast(id)
  animationsOf(ev.currentTarget).forEach((a) => a.play())
}
function animationsOf(root: EventTarget | null): Animation[] {
  const bar = (root as HTMLElement | null)?.querySelector('.toast-bar')
  return bar ? bar.getAnimations() : []
}
</script>

<template>
  <TransitionGroup tag="div" name="toast" class="toast-host" aria-live="polite" aria-atomic="false">
    <div
      v-for="item in toasts"
      :key="item.id"
      class="toast"
      :class="'toast-' + item.kind"
      role="status"
      @mouseenter="onEnter(item.id, $event)"
      @mouseleave="onLeave(item.id, $event)"
    >
      <span class="toast-chip" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" v-html="KIND_ICONS[item.kind]"></svg>
      </span>
      <span class="toast-copy">
        <b>{{ item.message }}</b>
        <small v-if="item.detail">{{ item.detail }}</small>
      </span>
      <button class="toast-close" type="button" title="关闭通知" aria-label="关闭通知" @click="dismissToast(item.id)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12" /></svg>
      </button>
      <span v-if="item.duration > 0" class="toast-bar" :style="{ animationDuration: item.duration + 'ms' }" aria-hidden="true"></span>
    </div>
  </TransitionGroup>
</template>
