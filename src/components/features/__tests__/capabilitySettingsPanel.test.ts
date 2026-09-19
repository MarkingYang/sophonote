import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EngineCapabilities } from '../../../services/tauri';
import CapabilitySettingsPanel from '../CapabilitySettingsPanel';

const mocks = vi.hoisted(() => ({ load: vi.fn(), native: vi.fn(), listener: vi.fn(), restart: vi.fn() }));
vi.mock('../../../services/tauri', () => ({ agentCapabilities: mocks.load, listenHermesStatusChanged: mocks.listener, restartHermesRuntime: mocks.restart }));
vi.mock('../../aistudio/ProjectChatPanel', () => ({ HermesCapabilitiesPanel: (props: unknown) => { mocks.native(props); return null; } }));

const snapshot = (engine: EngineCapabilities['engine'], name = 'read'): EngineCapabilities => ({ engine, available: true, version: '1', error: null, tools: [{ name, description: '读取文件' }], managedCategories: [], hermes: null });
let root: Root;
let container: HTMLDivElement;
const button = (name: string, scope = container) => [...scope.querySelectorAll<HTMLButtonElement>('button')].find((item) => item.textContent === name)!;
const click = async (name: string, nav: string) => { await act(async () => button(name, container.querySelector(`[aria-label="${nav}"]`)! as HTMLDivElement).click()); };
const search = () => container.querySelector<HTMLInputElement>('[aria-label="搜索能力"]')!;
const type = async (value: string) => { await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(search(), value); search().dispatchEvent(new Event('input', { bubbles: true })); }); };

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  mocks.load.mockReset().mockImplementation(async (engine) => snapshot(engine));
  mocks.native.mockClear(); mocks.listener.mockReset().mockResolvedValue(() => {});
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); });
const render = async () => { await act(async () => root.render(createElement(CapabilitySettingsPanel))); };

describe('多引擎能力目录', () => {
  it('Hermes 读取失败时其它引擎仍显示真实工具，重试只请求失败引擎', async () => {
    mocks.load.mockImplementation(async (engine) => { if (engine === 'hermes') throw new Error('Gateway 离线'); return snapshot(engine); });
    await render();
    expect(container.textContent).toContain('3 项能力');
    expect(container.textContent).toContain('Gateway 离线');
    const calls = mocks.load.mock.calls.length;
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="重试 Hermes"]')!.click());
    expect(mocks.load.mock.calls.slice(calls)).toEqual([['hermes']]);
    await click('Pi', '能力引擎');
    expect(container.textContent).toContain('1 项能力');
    expect(container.textContent).not.toContain('Gateway 离线');
  });
  it('切换引擎和分类保留搜索，不把内置桥接冒充用户 MCP', async () => {
    await render(); await type('read');
    await click('Pi', '能力引擎'); await click('MCP', '能力分类');
    expect(search().value).toBe('read');
    expect(container.textContent).toContain('Pi 尚未接入MCP管理');
    expect(container.textContent).toContain('0 项能力');
    expect(mocks.native).not.toHaveBeenCalled();
    await click('工具', '能力分类');
    expect(container.textContent).toContain('1 项能力');
  });
  it('统一搜索传入 Hermes 管理面，切换分类不丢失关键词', async () => {
    await render(); await type('report');
    await click('Hermes', '能力引擎'); await click('技能', '能力分类');
    expect(mocks.native.mock.lastCall?.[0]).toMatchObject({ searchQuery: 'report', tab: 'skills', showTabs: false });
    await click('资源广场', '能力分类');
    expect(mocks.native.mock.lastCall?.[0]).toMatchObject({ searchQuery: 'report', tab: 'hub' });
    expect(container.textContent).toContain('当前资源安装到 Hermes');
  });
  it('超时后允许重试，旧请求迟到不会覆盖新目录', async () => {
    vi.useFakeTimers();
    let finishOld!: (data: EngineCapabilities) => void;
    mocks.load.mockImplementation((engine) => engine === 'pi' ? new Promise((resolve) => { finishOld = resolve; }) : Promise.resolve(snapshot(engine)));
    await render();
    await act(async () => vi.advanceTimersByTimeAsync(30_000));
    expect(container.textContent).toContain('读取超时');
    mocks.load.mockImplementation(async (engine) => snapshot(engine, 'latest'));
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="重试 Pi"]')!.click());
    await act(async () => finishOld(snapshot('pi', 'obsolete')));
    await click('Pi', '能力引擎');
    expect(container.textContent).toContain('latest');
    expect(container.textContent).not.toContain('obsolete');
  });
  it('未完成请求在页面卸载后不会保留计时器', async () => {
    vi.useFakeTimers(); mocks.load.mockImplementation(() => new Promise(() => {}));
    await render();
    await act(async () => root.unmount());
    expect(vi.getTimerCount()).toBe(0);
    root = createRoot(container);
  });
});
