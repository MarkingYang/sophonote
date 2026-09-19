import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { projectAssignThread, projectCreate } from '../tauri';
import { parseWorkspaceBinding, peekWorkspaceBinding, rememberWorkspaceBinding, saveWorkspaceBinding } from '../workspaceBinding';

beforeEach(() => vi.mocked(invoke).mockReset());
describe('项目任务 IPC 契约', () => {
  it('项目与目录一次提交，兼容旧调用', async () => {
    const project = { id: 'p', name: '研究', docCount: 0, createdAt: '2026-09-18' };
    vi.mocked(invoke).mockResolvedValue({ success: true, data: project });
    await expect(projectCreate(project, '/workspace/research')).resolves.toEqual(project);
    expect(invoke).toHaveBeenLastCalledWith('project_create', { project, workspaceRoot: '/workspace/research' });
    await projectCreate(project);
    expect(invoke).toHaveBeenLastCalledWith('project_create', { project, workspaceRoot: null });
  });
  it('任务只能移到有效项目，空目标和后端拒绝须向界面传递', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data: null });
    await expect(projectAssignThread('t', '')).rejects.toThrow('任务必须归属项目');
    expect(invoke).not.toHaveBeenCalled();
    await projectAssignThread('t', 'p2');
    expect(invoke).toHaveBeenLastCalledWith('project_assign_thread', { threadId: 't', projectId: 'p2' });
    vi.mocked(invoke).mockResolvedValueOnce({ success: false, error: '任务仍在执行' });
    await expect(projectAssignThread('t', 'p')).rejects.toThrow('任务仍在执行');
  });
  it('保存失败不把未持久化目录放入缓存', async () => {
    const key = 'ui:project-workspace:failed-save';
    const old = parseWorkspaceBinding('/workspace/old');
    rememberWorkspaceBinding(key, old);
    vi.mocked(invoke).mockResolvedValueOnce({ success: false, error: 'database locked' });
    await expect(saveWorkspaceBinding(key, parseWorkspaceBinding('/workspace/new'))).rejects.toThrow();
    expect(peekWorkspaceBinding(key)).toEqual(old);
  });
});
