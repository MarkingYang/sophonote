import { describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { agentCapabilities } from '../tauri';

describe('能力目录 IPC 契约', () => {
  it('四种引擎通过统一只读命令传递明确的引擎标识', async () => {
    for (const engine of ['pi', 'claude_code', 'opencode', 'hermes'] as const) {
      const data = { engine, available: false, version: null, error: '未安装', managedCategories: [], tools: [], hermes: null };
      vi.mocked(invoke).mockResolvedValueOnce({ success: true, data });
      await expect(agentCapabilities(engine)).resolves.toEqual(data);
      expect(invoke).toHaveBeenLastCalledWith('agent_capabilities', { engine });
    }
  });
  it('拒绝把失败响应或缺失数据包装为空目录', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: false, error: 'Gateway 离线' });
    await expect(agentCapabilities('hermes')).rejects.toThrow('Gateway 离线');
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data: null });
    await expect(agentCapabilities('pi')).rejects.toThrow('无法读取引擎能力');
  });
});
