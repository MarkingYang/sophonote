import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { disableOpenViking, getOpenVikingStatus, saveOpenVikingConfig, testOpenVikingConnection } from '../tauri';
const config = { endpoint: 'http://127.0.0.1:1933', account: '', user: '', agent: 'hermes' };
const mock = vi.mocked(invoke);
beforeEach(() => mock.mockReset());
describe('OpenViking Tauri contract', () => {
  it('reads only the backend configuration projection', async () => {
    const status = { config, keyConfigured: true, available: true, activeProvider: '' };
    mock.mockResolvedValue({ success: true, data: status });
    await expect(getOpenVikingStatus()).resolves.toEqual(status);
    expect(mock).toHaveBeenCalledWith('agent_openviking_status');
  });
  it('preserves the saved key for empty input and forwards consent', async () => {
    mock.mockResolvedValue({ success: true, data: { restartRequired: true, credentialStorage: null } });
    await saveOpenVikingConfig(config, '  ', true);
    expect(mock).toHaveBeenCalledWith('agent_openviking_save', { request: { config, apiKey: null, consent: true } });
    expect(mock).toHaveBeenCalledTimes(1);
  });
  it('tests the draft through Rust without saving or restarting', async () => {
    mock.mockResolvedValue({ success: true, data: { version: '0.2.10' } });
    await expect(testOpenVikingConnection(config, 'fixture-key')).resolves.toEqual({ version: '0.2.10' });
    expect(mock).toHaveBeenCalledWith('agent_openviking_test', { request: { config, apiKey: 'fixture-key' } });
    expect(mock).toHaveBeenCalledTimes(1);
  });
  it('disables via dedicated command, never resets remote memory', async () => {
    mock.mockResolvedValue({ success: true, data: { restartRequired: true, credentialStorage: null } });
    await disableOpenViking();
    expect(mock).toHaveBeenCalledWith('agent_openviking_disable');
    expect(mock).toHaveBeenCalledTimes(1);
  });
  it('surfaces failures and refuses missing result payloads', async () => {
    mock.mockResolvedValue({ success: false, error: 'HTTP 401', data: null });
    await expect(testOpenVikingConnection(config, '')).rejects.toThrow('HTTP 401');
    mock.mockResolvedValue({ success: true, data: null });
    await expect(getOpenVikingStatus()).rejects.toThrow('无法读取');
  });
});
