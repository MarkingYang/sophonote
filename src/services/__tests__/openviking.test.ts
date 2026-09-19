import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { disableOpenViking, getOpenVikingStatus, saveOpenVikingConfig, testOpenVikingConnection, listOpenVikingMemories, readOpenVikingMemory, writeOpenVikingMemory } from '../tauri';
const config = { endpoint: 'https://api.vikingdb.cn-beijing.volces.com/openviking', account: '', user: '', agent: 'hermes' };
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
    expect(mock).toHaveBeenCalledWith('agent_openviking_save', { request: { config, apiKey: null, consent: true, allEngines: false } });
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

it('uses cloud memory IPC with revision and no credential projection', async () => {
  mock.mockResolvedValue({success:true,data:{entries:[],hasMore:false}});
  await listOpenVikingMemories('viking://~/memories', 100);
  expect(mock).toHaveBeenLastCalledWith('agent_openviking_list', {uri:'viking://~/memories',offset:100});
  mock.mockResolvedValue({success:true,data:{uri:'viking://~/memories/a.md',content:'old',revision:'revision'}});
  await readOpenVikingMemory('viking://~/memories/a.md');
  expect(mock).toHaveBeenLastCalledWith('agent_openviking_read', {uri:'viking://~/memories/a.md'});
  await writeOpenVikingMemory('viking://~/memories/a.md', 'new', 'revision');
  expect(mock).toHaveBeenLastCalledWith('agent_openviking_write', {request:{uri:'viking://~/memories/a.md',content:'new',baseRevision:'revision'}});
});
