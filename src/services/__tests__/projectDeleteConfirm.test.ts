import { describe, expect, it } from 'vitest';
import { formatProjectRemoveConfirm } from '../projectDeleteConfirm';

describe('formatProjectRemoveConfirm', () => {
  it('只提示移除工作室关联', () => {
    const r = formatProjectRemoveConfirm('示例项目');
    expect(r.buttonLabel).toBe('确认移除项目？');
    expect(r.warning).toContain('示例项目');
    expect(r.warning).toContain('本地文件与代码不会改变');
    expect(r.warning).not.toContain('永久删除');
  });
});
