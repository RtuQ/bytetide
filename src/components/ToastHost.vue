<script setup lang="ts">
import { dismissToast, toasts } from '../composables/useToast'
</script>

<template>
  <div class="toast-host" aria-live="polite" aria-atomic="false">
    <div v-for="item in toasts" :key="item.id" class="toast" :class="'toast-' + item.kind" role="status">
      <span class="toast-mark" aria-hidden="true"></span>
      <span class="toast-copy">
        <b>{{ item.message }}</b>
        <small v-if="item.detail">{{ item.detail }}</small>
      </span>
      <button class="toast-close" type="button" title="关闭通知" aria-label="关闭通知" @click="dismissToast(item.id)">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12" /></svg>
      </button>
    </div>
  </div>
</template>
