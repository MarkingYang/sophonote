import { describe, expect, it } from 'vitest';
import { parseWorkspaceBinding, peekWorkspaceBinding, rememberWorkspaceBinding, withWorkspacePermission } from '../workspaceBinding';

describe('workspaceBinding', () => {
  it('兼容旧版纯路径设置，并默认询问权限', () => {
    expect(parseWorkspaceBinding('/tmp/example')).toMatchObject({
      root: '/tmp/example',
      name: 'example',
      kind: 'folder',
      permissionMode: 'ask',
    });
  });

  it('恢复结构化绑定并安全回退未知权限模式', () => {
    const binding = parseWorkspaceBinding(JSON.stringify({
      version: 1,
      root: '/tmp/repo',
      name: 'repo',
      kind: 'git',
      permissionMode: 'unsafe',
      authorizedAt: '2026-08-20T00:00:00.000Z',
    }));
    expect(binding).toMatchObject({ kind: 'git', permissionMode: 'ask' });
    expect(binding && withWorkspacePermission(binding, 'plan').permissionMode).toBe('plan');
  });

  it('记住绑定后可同步读出，避免热切换先清空再填', () => {
    expect(peekWorkspaceBinding('ui:thread-workspace:t1')).toBeUndefined();
    rememberWorkspaceBinding('ui:thread-workspace:t1', null);
    expect(peekWorkspaceBinding('ui:thread-workspace:t1')).toBeNull();
    const binding = parseWorkspaceBinding('/tmp/example');
    rememberWorkspaceBinding('ui:thread-workspace:t1', binding);
    expect(peekWorkspaceBinding('ui:thread-workspace:t1')?.root).toBe('/tmp/example');
  });
});
