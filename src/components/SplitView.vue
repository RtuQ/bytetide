<script setup lang="ts">
import { useSessionStore } from '../stores/session'
import SessionColumn from './SessionColumn.vue'

const store = useSessionStore()
</script>

<template>
  <div class="split-cols">
    <div v-for="(_, i) in store.columns" :key="i" class="split-col">
      <SessionColumn :column-index="i" />
    </div>
    <button
      v-if="store.columns.length < 4"
      class="split-add"
      title="增加一列"
      @click="store.addColumn()"
    >
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14"/><path d="M5 12h14"/></svg>
    </button>
  </div>
</template>
