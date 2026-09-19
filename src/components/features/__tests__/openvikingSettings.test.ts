import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/tauri', () => ({
  getOpenVikingStatus: vi.fn(), saveOpenVikingConfig: vi.fn(), testOpenVikingConnection: vi.fn(),
  disableOpenViking: vi.fn(), restartHermesRuntime: vi.fn(), listOpenVikingMemories: vi.fn().mockResolvedValue({entries:[],hasMore:false}),
}));
import { getOpenVikingStatus, saveOpenVikingConfig, disableOpenViking, restartHermesRuntime, testOpenVikingConnection } from '../../../services/tauri';
import OpenVikingSettingsPanel from '../OpenVikingSettingsPanel';
let root: Root;
let container: HTMLDivElement;
const config = { endpoint: 'https://api.vikingdb.cn-beijing.volces.com/openviking', account: '', user: '', agent: 'hermes' };
const status = { config, available: true, activeProvider: '', keyConfigured: true };
const button = (text: string) => [...container.querySelectorAll('button')].find((b) => b.textContent === text)!;
async function render() { await act(async () => root.render(createElement(OpenVikingSettingsPanel))); }
beforeEach(() => {
  vi.resetAllMocks();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
  vi.mocked(getOpenVikingStatus).mockResolvedValue(status);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });
describe('OpenViking 设置交互', () => {
  it('requires consent before activation and never reads back a saved key', async () => {
    await render();
    expect(button('保存并启用').disabled).toBe(true);
    expect((container.querySelector('#openviking-key') as HTMLInputElement).value).toBe('');
    expect(container.textContent).toContain('已保存');
    await act(async () => (container.querySelector('input[type=checkbox]') as HTMLInputElement).click());
    vi.mocked(saveOpenVikingConfig).mockResolvedValue({ credentialStorage: null, restartRequired: true });
    await act(async () => button('保存并启用').click());
    expect(saveOpenVikingConfig).toHaveBeenCalledWith(config, '', true, true);
    expect(restartHermesRuntime).not.toHaveBeenCalled();
    expect(container.textContent).toContain('配置已保存并启用');
    expect(container.textContent).not.toContain('连接与访问权限检查通过');
  });
  it('does not silently expand legacy consent and reports per-engine failures', async () => {
    vi.mocked(getOpenVikingStatus).mockResolvedValue({ ...status, activeProvider: 'openviking', hostEnginesEnabled: false });
    vi.mocked(testOpenVikingConnection).mockResolvedValue({ version:'cloud', engines:[
      {engine:'hermes',success:true,message:'鉴权通过'}, {engine:'pi',success:false,message:'HTTP 403'},
      {engine:'claude_code',success:true,message:'鉴权通过'}, {engine:'opencode',success:true,message:'鉴权通过'},
    ] });
    await render();
    expect(button('保存配置').disabled).toBe(true);
    expect(container.textContent).toContain('旧配置仅启用 Hermes');
    await act(async () => button('测试连接').click());
    expect(container.textContent).toContain('Pi失败');
    expect(container.textContent).toContain('HTTP 403');
    expect(container.textContent).toContain('Claude Code通过');
    expect(container.textContent).toContain('不代表自动提取');
    expect(saveOpenVikingConfig).not.toHaveBeenCalled();
  });
  it('keeps inputs disabled with a readable retry when Hermes is unavailable', async () => {
    vi.mocked(getOpenVikingStatus).mockRejectedValueOnce(new Error('Hermes 未连接'));
    await render();
    expect(container.querySelector('[role=alert]')?.textContent).toBe('Hermes 未连接');
    expect(button('保存并启用').disabled).toBe(true);
    await act(async () => button('重新读取').click());
    expect(container.querySelector('fieldset')?.disabled).toBe(false);
  });
  it('disables without deleting data or automatically restarting', async () => {
    vi.mocked(getOpenVikingStatus).mockResolvedValue({ ...status, activeProvider: 'openviking' });
    vi.mocked(disableOpenViking).mockResolvedValue({ credentialStorage: null, restartRequired: true });
    await render();
    await act(async () => button('停用自动记忆').click());
    expect(disableOpenViking).toHaveBeenCalledTimes(1);
    expect(restartHermesRuntime).not.toHaveBeenCalled();
    expect(container.textContent).toContain('远端数据保留');
  });
  it('keeps partial credential-save failure explicit instead of claiming enabled', async () => {
    vi.mocked(saveOpenVikingConfig).mockRejectedValue(new Error('凭据已保存；连接配置未保存'));
    await render();
    await act(async () => (container.querySelector('input[type=checkbox]') as HTMLInputElement).click());
    await act(async () => button('保存并启用').click());
    expect(container.querySelector('[role=alert]')?.textContent).toContain('凭据已保存；连接配置未保存');
    expect(button('保存并启用')).toBeDefined();
  });
});
