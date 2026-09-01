import { describe, expect, it } from 'vitest';
import { getTool, toolRegistry } from '../registry';

/**
 * 工具注册表契约测试（DEC-041）：
 * 壳层与导航状态完全由注册表驱动，这里守住注册数据的最小完整性。
 */

describe('toolRegistry', () => {
  it('工具 id 全局唯一', () => {
    const ids = toolRegistry.map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('每个工具的展示与渲染字段完整', () => {
    for (const tool of toolRegistry) {
      expect(tool.id, 'id 不能为空').toBeTruthy();
      expect(tool.title, `${tool.id} 缺标题`).toBeTruthy();
      expect(tool.description, `${tool.id} 缺描述`).toBeTruthy();
      expect(tool.icon, `${tool.id} 缺图标`).toBeDefined();
      expect(tool.category, `${tool.id} 缺分类`).toBeTruthy();
      expect(tool.keywords.length, `${tool.id} 缺关键词`).toBeGreaterThan(0);
      expect(tool.Component, `${tool.id} 缺整页组件`).toBeDefined();
    }
  });

  it('内置三个首发工具都已注册', () => {
    const ids = toolRegistry.map((t) => t.id);
    expect(ids).toEqual(expect.arrayContaining(['today', 'tasks', 'pomodoro']));
  });
});

describe('getTool', () => {
  it('按 id 返回工具定义', () => {
    expect(getTool('today')?.title).toBe('今日');
    expect(getTool('pomodoro')?.category).toBe('专注节奏');
  });

  it('未知 id 或 null 返回 undefined（壳层回退到画廊）', () => {
    expect(getTool('not-exist')).toBeUndefined();
    expect(getTool(null)).toBeUndefined();
  });
});
