import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class { onmessage = null; } }));
import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from '../agentStore';
import { submissionScopeKey, readPendingSubmissions, readSubmissionDraft, recoverableSubmission } from '../../services/agentSubmission';
const mock = vi.mocked(invoke);
const key = submissionScopeKey(null);
const preparing = { submissionId: 's', clientRequestId: 'id', threadId: null, runId: null, requestedEngine: 'auto', selectedEngine: null, state: 'preparing', generation: 1, cancelRequested: false, message: null };
const finished = { ...preparing, threadId: 't', runId: 'r', selectedEngine: 'pi', state: 'succeeded', generation: 4 };
const start = () => useAgentStore.getState().startRun(null, '原始任务', undefined, null, null, null, [], 'm', 'p', null, false, null, 'plan', undefined, true);
beforeEach(() => {
  mock.mockReset(); localStorage.clear();
  useAgentStore.setState({ pendingSubmissions: {}, submissionMonitors: {}, threads: [], runningRunByThreadId: {}, resumingRunByThreadId: {}, historyLoadingByThreadId: {}, eventsByRunId: {}, lastStartError: null });
  vi.spyOn(useAgentStore.getState(), 'loadRunSnapshot').mockResolvedValue(null);
  vi.spyOn(useAgentStore.getState(), 'loadThreads').mockResolvedValue();
});
afterEach(() => vi.restoreAllMocks());
describe('自动提交到 Run 的恢复', () => {
  it('审批回传使用卡片的具体身份，不只发送 Run ID', async () => {
    mock.mockResolvedValue({ success: true, data: true });
    expect(await useAgentStore.getState().respondApproval('run', 'approval', 'once')).toBe(true);
    expect(mock).toHaveBeenCalledWith('agent_run_approval_respond', { runId: 'run', approvalId: 'approval', choice: 'once', all: false });
  });
  it('不发送引擎选择，只提交一次；以 Host 实际引擎绑定并保留极快终态', async () => {
    mock.mockImplementation(async (command) => ({ success: true, data: command === 'agent_submission_auto_start' ? preparing : finished }));
    expect(await start()).toEqual(expect.objectContaining({ threadId: 't', runId: 'r' }));
    const calls = mock.mock.calls.filter(([command]) => command === 'agent_submission_auto_start');
    expect(calls).toHaveLength(1);
    const args = calls[0][1] as { request: { clientRequestId: string; request: Record<string, unknown> } };
    expect(args.request.request.engine).toBeUndefined();
    expect(args.request.request.hermesProvider).toBe('p');
    expect(args.request.clientRequestId).toMatch(/-7[0-9a-f]{3}-/);
    expect(useAgentStore.getState().threads[0].engine).toBe('pi');
    expect(useAgentStore.getState().runningRunByThreadId.t).toBeUndefined();
    expect(useAgentStore.getState().pendingSubmissions).toEqual({});
  });
  it.each(['failed', 'cancelled'] as const)('缺少终态事件时保留 Host 的 %s 结果', async (state) => {
    mock.mockResolvedValue({ success: true, data: { ...finished, state } });
    expect(await start()).not.toBeNull();
    expect(useAgentStore.getState().threads[0].status).toBe(state);
    expect(useAgentStore.getState().runningRunByThreadId.t).toBeUndefined();
    expect(useAgentStore.getState().resumingRunByThreadId.t).toBeUndefined();
  });
  it('较新的 Run 中断快照优先于仍在执行中的提交快照', async () => {
    mock.mockResolvedValue({ success: true, data: { ...finished, state: 'running' } });
    vi.mocked(useAgentStore.getState().loadRunSnapshot).mockResolvedValue({
      runId: 'r', threadId: 't', runStatus: 'interrupted', latestSeq: 0,
      events: [], messages: [], toolCalls: [], pendingApprovals: [],
    });
    expect(await start()).not.toBeNull();
    expect(useAgentStore.getState().threads[0].status).toBe('failed');
    expect(useAgentStore.getState().runningRunByThreadId.t).toBeUndefined();
  });
  it('start 回包丢失时只按原 ID 查询，不重发任务', async () => {
    mock.mockImplementation(async (command) => {
      if (command === 'agent_submission_auto_start') throw new Error('connection lost after accept');
      return { success: true, data: finished };
    });
    expect(await start()).not.toBeNull();
    expect(mock.mock.calls.map(([command]) => command)).toEqual(['agent_submission_auto_start', 'agent_submission_find']);
  });
  it('收到明确拒绝时恢复草稿，不留下未知提交锁', async () => {
    mock.mockResolvedValue({ success: false, data: null, error: '执行名额已满' });
    expect(await start()).toBeNull();
    expect(useAgentStore.getState().lastStartError).toBe('执行名额已满');
    expect(useAgentStore.getState().pendingSubmissions).toEqual({});
    expect(mock).toHaveBeenCalledTimes(1);
  });
  it('同一草稿重复发送被阻止，准备取消不创建用户消息或 Run', async () => {
    let accept!: (v: unknown) => void;
    const cancelled = { ...preparing, state: 'cancelled', cancelRequested: true, generation: 2 };
    mock.mockImplementation(async (command) => {
      if (command === 'agent_submission_auto_start') return new Promise((resolve) => { accept = resolve; });
      return { success: true, data: command === 'agent_submission_find' ? preparing : cancelled };
    });
    const pending = start();
    expect(await start()).toBeNull();
    await useAgentStore.getState().cancelSubmission(key);
    accept({ success: true, data: preparing });
    expect(await pending).toBeNull();
    expect(useAgentStore.getState().threads).toEqual([]);
    expect(mock.mock.calls.filter(([command]) => command === 'agent_submission_auto_start')).toHaveLength(1);
    expect(localStorage.getItem('sophonote.pending-submissions.v1')).not.toContain('原始任务');
  });
});


