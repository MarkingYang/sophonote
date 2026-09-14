import { beforeEach, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { getAgentUpdateStatus, updateAgentRuntime } from '../tauri';
beforeEach(() => vi.mocked(invoke).mockReset());
it('将引擎与显式检查标记传给 Host，不触发其它引擎更新', async () => {
  vi.mocked(invoke).mockResolvedValue({ success: true, data: { engine: 'pi' } });
  await getAgentUpdateStatus('pi', true);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('agent_runtime_update_status', { engine: 'pi', check: true });
});
it('保留后端在途 Run 错误，不把失败当成更新成功', async () => {
  vi.mocked(invoke).mockResolvedValue({ success: false, error: '请先结束正在执行的 Claude Code 会话' });
  await expect(updateAgentRuntime('claude_code')).rejects.toThrow('请先结束');
  expect(invoke).toHaveBeenCalledExactlyOnceWith('agent_runtime_update', { engine: 'claude_code' });
});
