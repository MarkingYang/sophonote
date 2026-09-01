import { describe, expect, it } from 'vitest';
import { navItems } from '../Sidebar';
import { pageLoaders } from '../../../services/pagePreload';

describe('一级导航层级', () => {
  it('旧知识库与收件箱均不进入一级导航，计划任务拥有独立入口', () => {
    const ids = navItems.map((item) => item.id);

    expect(ids).not.toContain('library');
    expect(ids).not.toContain('inbox');
    expect(ids).toContain('scheduled-tasks');
    expect(pageLoaders).not.toHaveProperty('library');
    expect(pageLoaders).not.toHaveProperty('inbox');
    expect(pageLoaders).toHaveProperty('scheduled-tasks');
  });
});
