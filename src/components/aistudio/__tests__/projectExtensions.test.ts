import { act, createElement, useEffect } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/tauri', () => ({ getSetting: vi.fn().mockResolvedValue('480'), updateSetting: vi.fn().mockResolvedValue(undefined) }));
const lifecycle = vi.hoisted(() => ({ mounts: vi.fn(), closes: vi.fn(), active: true }));
vi.mock('../../layout/KeptAlivePage', () => ({ usePageSurfaceActive: () => lifecycle.active }));
vi.mock('../ProjectFilesPanel', () => ({ default: ({ root, onAttach }: { root: string; onAttach: (path: string, name: string) => void }) => createElement('button', { onClick: () => onAttach(`${root}/readme.md`, 'readme.md') }, '加入文件') }));
vi.mock('../AgentBrowserPanel', () => ({ default: ({ connectAgent, active }: { connectAgent: boolean; active: boolean }) => createElement('div', { 'data-browser': String(connectAgent), 'data-active': String(active) }) }));
vi.mock('../LocalTerminal', () => ({ default: ({ root }: { root: string }) => {
  useEffect(() => { lifecycle.mounts(root); return lifecycle.closes; }, [root]);
  return createElement('div', { 'data-terminal': root });
} }));
import ProjectExtensions from '../ProjectExtensions';
import { updateSetting } from '../../../services/tauri';
let host: HTMLDivElement; let root: Root;
const attach = vi.fn();
const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent === label || node.getAttribute('aria-label') === label)!;
const click = async (label: string) => { await act(async () => button(label).click()); };
const render = async (permissionMode: 'default' | 'plan' = 'default') => {
  await act(async () => root.render(createElement(ProjectExtensions, { root: '/workspace/project', permissionMode, onAttach: attach }, createElement('textarea', { 'aria-label': '对话草稿', defaultValue: '保留草稿' }))));
};
beforeEach(() => {
  vi.clearAllMocks(); lifecycle.active = true; Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
describe('项目右侧扩展', () => {
  it('切换三种扩展时保持对话草稿，显式加入附件，终端在关闭时卸载', async () => {
    await render();
    const draft = host.querySelector('textarea');
    await click('扩展'); await click('文件夹'); await click('加入文件');
    expect(attach).toHaveBeenCalledWith(expect.objectContaining({ kind: 'file', path: '/workspace/project/readme.md' }));
    await click('扩展'); await click('浏览器');
    expect(host.querySelector('[data-browser]')?.getAttribute('data-browser')).toBe('false');
    await click('扩展');
    expect(host.querySelector('[data-browser]')?.getAttribute('data-active')).toBe('false');
    await click('终端');
    expect(lifecycle.mounts).toHaveBeenCalledWith('/workspace/project');
    expect(host.querySelector('textarea')).toBe(draft);
    expect(draft?.value).toBe('保留草稿');
    await click('关闭扩展');
    expect(lifecycle.closes).toHaveBeenCalledTimes(1);
    expect(host.querySelector('aside')).toBeNull();
  });
  it('离开保活项目页也回收终端，返回不会静默重启', async () => {
    await render(); await click('扩展'); await click('终端');
    lifecycle.active = false; await render();
    expect(lifecycle.closes).toHaveBeenCalledTimes(1);
    lifecycle.active = true; await render();
    expect(lifecycle.mounts).toHaveBeenCalledTimes(1);
    expect(host.querySelector('[data-terminal]')).toBeNull();
  });
  it('计划模式不启动 PTY，切换权限后才挂载；键盘调整宽度持久化', async () => {
    await render('plan'); await click('扩展'); await click('终端');
    expect(lifecycle.mounts).not.toHaveBeenCalled();
    expect(host.textContent).toContain('计划模式下终端不可用');
    await render(); expect(lifecycle.mounts).toHaveBeenCalledTimes(1);
    const handle = host.querySelector('[role="separator"]')!;
    await act(async () => handle.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true })));
    expect(updateSetting).toHaveBeenLastCalledWith('ui:project-extension-width', '488');
    await render('plan'); expect(lifecycle.closes).toHaveBeenCalledTimes(1);
  });
});
