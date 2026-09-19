import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/confirmDiscard', () => ({confirmDiscard: vi.fn()}));
import { confirmDiscard } from '../../../services/confirmDiscard';
vi.mock('../../../services/tauri', () => ({ listOpenVikingMemories: vi.fn(), readOpenVikingMemory: vi.fn(), writeOpenVikingMemory: vi.fn() }));
import { listOpenVikingMemories, readOpenVikingMemory, writeOpenVikingMemory } from '../../../services/tauri';
import OpenVikingMemoryBrowser from '../OpenVikingMemoryBrowser';
let root: Root; let container: HTMLDivElement;
const uri = 'viking://~/memories/preferences.md';
const doc = {uri,content:'原始偏好',revision:'old-revision'};
const button = (text: string) => [...container.querySelectorAll('button')].find((b) => b.textContent === text)!;
async function render(enabled = true) { await act(async () => root.render(createElement(OpenVikingMemoryBrowser, {enabled,refreshToken:0}))); }
async function edit(value: string) {
  await act(async () => button('preferences.md').click());
  await act(async () => button('编辑记忆').click());
  await act(async () => {
    const input=container.querySelector('textarea')!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value')!.set!.call(input,value);
    input.dispatchEvent(new Event('input',{bubbles:true}));
  });
}
beforeEach(() => {
  vi.resetAllMocks(); Object.assign(globalThis,{IS_REACT_ACT_ENVIRONMENT:true});
  container=document.createElement('div');document.body.append(container);root=createRoot(container);
  vi.mocked(listOpenVikingMemories).mockResolvedValue({entries:[{uri,name:'preferences.md',isDirectory:false}],hasMore:false});
  vi.mocked(readOpenVikingMemory).mockResolvedValue(doc);
});
afterEach(async () => { await act(async () => root.unmount());container.remove();vi.restoreAllMocks(); });
describe('云端记忆浏览与编辑', () => {
  it('does not request memory before credentials are configured', async () => {
    await render(false);expect(listOpenVikingMemories).not.toHaveBeenCalled();
  });
  it('browses and saves explicitly using the read revision', async () => {
    await render();await edit('新的偏好');
    expect(writeOpenVikingMemory).not.toHaveBeenCalled();
    vi.mocked(writeOpenVikingMemory).mockResolvedValue({...doc,content:'新的偏好',revision:'new-revision'});
    await act(async () => button('保存到云端').click());
    expect(writeOpenVikingMemory).toHaveBeenCalledWith(uri,'新的偏好','old-revision');
    expect(container.textContent).toContain('已保存到云端');expect(container.querySelector('textarea')).toBeNull();
  });
  it('retains draft on conflict and guards directory navigation', async () => {
    await render();await edit('未保存修改');
    vi.mocked(writeOpenVikingMemory).mockRejectedValue(new Error('云端记忆已被修改，本次未写入'));
    await act(async () => button('保存到云端').click());
    expect(container.querySelector('textarea')!.value).toBe('未保存修改');
    expect(container.querySelector('[role=alert]')?.textContent).toContain('已被修改');
    vi.mocked(confirmDiscard).mockResolvedValue(false);
    await act(async () => button('刷新目录').click());
    expect(listOpenVikingMemories).toHaveBeenCalledTimes(1);
    expect(container.querySelector('textarea')!.value).toBe('未保存修改');
  });
  it('browses the separate Hermes peer memory scope', async () => {
    await render();await act(async () => button('Hermes 记忆').click());
    expect(listOpenVikingMemories).toHaveBeenLastCalledWith('viking://~/peers/hermes/memories',0);
    expect(container.querySelector('button[aria-label="上级记忆目录"]')?.hasAttribute('disabled')).toBe(true);
  });
  it('shows quota failure and retries without fake local data', async () => {
    vi.mocked(listOpenVikingMemories).mockRejectedValueOnce(new Error('HTTP 402，云端套餐额度不可用'));
    await render();expect(container.textContent).toContain('HTTP 402');
    await act(async () => button('刷新目录').click());
    expect(button('preferences.md')).toBeDefined();expect(container.querySelector('[role=alert]')).toBeNull();
  });
});
