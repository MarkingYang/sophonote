import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getHermesSidecarStatus, listenHermesSidecarProgress, pullHermesSidecar } from '../tauri';

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const status = {
  currentVersion: '0.20.0',
  currentCommit: '07da945c214481083049500bd29f45cabc5a04b2',
  currentSource: 'bundled',
  pendingVersion: '0.20.6',
  pendingCommit: 'a'.repeat(40),
  updateReady: true,
  repository: 'https://github.com/NousResearch/hermes-agent',
};

describe('Hermes Sidecar Tauri contract', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
  });

  it('reads current and pending versions through the dedicated command', async () => {
    invokeMock.mockResolvedValue({ success: true, data: status, error: null });
    await expect(getHermesSidecarStatus()).resolves.toEqual(status);
    expect(invokeMock).toHaveBeenCalledWith('hermes_sidecar_status');
  });

  it('pulls into the pending slot and preserves backend errors', async () => {
    invokeMock.mockResolvedValueOnce({ success: true, data: status, error: null });
    await expect(pullHermesSidecar()).resolves.toEqual(status);
    expect(invokeMock).toHaveBeenCalledWith('hermes_sidecar_pull');

    invokeMock.mockResolvedValueOnce({ success: false, data: null, error: 'download failed' });
    await expect(pullHermesSidecar()).rejects.toThrow('download failed');
  });

  it('projects phased update progress from the dedicated event', async () => {
    const unlisten = vi.fn();
    listenMock.mockImplementationOnce(async (_event, handler) => {
      handler({
        event: 'sophonote:hermes-sidecar-update-progress',
        id: 1,
        payload: {
          operationId: 'update-1',
          phase: 'downloading',
          state: 'running',
          percent: 24,
          message: '正在下载官方 Release…',
          bytesDownloaded: 1024,
          totalBytes: 4096,
        },
      });
      return unlisten;
    });
    const callback = vi.fn();

    await expect(listenHermesSidecarProgress(callback)).resolves.toBe(unlisten);
    expect(listenMock).toHaveBeenCalledWith(
      'sophonote:hermes-sidecar-update-progress',
      expect.any(Function),
    );
    expect(callback).toHaveBeenCalledWith(expect.objectContaining({
      phase: 'downloading',
      percent: 24,
      bytesDownloaded: 1024,
    }));
  });
});
