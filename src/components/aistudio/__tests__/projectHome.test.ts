import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/workspaceBinding', () => ({ PROJECT_WORKSPACE_KEY_PREFIX: 'ui:project-workspace:', loadWorkspaceBinding: vi.fn().mockResolvedValue({ root: '/workspace/research', name: 'research' }) }));
import ProjectHome from '../ProjectHome';
import type { Project } from '../../../types';
let host: HTMLDivElement; let root: Root;
beforeEach(() => { Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true }); host = document.createElement('div'); document.body.append(host); root = createRoot(host); });
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });
describe('中文项目首页', () => {
  it('无项目时隐藏空态卡片，仅保留顶部创建入口与加载反馈', async () => {
    const onCreate = vi.fn();
    const props = { projects: [], threads: [], loading: true, onCreate, onOpen: vi.fn() };
    await act(async () => root.render(createElement(ProjectHome, props)));
    expect(host.textContent).toContain('正在加载项目');
    expect(host.textContent).not.toContain('创建你的第一个项目');
    await act(async () => root.render(createElement(ProjectHome, { ...props, loading: false })));
    expect(host.textContent).not.toContain('创建你的第一个项目');
    expect(host.querySelector('[aria-label="未归属任务"]')).toBeNull();
    expect([...host.querySelectorAll('button')].filter((b) => b.textContent === '创建项目')).toHaveLength(1);
    await act(async () => [...host.querySelectorAll('button')].find((b) => b.textContent === '创建项目')!.click());
    expect(onCreate).toHaveBeenCalledOnce();
  });
  it('卡片按更新或名称排序，点击传递精确项目 ID', async () => {
    const projects: Project[] = [
      { id: 'a', name: 'A 项目', createdAt: '2026-09-01', docCount: 0 },
      { id: 'b', name: 'B 项目', createdAt: '2026-09-18', docCount: 0 },
    ];
    const onOpen = vi.fn();
    await act(async () => root.render(createElement(ProjectHome, { projects, threads: [], loading: false, onCreate: vi.fn(), onOpen })));
    const cards = () => [...host.querySelectorAll('[aria-label^="打开项目"]')];
    expect(cards()[0].getAttribute('aria-label')).toBe('打开项目 B 项目');
    const sort = host.querySelector('select')!;
    await act(async () => { sort.value = 'name'; sort.dispatchEvent(new Event('change', { bubbles: true })); });
    expect(cards()[0].getAttribute('aria-label')).toBe('打开项目 A 项目');
    expect(cards()[0].textContent).toContain('research');
    await act(async () => (cards()[0] as HTMLButtonElement).click());
    expect(onOpen).toHaveBeenCalledWith('a');
  });
});
