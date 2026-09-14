import { describe, expect, it, vi } from 'vitest';
import {
  isBenignTauriUnlistenRejection,
  isDeferredResizeObserverNotification,
  safeUnlisten,
  subscribeTauriListener,
} from '../browserErrors';

describe('browser error classification', () => {
  it('ignores only benign ResizeObserver delivery diagnostics', () => {
    expect(
      isDeferredResizeObserverNotification(
        'ResizeObserver loop completed with undelivered notifications.'
      )
    ).toBe(true);
    expect(isDeferredResizeObserverNotification('ResizeObserver loop limit exceeded')).toBe(true);
    expect(isDeferredResizeObserverNotification('Cannot read properties of undefined')).toBe(false);
    expect(isDeferredResizeObserverNotification(null)).toBe(false);
  });

  it('recognizes Tauri event unlisten IPC rejections from the injected user-script', () => {
    const reason = new Error('');
    reason.stack = [
      '@user-script:11:11:75',
      'unregisterListener@user-script:11:13:9',
      '_unlisten@http://localhost:1420/node_modules/.vite/deps/chunk-BLKRPNAV.js:27:61',
      '@http://localhost:1420/src/components/aistudio/ProjectChatPanel.tsx:356:25',
    ].join('\n');
    expect(isBenignTauriUnlistenRejection(reason)).toBe(true);
    expect(isBenignTauriUnlistenRejection(new Error('Cannot read properties of undefined'))).toBe(false);
    expect(isBenignTauriUnlistenRejection('disk full')).toBe(false);
  });
});

describe('safe Tauri unlisten', () => {
  it('swallows both thrown errors and rejected unlisten promises', async () => {
    expect(() => safeUnlisten(undefined)).not.toThrow();
    expect(() => safeUnlisten(() => { throw new Error('already gone'); })).not.toThrow();
    expect(() => safeUnlisten(() => Promise.reject(new Error('unregisterListener')))).not.toThrow();
    await Promise.resolve();
  });

  it('unlistens immediately if the effect cleaned up before listen resolved', async () => {
    const stop = vi.fn(() => Promise.reject(new Error('unregisterListener')));
    let resolveSubscribe: (value: () => unknown) => void = () => undefined;
    const subscribe = new Promise<() => unknown>((resolve) => {
      resolveSubscribe = resolve;
    });
    const cleanup = subscribeTauriListener(subscribe);
    cleanup();
    resolveSubscribe(stop);
    await Promise.resolve();
    expect(stop).toHaveBeenCalledTimes(1);
  });

  it('does not report subscribe errors after dispose', async () => {
    const onError = vi.fn();
    const cleanup = subscribeTauriListener(Promise.reject(new Error('listen failed')), onError);
    cleanup();
    await Promise.resolve();
    expect(onError).not.toHaveBeenCalled();
  });
});
