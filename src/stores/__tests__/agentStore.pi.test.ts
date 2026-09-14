import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class { onmessage = null; } }));
import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from '../agentStore';
import { piModelOptions, piRuntimeStatus, claudeModelOptions, claudeRuntimeStatus } from '../../services/tauri';

const mock = vi.mocked(invoke);
beforeEach(() => {
  mock.mockReset();
  useAgentStore.setState({ threads: [], activeRuns: {}, eventsByRunId: {}, runIdsByThreadId: {},
    messagesByThreadId: {}, toolCardsByThreadId: {}, runningRunByThreadId: {},
    historyLoadingByThreadId: {}, resumingRunByThreadId: {}, lastStartError: null });
});
const start = (engine?: 'pi' | 'hermes' | 'claude_code', thread: string | null = null) => useAgentStore.getState().startRun(
  thread, '测试任务', undefined, null, null, null, [], 'fixture-model', 'fixture-provider', null,
  false, '/fixture/workspace', 'plan', engine,
);

describe('Pi sidecar contract', () => {
  it('sends engine and frozen workspace permission through the existing Run command', async () => {
    mock.mockImplementation(async (command) => ({ success: true, data: command === 'agent_run_start' ? { threadId: 'pi-thread', runId: 'pi-run' } : useAgentStore.getState().threads }));
    await start('pi');
    expect(mock).toHaveBeenCalledWith('agent_run_start', expect.objectContaining({ request: expect.objectContaining({
      engine: 'pi', workspaceRoot: '/fixture/workspace', workspacePermissionMode: 'plan',
      hermesProvider: 'fixture-provider', hermesModel: 'fixture-model',
    }) }));
    expect(useAgentStore.getState().threads[0].engine).toBe('pi');
  });
  it('surfaces an unavailable runtime without creating a thread or falling back to Hermes', async () => {
    mock.mockResolvedValue({ success: false, error: 'Pi 运行时未安装' });
    expect(await start('pi')).toBeNull();
    expect(mock).toHaveBeenCalledTimes(1);
    expect(useAgentStore.getState().lastStartError).toBe('Pi 运行时未安装');
    expect(useAgentStore.getState().threads).toEqual([]);
  });
  it('omits the override when restoring a thread so Rust resolves its persisted engine', async () => {
    mock.mockImplementation(async (command) => ({ success: true, data: command === 'agent_run_start' ? { threadId: 'pi-thread', runId: 'second' } : useAgentStore.getState().threads }));
    useAgentStore.setState({ threads: [{ id: 'pi-thread', engine: 'pi', title: 'Pi', status: 'completed',
      projectId: null, createdAt: 1, updatedAt: 1, latestRunId: 'first' }] });
    await start(undefined, 'pi-thread');
    expect(mock).toHaveBeenCalledWith('agent_run_start', expect.objectContaining({ request: expect.objectContaining({ engine: null }) }));
    expect(useAgentStore.getState().threads[0].engine).toBe('pi');
  });
  it('loads host projections and preserves actionable errors', async () => {
    const status = { available: true, version: '0.85.1', error: null, commands: true, browser: false, mcp: false };
    mock.mockResolvedValueOnce({ success: true, data: status });
    expect(await piRuntimeStatus()).toEqual(status);
    expect(mock).toHaveBeenLastCalledWith('agent_pi_status');
    mock.mockResolvedValueOnce({ success: false, error: '模型配置不可读取' });
    await expect(piModelOptions()).rejects.toThrow('模型配置不可读取');
    expect(mock).toHaveBeenLastCalledWith('agent_pi_models');
  });
});


describe('Claude Code sidecar contract', () => {
  it('dispatches Claude Code through the same frozen Run contract', async () => {
    mock.mockImplementation(async (command) => ({ success: true, data: command === 'agent_run_start' ? { threadId: 'claude-thread', runId: 'claude-run' } : useAgentStore.getState().threads }));
    await start('claude_code');
    expect(mock).toHaveBeenCalledWith('agent_run_start', expect.objectContaining({ request: expect.objectContaining({
      engine: 'claude_code', workspacePermissionMode: 'plan', hermesProvider: 'fixture-provider',
    }) }));
    expect(useAgentStore.getState().threads[0].engine).toBe('claude_code');
  });
  it('keeps a missing CLI error visible and never falls back to Pi or Hermes', async () => {
    mock.mockResolvedValue({ success: false, error: '未检测到 Claude Code' });
    expect(await start('claude_code')).toBeNull();
    expect(mock).toHaveBeenCalledOnce();
    expect(useAgentStore.getState().lastStartError).toBe('未检测到 Claude Code');
    expect(useAgentStore.getState().threads).toEqual([]);
  });
  it('loads only the Claude runtime and provider projection', async () => {
    mock.mockResolvedValueOnce({ success: true, data: { available: true, version: '2.1.233' } });
    expect((await claudeRuntimeStatus()).available).toBe(true);
    expect(mock).toHaveBeenLastCalledWith('agent_claude_status');
    mock.mockResolvedValueOnce({ success: true, data: { providers: [] } });
    expect((await claudeModelOptions()).providers).toEqual([]);
    expect(mock).toHaveBeenLastCalledWith('agent_claude_models');
  });
});
