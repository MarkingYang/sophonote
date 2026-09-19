import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { confirmDiscard } from '../confirmDiscard';
beforeEach(() => {
  HTMLDialogElement.prototype.showModal = vi.fn();
  HTMLDialogElement.prototype.close = vi.fn();
});
afterEach(() => document.querySelectorAll('dialog').forEach((d) => d.remove()));
it('waits for an explicit choice and defaults cancellation to keeping the draft', async () => {
  const result = confirmDiscard();
  const dialog = document.querySelector('dialog')!;
  expect(dialog.querySelector('button')!.autofocus).toBe(true);
  dialog.dispatchEvent(new Event('cancel', {cancelable:true}));
  await expect(result).resolves.toBe(false);
  expect(document.querySelector('dialog')).toBeNull();
});
it('only the discard button authorizes abandoning edits', async () => {
  const result = confirmDiscard();
  document.querySelectorAll<HTMLButtonElement>('dialog button')[1].click();
  await expect(result).resolves.toBe(true);
  expect(document.querySelector('dialog')).toBeNull();
});
