/**
 * ResizeObserver 在同一帧内发生级联布局时会主动延后剩余通知，并通过 window.error
 * 报告一条诊断信息。它不代表 React 渲染失败，也不会破坏下一帧布局；不能升级成
 * SophoNote 的全屏致命错误。两种文案分别来自 Chromium/WebKit。
 */
export function isDeferredResizeObserverNotification(message: unknown): boolean {
  if (typeof message !== 'string') return false;
  return (
    message.startsWith('ResizeObserver loop completed with undelivered notifications') ||
    message === 'ResizeObserver loop limit exceeded'
  );
}
