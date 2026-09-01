import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './index.css';
// 发布性能债治理：编辑器/公式/高亮的 CSS 统一在入口静态引入。
// 若留在懒加载组件里，Vite 会为异步 chunk 的 CSS 注入 __vitePreload helper，
// 导致入口静态 import 重型 vendor chunk，懒加载失效。CSS 体量小（~115KB）且全站通用。
import '@milkdown/crepe/theme/common/style.css';
import '@milkdown/crepe/theme/classic.css';
import 'katex/dist/katex.min.css';
import 'highlight.js/styles/github-dark.css';
import App from './App';
import { isDeferredResizeObserverNotification } from './services/browserErrors';

// 全局致命错误浮层：白屏时把真实错误显示出来，而不是无声失败
function showFatalError(message: string) {
  let overlay = document.getElementById('fatal-error-overlay');
  if (!overlay) {
    overlay = document.createElement('div');
    overlay.id = 'fatal-error-overlay';
    // 引导级兜底浮层：令牌优先，保留字面量 fallback（CSS 未加载时仍可读）
    overlay.style.cssText =
      'position:fixed;inset:0;z-index:99999;background:var(--danger-subtle,#fef2f2);color:var(--danger,#991b1b);padding:40px;font:13px/1.6 monospace;white-space:pre-wrap;overflow:auto;';
    document.body.appendChild(overlay);
  }
  overlay.textContent = `SophoNote 渲染出错\n\n${message}`;
}

window.addEventListener('error', (e) => {
  // Chromium/WebKit 会把 ResizeObserver 的「本帧通知延后」作为 window.error
  // 派发。布局会在下一帧正常继续，它不是应用崩溃，不能覆盖整个工作区。
  if (isDeferredResizeObserverNotification(e.message)) {
    e.preventDefault();
    return;
  }
  showFatalError(`${e.message}\n${e.filename}:${e.lineno}:${e.colno}\n${e.error?.stack || ''}`);
});
window.addEventListener('unhandledrejection', (e) => {
  const reason = e.reason;
  showFatalError(`Unhandled Promise rejection:\n${reason?.stack || reason?.message || String(reason)}`);
});

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>
);
