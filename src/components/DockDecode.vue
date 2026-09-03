<script setup lang="ts">
// 纯占位空态：plan-parser-v1 落地后此处显示实时解码结果
// （解码列表届时自侧栏 ParserPanel 迁入，本组件无任何交互逻辑）。
</script>

<template>
  <div class="dock-decode-empty">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1" />
      <path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1" />
    </svg>
    <div class="dock-decode-title">实时解码</div>
    <div class="dock-decode-desc">
      解析器（plan-parser-v1）落地后此处显示实时解码结果
    </div>
  </div>
</template>
