import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'

// 组件交互测试（Stage 3 Task 4）：jsdom + @vue/test-utils 的独立 project。
// 主 `npm test`（vitest.config.ts，node 环境）跑纯逻辑单测；本 config 只跑
// src/components/__tests__/ 的组件挂载测试。组件测试文件另带
// `@vitest-environment jsdom` docblock——即使被主 config 的 include 通配扫到，
// 也保证在 jsdom 下运行（双保险，主 config 无需感知组件测试的存在）。
export default defineConfig({
  plugins: [vue()],
  test: {
    environment: 'jsdom',
    include: ['src/components/__tests__/*.test.ts'],
  },
})
