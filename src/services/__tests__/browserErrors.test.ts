import { describe, expect, it } from 'vitest';
import { isDeferredResizeObserverNotification } from '../browserErrors';

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
});
