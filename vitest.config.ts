import { defineConfig } from 'vitest/config';

// NB-31：前端单测基座（AG-23 整改项 P1-3 的第一步）。
// 测试文件约定放 src/**/__tests__/*.test.ts。
// localStorage 不依赖 jsdom/Node 注入：Node ≥22.4 的实验性 localStorage 全局会挡住
// jsdom 的注入（vitest populateGlobal 跳过已存在的全局键），zustand persist 因此拿不到
// storage —— 由 vitest.setup.ts 统一垫内存实现，见该文件注释。
export default defineConfig({
  test: {
    environment: 'jsdom',
    setupFiles: ['./vitest.setup.ts'],
    include: ['src/**/__tests__/**/*.test.ts'],
  },
});
