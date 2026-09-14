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

/**
 * Tauri 的 UnlistenFn 实际会返回 Promise。React StrictMode / HMR 卸载时
 * `plugin:event|unlisten` 常因 listener 已失效而拒绝，栈在注入的 user-script。
 * 这不是渲染失败，不能盖住整个工作区。
 */
export function isBenignTauriUnlistenRejection(reason: unknown): boolean {
  const text = reason instanceof Error
    ? `${reason.message}\n${reason.stack ?? ''}`
    : String(reason ?? '');
  return text.includes('unregisterListener') && text.includes('_unlisten');
}

/** Tauri UnlistenFn 类型是 `() => void`，实现却返回 invoke Promise。 */
export function safeUnlisten(unlisten: (() => unknown) | null | undefined): void {
  if (unlisten == null) return;
  try {
    void Promise.resolve(unlisten()).catch(() => undefined);
  } catch {
    /* HMR / StrictMode 下 listener 可能已经不存在 */
  }
}

/**
 * 订阅 Tauri 事件并给出可安全放进 useEffect cleanup 的卸载函数。
 * 先卸载再 resolve 时立刻 unlisten，避免泄漏；listen 失败交给可选回调，不抛到全局。
 */
export function subscribeTauriListener(
  subscribe: Promise<() => unknown>,
  onSubscribeError?: (error: unknown) => void,
): () => void {
  let disposed = false;
  let unlisten: (() => unknown) | undefined;
  void subscribe
    .then((stop) => {
      if (disposed) safeUnlisten(stop);
      else unlisten = stop;
    })
    .catch((error) => {
      if (!disposed) onSubscribeError?.(error);
    });
  return () => {
    disposed = true;
    safeUnlisten(unlisten);
  };
}
