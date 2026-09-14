// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { useSidecarConfig } from '../useSidecarConfig';
import type { AgentEngine } from '../../../stores/agentStore';
import * as tauri from '../../../services/tauri';
vi.mock('../../../services/tauri', () => ({ piRuntimeStatus: vi.fn(), piModelOptions: vi.fn(), claudeRuntimeStatus: vi.fn(), claudeModelOptions: vi.fn() }));
const options = { provider: 'deepseek', model: 'flash', providers: [{ slug: 'deepseek', name: 'DeepSeek', models: ['flash', 'pro'], authenticated: null, isCurrent: null }] };
const ready = { available: true, version: 'test', error: null, commands: true, browser: false, mcp: false };
let root: Root;
let container: HTMLDivElement;
let result: ReturnType<typeof useSidecarConfig>;
const config = {};
function Probe({ engine }: { engine: AgentEngine }) { result = useSidecarConfig(engine, config); return null; }
async function render(engine: AgentEngine = 'pi') { await act(async () => root.render(createElement(Probe, { engine }))); }
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.resetAllMocks(); window.localStorage.clear();
  vi.mocked(tauri.piRuntimeStatus).mockResolvedValue(ready);
  vi.mocked(tauri.claudeRuntimeStatus).mockResolvedValue(ready);
  vi.mocked(tauri.piModelOptions).mockResolvedValue(options);
  vi.mocked(tauri.claudeModelOptions).mockResolvedValue(options);
  container = document.createElement('div'); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); vi.useRealTimers(); });
it('模型先显示，不等待运行时校验，沿用 Hermes 已选模型', async () => {
  window.localStorage.setItem('sophonote.hermes.provider', 'deepseek');
  window.localStorage.setItem('sophonote.hermes.model', 'pro');
  vi.mocked(tauri.piRuntimeStatus).mockReturnValue(new Promise(() => {}));
  await render();
  expect(result.selection).toEqual({ provider: 'deepseek', model: 'pro' });
  expect(result.modelsLoading).toBe(false); expect(result.runtimeLoading).toBe(true);
});
it('模型读取不会阻塞运行时状态；超时后可重试', async () => {
  vi.useFakeTimers();
  vi.mocked(tauri.piModelOptions).mockReturnValueOnce(new Promise(() => {}));
  await render(); expect(result.status?.available).toBe(true);
  await act(async () => { vi.advanceTimersByTime(30_000); });
  expect(result.modelsLoading).toBe(false); expect(result.error).toContain('超时');
  await act(async () => result.refresh((n) => n + 1));
  expect(result.error).toBeNull(); expect(result.selection.model).toBe('flash');
});
it('切换 Claude 后不接受 Pi 的迟到结果，保持同一模型偏好', async () => {
  let resolve!: (value: typeof options) => void;
  vi.mocked(tauri.piModelOptions).mockReturnValueOnce(new Promise((done) => { resolve = done; }));
  await render(); await render('claude_code');
  await act(async () => resolve({ ...options, model: 'pro' }));
  expect(result.selection.model).toBe('flash'); expect(result.status?.available).toBe(true);
});
