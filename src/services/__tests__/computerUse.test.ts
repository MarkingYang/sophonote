import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { computerUseStatus, computerUseActionStart, computerUseActionStatus } from '../tauri';

describe('Computer Use Tauri contract', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());
  it('preserves unknown readiness instead of inferring success', async () => {
    vi.mocked(invoke).mockResolvedValue({ success: true, data: { installed: true, ready: null } });
    await expect(computerUseStatus()).resolves.toEqual({ installed: true, ready: null });
    expect(invoke).toHaveBeenCalledWith('agent_computer_use_status');
  });
  it('uses typed installation/permission commands and preserves process identity', async () => {
    const data = { running: true, exitCode: null, pid: 42 };
    vi.mocked(invoke).mockResolvedValue({ success: true, data });
    await expect(computerUseActionStart('install')).resolves.toEqual(data);
    expect(invoke).toHaveBeenCalledWith('agent_computer_use_action_start', { action: 'install' });
    await computerUseActionStatus('grant');
    expect(invoke).toHaveBeenCalledWith('agent_computer_use_action_status', { action: 'grant' });
  });
  it('does not hide unsupported Runtime or setup errors', async () => {
    vi.mocked(invoke).mockResolvedValue({ success: false, error: 'Hermes 404' });
    await expect(computerUseStatus()).rejects.toThrow('Hermes 404');
    await expect(computerUseActionStart('install')).rejects.toThrow('Hermes 404');
  });
});
