import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // 发布性能债治理：把稳定的重型依赖拆成独立 vendor chunk——
  // 入口只保留应用壳；编辑器栈与公式/高亮栈随懒加载页面按需进入，且各自独立缓存。
  build: {
    // Tauri WebView 从本地磁盘毫秒级载入，运行时 modulepreload 预加载无收益；
    // 关闭后 Vite 不再用 __vitePreload 包裹动态 import，避免该 helper 被 Rollup
    // 放进 vendor chunk、导致入口静态 import 重型 vendor 而懒加载失效。
    modulePreload: false,
    // 剩余 >500KB 的 chunk（vendor-editor/vendor-render/mermaid/cynefin）均为懒加载，
    // 不进启动路径，告警阈值相应放宽，保持构建日志干净。
    chunkSizeWarningLimit: 1500,
    rollupOptions: {
      output: {
        manualChunks(id: string) {
          // Vite 客户端构建会用 __vitePreload 包裹所有动态 import，该 helper 被
          // 含动态 import 的各 chunk 共享；Rollup 默认可能把它放进 vendor-editor，
          // 导致入口静态拉入 1.3MB 重型 vendor、懒加载失效。钉到 vendor-react（入口
          // 本就要静态引用）断开这条边。
          if (id.includes("preload-helper")) return "vendor-react";
          if (!id.includes("node_modules")) return undefined;
          // CSS 等非 JS 模块禁止进 vendor chunk：样式已在 main.tsx 入口静态引入，
          // 若 vendor chunk 携带 CSS，异步引入需要 __vitePreload 注入 CSS，
          // helper 会被 Rollup 放进 vendor chunk，入口随之静态 import 重型 vendor，懒加载失效。
          if (!/\.(js|mjs|cjs|jsx|ts|tsx)$/.test(id.split("?")[0])) return undefined;
          // 异步语言包：legacy-modes（~800KB）与 lang-* 由 language-data 动态加载，
          // 保持 Rollup 自然异步分块；若并入 vendor，语法树会被编辑器启动时全量拉入。
          if (/node_modules\/@codemirror\/(legacy-modes|lang-)/.test(id)) return undefined;
          if (/node_modules\/@lezer\//.test(id)) {
            // common/lr/markdown 是编辑器核心解析器；其余语法包随 lang-* 异步
            return /node_modules\/@lezer\/(common|lr|markdown)\//.test(id)
              ? "vendor-editor"
              : undefined;
          }
          // React 核心（react-dom/scheduler）：全局必需，单独长缓存
          if (/node_modules\/(react|react-dom|scheduler)\//.test(id)) return "vendor-react";
          // 文档编辑器栈：Milkdown/ProseMirror/CodeMirror，仅含编辑器的懒加载页面引用
          if (/node_modules\/(@milkdown|@codemirror|@floating-ui|prosemirror-)\//.test(id))
            return "vendor-editor";
          // 渲染重依赖：KaTeX 公式 + highlight.js 代码高亮，仅预览/详情路径引用
          if (/node_modules\/(katex|highlight\.js|lowlight)\//.test(id))
            return "vendor-render";
          return undefined;
        },
      },
    },
  },

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
