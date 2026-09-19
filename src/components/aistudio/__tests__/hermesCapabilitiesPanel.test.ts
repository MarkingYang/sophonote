import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { HermesCapabilities, HermesHubPage } from '../../../services/tauri';
import { HermesCapabilitiesPanel } from '../ProjectChatPanel';

const mocks = vi.hoisted(() => ({ hub: vi.fn(), enable: vi.fn() }));
vi.mock('../../../services/tauri', async (original) => ({ ...await original<typeof import('../../../services/tauri')>(), hermesSkillsHub: mocks.hub, hermesSkillSetEnabled: mocks.enable }));
const data: HermesCapabilities = {
  commands: [], references: [], tools: [], toolsets: [], mcpServers: [],
  skills: [{ name: 'report', description: '行业研究', origin: 'bundled', enabled: true }, { name: 'other', description: '写作', origin: 'bundled', enabled: true }],
  terminalBackends: { active: '', backends: [] }, hubSources: { indexAvailable: true, sources: [] }, browserConnected: false, browserUrl: '',
};
let root: Root; let container: HTMLDivElement;
const refresh = vi.fn();
const render = async (tab: 'skills' | 'hub', searchQuery: string) => { await act(async () => root.render(createElement(HermesCapabilitiesPanel, { snapshot: data, error: null, connStatus: 'connected', tab, searchQuery, onTab: vi.fn(), onRefresh: refresh, onReconnect: vi.fn(), embedded: true, showTabs: false }))); };
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
  mocks.enable.mockReset().mockResolvedValue(undefined); mocks.hub.mockReset(); refresh.mockClear();
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); });

describe('Hermes 管理面统一搜索', () => {
  it('外部搜索过滤真实管理列表且没有重复搜索框，启停仍调用原生接口', async () => {
    await render('skills', 'Hermes 行业');
    expect(container.textContent).toContain('report'); expect(container.textContent).not.toContain('other');
    expect(container.querySelector('input')).toBeNull();
    expect(container.querySelector('button button')).toBeNull();
    await act(async () => container.querySelector<HTMLButtonElement>('[role="switch"]')!.click());
    expect(mocks.enable).toHaveBeenCalledWith('report', false); expect(refresh).toHaveBeenCalledOnce();
  });
  it('资源广场超时结束等待，重新进入后可重试', async () => {
    vi.useFakeTimers(); mocks.hub.mockImplementation(() => new Promise(() => {}));
    await render('hub', 'slow');
    await act(async () => vi.advanceTimersByTimeAsync(30_350));
    expect(container.textContent).toContain('资源广场搜索超时');
    expect(container.textContent).not.toContain('正在检索连接的 Hub');
    await render('skills', 'slow'); await render('hub', 'slow');
    await act(async () => vi.advanceTimersByTimeAsync(350));
    expect(mocks.hub).toHaveBeenCalledTimes(2);
  });
  it('资源广场搜索防抖，迟到的旧结果不会覆盖新关键词结果', async () => {
    vi.useFakeTimers();
    let oldResult!: (page: HermesHubPage) => void;
    const page = (name: string) => ({ items: [{ name, identifier: name, description: '', source: '', trust: '' }], total: 1, page: 1, totalPages: 1 });
    mocks.hub.mockImplementationOnce(() => new Promise((resolve) => { oldResult = resolve; })).mockResolvedValueOnce(page('new-result'));
    await render('hub', 'old');
    await act(async () => vi.advanceTimersByTimeAsync(350));
    await render('hub', 'new');
    await act(async () => vi.advanceTimersByTimeAsync(350));
    expect(mocks.hub.mock.calls).toEqual([['old', 1], ['new', 1]]);
    await act(async () => oldResult(page('old-result') as HermesHubPage));
    expect(container.textContent).toContain('new-result'); expect(container.textContent).not.toContain('old-result');
    expect(container.textContent).toContain('安装到 Hermes');
  });
});
