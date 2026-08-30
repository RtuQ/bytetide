import { defineConfig } from 'vitest/config'

// 纯函数单测：composable 为纯 TS（无 DOM/Vue 运行时依赖），
// 用 node 环境即可；Node 18+ 自带 TextEncoder，无需 polyfill。
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/__tests__/**/*.test.ts'],
  },
})
