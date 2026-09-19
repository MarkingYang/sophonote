import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { hasApiKey } from '../tauri';
import { useAppStore } from '../../stores/appStore';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  useAppStore.setState({ apiKeys: {} });
});

describe('凭据状态契约', () => {
  it('only exposes configured or missing through the existing command', async () => {
    invokeMock.mockResolvedValueOnce({ success: true, data: 'configured' });
    await expect(hasApiKey('openrouter-rankings')).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith('keychain_get_api_key', { provider: 'openrouter-rankings' });
    invokeMock.mockResolvedValueOnce({ success: false, error: 'not_found' });
    await expect(hasApiKey('embedding')).resolves.toBe(false);
  });

  it('keeps unavailable credentials unknown and permits a later successful retry', async () => {
    invokeMock.mockResolvedValueOnce({ success: false, error: '请在对应配置中重新保存并完成系统授权' });
    await useAppStore.getState().ensureApiKeyLoaded('deepseek');
    expect(useAppStore.getState().apiKeys.deepseek).toBeUndefined();
    invokeMock.mockResolvedValueOnce({ success: true, data: 'configured' });
    await useAppStore.getState().ensureApiKeyLoaded('deepseek');
    expect(useAppStore.getState().apiKeys.deepseek).toBe('configured');
  });

  it('a stale status request cannot overwrite a credential saved in the meantime', async () => {
    let resolve!: (value: unknown) => void;
    invokeMock.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    const pending = useAppStore.getState().ensureApiKeyLoaded('deepseek');
    useAppStore.setState({ apiKeys: { deepseek: 'configured' } });
    resolve({ success: false, error: 'not_found' });
    await pending;
    expect(useAppStore.getState().apiKeys.deepseek).toBe('configured');
  });
});
