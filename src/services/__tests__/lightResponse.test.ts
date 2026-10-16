import { webcrypto } from 'node:crypto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class {} }));
import { invoke } from '@tauri-apps/api/core';
import { startTextResponse, responseAction } from '../lightResponse';
import { useAgentStore } from '../../stores/agentStore';
const mock = vi.mocked(invoke);
const request = { message: '翻译：hello', projectId: 'p', hermesProvider: 'configured-2', hermesModel: 'selected' };
beforeEach(() => { vi.restoreAllMocks(); mock.mockReset(); localStorage.clear(); vi.stubGlobal('crypto', webcrypto); });
describe('独立文本响应提交', () => {
  it('丢失回包保留请求身份；手动重试原意图，成功后下一条使用新身份', async () => {
    mock.mockRejectedValueOnce(new Error('lost reply')).mockResolvedValue({ success: true, data: { id: 'turn', conversationId: 'response-1' } });
    await expect(startTextResponse(request)).rejects.toThrow('lost reply');
    expect(JSON.stringify(localStorage)).not.toContain(request.message);
    await startTextResponse(request);
    const first = mock.mock.calls[0][1] as { clientRequestId: string; request: object };
    expect(mock.mock.calls[1][1]).toEqual(first);
    expect(first.clientRequestId).toMatch(/-7[0-9a-f]{3}-/);
    expect(first.request).not.toHaveProperty('engine');
    await startTextResponse(request);
    expect((mock.mock.calls[2][1] as typeof first).clientRequestId).not.toBe(first.clientRequestId);
    expect(mock.mock.calls.every(([command]) => command === 'light_response_start')).toBe(true);
  });
  it('明确拒绝可重新提交；未匹配画像返回 null，由调用方进入原路由', async () => {
    mock.mockResolvedValueOnce({ success: false, data: null, error: '配置变化' }).mockResolvedValue({ success: true, data: null });
    await expect(startTextResponse(request)).rejects.toThrow('配置变化');
    expect(localStorage.length).toBe(0);
    await expect(startTextResponse(request)).resolves.toBeNull();
  });
  it('导航合并时保留不同身份，不伪造 Agent Run 或引擎', async () => {
    const agent = { id: 'thread-1', projectId: 'p', title: 'Agent', engine: 'pi', latestRunId: 'run-1', updatedAt: 1 };
    const text = { id: 'response-1', conversationKind: 'response', projectId: 'p', title: '文本', latestRunId: null, updatedAt: 2 };
    mock.mockImplementation(async command => ({ success: true, data: command === 'agent_thread_list' ? [agent] : [text] }));
    useAgentStore.setState({ threads: [], historyThreads: [] });
    await useAgentStore.getState().loadThreads('p');
    expect(useAgentStore.getState().threads).toEqual([text, agent]);
    expect(useAgentStore.getState().threads[0].engine).toBeUndefined();
    await responseAction('response-1', 'p', 'close');
    expect(mock).toHaveBeenLastCalledWith('light_response_action', { conversationId: 'response-1', projectId: 'p', action: 'close', value: null });
  });
});
