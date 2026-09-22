// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import AgentRuntimeUpdateCard from '../AgentRuntimeUpdateCard';
import * as api from '../../../services/tauri';
import { openUrl } from '@tauri-apps/plugin-opener';
vi.mock('../../../services/tauri', () => ({ getAgentUpdateStatus: vi.fn(), listenAgentUpdateProgress: vi.fn(), updateAgentRuntime: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }));
const status: api.AgentUpdateStatus = { engine: 'pi', currentVersion: '0.85.1', latestVersion: null, source: '随 SophoNote 分发', canUpdate: true, message: null, progress: null };
let root: Root;
let container: HTMLDivElement;
const button = (text: string) => [...container.querySelectorAll('button')].find((b) => b.textContent?.includes(text))!;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true }); vi.resetAllMocks();
  vi.mocked(api.getAgentUpdateStatus).mockResolvedValue(status);
  vi.mocked(api.listenAgentUpdateProgress).mockResolvedValue(vi.fn());
  vi.mocked(api.updateAgentRuntime).mockResolvedValue(status);
  vi.mocked(openUrl).mockResolvedValue(undefined);
  container = document.createElement('div'); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); });
it('Pi 展示版本和生效策略，检查官方稳定版', async () => {
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'pi' })));
  expect(container.textContent).toContain('v0.85.1'); expect(container.textContent).toContain('下一轮');
  await act(async () => button('检查版本').click());
  expect(api.getAgentUpdateStatus).toHaveBeenLastCalledWith('pi', true);
});
it('更新先订阅进度，等待期间防止重复点击，失败可重试', async () => {
  let reject!: (e: Error) => void;
  vi.mocked(api.updateAgentRuntime).mockImplementation(() => new Promise((_, fail) => { reject = fail; }));
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'pi' })));
  await act(async () => button('拉取更新').click());
  expect(button('更新中').disabled).toBe(true);
  expect(api.listenAgentUpdateProgress).toHaveBeenCalledTimes(2);
  await act(async () => reject(new Error('官方下载中断')));
  expect(container.querySelector('[role="alert"]')?.textContent).toContain('官方下载中断');
  expect(button('拉取更新').disabled).toBe(false);
});
it('自定义 Claude 安装明确提示手动更新，不调用未知更新器', async () => {
  vi.mocked(api.getAgentUpdateStatus).mockResolvedValue({ ...status, engine: 'claude_code', canUpdate: false, message: '请按原安装方式更新' });
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'claude_code' })));
  expect(button('更新本机 CLI').disabled).toBe(true);
  expect(container.textContent).toContain('请按原安装方式更新');
  await act(async () => button('重新检测').click());
  expect(api.updateAgentRuntime).not.toHaveBeenCalled();
});
it('重挂载恢复后台更新状态及下载进度', async () => {
  vi.mocked(api.getAgentUpdateStatus).mockResolvedValue({ ...status, progress: { engine: 'pi', phase: 'downloading', state: 'running', message: '正在下载', bytesDownloaded: 1048576, totalBytes: 2097152 } });
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'pi' })));
  expect(button('更新中').disabled).toBe(true);
  expect(container.querySelector('progress')?.value).toBe(50);
});
it('OpenCode 共用完整更新卡片，检查、更新和说明链接均指向自身', async () => {
  const opencode = { ...status, engine: 'opencode' as const, currentVersion: '1.18.31', latestVersion: '1.18.32' };
  vi.mocked(api.getAgentUpdateStatus).mockResolvedValue(opencode);
  vi.mocked(api.updateAgentRuntime).mockResolvedValue({ ...opencode, currentVersion: '1.18.32' });
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'opencode' })));
  for (const label of ['当前版本', '运行来源', '更新策略', '更新源', '官方稳定版', 'anomalyco/opencode', '下一轮生效']) {
    expect(container.textContent).toContain(label);
  }
  await act(async () => button('检查版本').click());
  expect(api.getAgentUpdateStatus).toHaveBeenLastCalledWith('opencode', true);
  await act(async () => button('拉取更新').click());
  expect(api.updateAgentRuntime).toHaveBeenCalledExactlyOnceWith('opencode');
  expect(container.textContent).toContain('v1.18.32');
  await act(async () => button('官方更新说明').click());
  expect(openUrl).toHaveBeenCalledWith('https://github.com/anomalyco/opencode/releases');
});
it('OpenCode 忽略其他引擎进度，失败保留版本且允许重试', async () => {
  let notify!: (progress: api.AgentUpdateProgress) => void;
  vi.mocked(api.listenAgentUpdateProgress).mockImplementation(async (callback) => { notify = callback; return vi.fn(); });
  vi.mocked(api.getAgentUpdateStatus).mockResolvedValue({ ...status, engine: 'opencode', currentVersion: '1.18.31' });
  vi.mocked(api.updateAgentRuntime).mockRejectedValue(new Error('SHA-256 不匹配，保留原版本'));
  await act(async () => root.render(createElement(AgentRuntimeUpdateCard, { engine: 'opencode' })));
  await act(async () => notify({ engine: 'pi', phase: 'downloading', state: 'running', message: 'Pi 下载中', bytesDownloaded: 1, totalBytes: 2 }));
  expect(container.textContent).not.toContain('Pi 下载中');
  expect(button('拉取更新').disabled).toBe(false);
  await act(async () => button('拉取更新').click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain('SHA-256');
  expect(container.textContent).toContain('v1.18.31');
  expect(button('拉取更新').disabled).toBe(false);
});
