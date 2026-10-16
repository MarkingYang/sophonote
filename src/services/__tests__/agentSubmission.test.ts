import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke, type Channel } from '@tauri-apps/api/core';
import type { AgentEvent } from '../../stores/agentStore';
import { cancelSubmission, createManualSubmission, getSubmission, startManualSubmission, type SubmissionSnapshot } from '../agentSubmission';

const mock = vi.mocked(invoke);
const snapshot: SubmissionSnapshot = {
  submissionId: 'submission-1', clientRequestId: 'id', threadId: null, runId: null,
  requestedEngine: 'pi', state: 'preparing', generation: 1, cancelRequested: false, message: null,
};
beforeEach(() => { vi.restoreAllMocks(); mock.mockReset(); });

describe('手动提交契约', () => {
  it('按 UUIDv7 创建一次 ID 并固定原输入；重试保留 ID 和上下文', async () => {
    const now = 1_800_000_000_000;
    vi.spyOn(Date, 'now').mockReturnValue(now);
    const input = { engine: 'pi' as const, message: '原输入', attachments: [{ kind: 'file' as const, name: '原附件', path: '/fixture' }] };
    const envelope = createManualSubmission(input);
    expect(envelope.clientRequestId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(parseInt(envelope.clientRequestId.replaceAll('-', '').slice(0, 12), 16)).toBe(now);
    input.message = '已修改'; input.attachments[0].name = '已修改';
    expect(envelope.request.message).toBe('原输入');
    expect(envelope.request.attachments?.[0].name).toBe('原附件');
    expect(Object.isFrozen(envelope.request.attachments?.[0])).toBe(true);
    const channel = {} as Channel<AgentEvent>;
    mock.mockResolvedValue({ success: true, data: snapshot, error: null });
    await startManualSubmission(envelope, channel);
    await startManualSubmission(envelope, channel);
    expect(mock.mock.calls[0]).toEqual(mock.mock.calls[1]);
    expect(mock).toHaveBeenCalledWith('agent_submission_start', { request: envelope, onEvent: channel });
    expect(createManualSubmission(input).clientRequestId).not.toBe(envelope.clientRequestId);
  });

  it('查询和取消只投影 Host 状态，不将取消请求冒充终态', async () => {
    const unknown = { ...snapshot, runId: 'run-1', state: 'dispatching', cancelRequested: true };
    mock.mockResolvedValue({ success: true, data: unknown, error: null });
    expect(await cancelSubmission('submission-1')).toEqual(unknown);
    expect(mock).toHaveBeenCalledWith('agent_submission_cancel', { submissionId: 'submission-1' });
    await getSubmission('submission-1');
    expect(mock).toHaveBeenLastCalledWith('agent_submission_get', { submissionId: 'submission-1' });
    expect(mock).toHaveBeenCalledTimes(2); // no automatic resubmit
  });

  it('传递错误，缺失数据不伪造快照，不自动重试', async () => {
    mock.mockResolvedValue({ success: false, data: null, error: '提交内容不一致' });
    await expect(getSubmission('x')).rejects.toThrow('提交内容不一致');
    mock.mockResolvedValue({ success: true, data: null, error: null });
    await expect(getSubmission('x')).rejects.toThrow('无法读取提交状态');
    mock.mockRejectedValue(new Error('连接断开'));
    await expect(cancelSubmission('x')).rejects.toThrow('连接断开');
    expect(mock).toHaveBeenCalledTimes(3);
  });
});


it('恢复草稿不会覆盖新附件，忽略重挂载产生的视图 ID', async () => {
  const { canRestoreDraftAttachments } = await import('../agentSubmission');
  const saved = [{ kind: 'file' as const, name: '资料', path: '/fixture/original.txt' }];
  expect(canRestoreDraftAttachments([{ ...saved[0], id: 'new-view-id' }], saved)).toBe(true);
  expect(canRestoreDraftAttachments([{ ...saved[0], id: 'new', path: '/fixture/new.txt' }], saved)).toBe(false);
  expect(canRestoreDraftAttachments([], saved)).toBe(true);
});