describe('无 Run 草稿恢复', () => {
  it.each([false, true])('名额满的持久拒绝在回包丢失=%s 时仍保留恢复身份且不重发', async (lostReply) => {
    let requestId = '';
    mock.mockImplementation(async (command, args) => {
      if (command === 'agent_submission_auto_start') {
        requestId = (args as { request: { clientRequestId: string } }).request.clientRequestId;
        if (lostReply) throw new Error('reply lost after durable rejection');
      }
      return { success: true, data: { ...preparing, clientRequestId: requestId, state: 'failed', generation: 0, message: '当前任务执行名额已满，请等待已有任务结束或完成恢复' } };
    });
    expect(await start()).toBeNull();
    const saved = readPendingSubmissions();
    expect(recoverableSubmission(saved[key])).toBe(true);
    expect(saved[key].clientRequestId).toBe(requestId);
    expect(saved[key].snapshot?.runId).toBeNull();
    expect(useAgentStore.getState().lastStartError).toContain('名额');
    expect(useAgentStore.getState().threads).toEqual([]);
    expect(JSON.stringify(saved)).not.toContain('原始任务');
    useAgentStore.setState({ pendingSubmissions: saved });
    await useAgentStore.getState().recoverSubmission(key);
    expect(mock.mock.calls.filter(([cmd]) => cmd === 'agent_submission_auto_start')).toHaveLength(1);
    expect(mock.mock.calls.at(-1)?.[0]).toBe('agent_submission_get');
  });

  it('准备失败持久化恢复身份，重挂载只查询；正文不进入浏览器存储', async () => {
    const failed = { ...preparing, state: 'failed' };
    mock.mockResolvedValue({ success: true, data: failed });
    expect(await start()).toBeNull();
    const saved = readPendingSubmissions();
    expect(recoverableSubmission(saved[key])).toBe(true);
    expect(JSON.stringify(saved)).not.toContain('原始任务');
    useAgentStore.setState({ pendingSubmissions: saved });
    mock.mockClear();
    expect(await useAgentStore.getState().recoverSubmission(key)).toBeNull();
    expect(mock.mock.calls.map(([command]) => command)).toEqual(['agent_submission_get']);
    const draft = { message: '原始任务', focusDocument: { articleId: 'a', baseVersion: 12, markdown: '原正文', title: '文档' } };
    mock.mockResolvedValue({ success: true, data: draft });
    expect(await readSubmissionDraft(saved[key].clientRequestId)).toEqual(draft);
    expect(mock.mock.calls.at(-1)).toEqual(['agent_submission_draft', { clientRequestId: saved[key].clientRequestId, discard: false }]);
  });
  it('用户再次发送采用新身份；明确拒绝新请求仍保留旧草稿入口', async () => {
    const failed = { ...preparing, state: 'failed' };
    mock.mockResolvedValue({ success: true, data: failed });
    await start();
    const old = useAgentStore.getState().pendingSubmissions[key];
    mock.mockResolvedValue({ success: false, data: null, error: '名额不足' });
    await start();
    expect(useAgentStore.getState().pendingSubmissions[key]).toEqual(old);
    const sent = mock.mock.calls.filter(([cmd]) => cmd === 'agent_submission_auto_start');
    expect(sent).toHaveLength(2);
    expect(sent[0][1]).not.toEqual(sent[1][1]);
  });
  it('有 Run 或未知状态永不作为可重新发送草稿', () => {
    expect(recoverableSubmission({ clientRequestId: 'id', snapshot: { ...finished, state: 'cancelled' }, cancelRequested: true } as never)).toBe(false);
    expect(recoverableSubmission({ clientRequestId: 'id', snapshot: { ...preparing, state: 'recovering' }, cancelRequested: false } as never)).toBe(false);
  });
});
