import { defineConfig, configDefaults } from 'vitest/config'

// 纯函数单测：composable 为纯 TS（无 DOM/Vue 运行时依赖），
// 用 node 环境即可；Node 18+ 自带 TextEncoder，无需 polyfill。
// 组件挂载测试（jsdom + @vitejs/plugin-vue + @vue/test-utils，Stage 3 Task 4）
// 走 vitest.components.config.ts 单独 project（`npm run test:components`），
// 本 node project 排除之——.vue 导入在本配置下无法解析。
export default defineConfig({
  test: {
    // 语言锁 zh-CN（jsdom navigator.language=en-US 会把跟随系统的默认判成英文，污染中文断言）
    setupFiles: ['./src/test/vitest-setup.ts'],
    environment: 'node',
    include: ['src/**/__tests__/**/*.test.ts'],
    exclude: [
      'src/components/__tests__/Scenario*.test.ts',
      ...configDefaults.exclude,
    ],
  },
})
