/**
 * NB-31：flushDocument 纯函数契约测试。
 * 覆盖审计 P0-1 的核心要求：后端写失败时不得推进已保存基线（防假保存）。
 */
import { describe, it, expect, vi } from 'vitest';
import { flushDocument, messageOf } from '../noteSave';

const input = (over: Partial<Parameters<typeof flushDocument>[0]> = {}) => ({
  md: 'new-md',
  title: '新标题',
  lastSavedMd: 'old-md',
  savedTitle: '旧标题',
  writeContent: vi.fn().mockResolvedValue(undefined),
  writeTitle: vi.fn().mockResolvedValue(undefined),
  ...over,
});

describe('noteSave.flushDocument（NB-31 保存失败契约）', () => {
  it('无变化：不触发任何写入，状态 unchanged', async () => {
    const i = input({ md: 'same', title: '同标题', lastSavedMd: 'same', savedTitle: '同标题' });
    const out = await flushDocument(i);
    expect(out.status).toBe('unchanged');
    expect(i.writeContent).not.toHaveBeenCalled();
    expect(i.writeTitle).not.toHaveBeenCalled();
    expect(out.lastSavedMd).toBe('same');
  });

  it('正文+标题都成功：基线全部推进，且正文先于标题写入', async () => {
    const order: string[] = [];
    const i = input({
      writeContent: vi.fn(async () => { order.push('content'); }),
      writeTitle: vi.fn(async () => { order.push('title'); }),
    });
    const out = await flushDocument(i);
    expect(out.status).toBe('saved');
    expect(out.lastSavedMd).toBe('new-md');
    expect(out.savedTitle).toBe('新标题');
    expect(order).toEqual(['content', 'title']);
  });

  it('正文写失败：整体 error，正文基线不推进，且绝不尝试写标题（防半成功）', async () => {
    const i = input({ writeContent: vi.fn().mockRejectedValue(new Error('disk full')) });
    const out = await flushDocument(i);
    expect(out.status).toBe('error');
    expect(out.error).toBe('disk full');
    expect(out.lastSavedMd).toBe('old-md');
    expect(out.savedTitle).toBe('旧标题');
    expect(i.writeTitle).not.toHaveBeenCalled();
  });

  it('正文成功、标题失败：正文基线推进，标题基线不推进，整体 error', async () => {
    const i = input({ writeTitle: vi.fn().mockRejectedValue(new Error('rename denied')) });
    const out = await flushDocument(i);
    expect(out.status).toBe('error');
    expect(out.error).toBe('rename denied');
    expect(out.lastSavedMd).toBe('new-md');
    expect(out.savedTitle).toBe('旧标题');
  });

  it('仅正文变化且成功：不调用标题写入', async () => {
    const i = input({ title: '旧标题' });
    const out = await flushDocument(i);
    expect(out.status).toBe('saved');
    expect(i.writeTitle).not.toHaveBeenCalled();
  });

  it('messageOf 对非 Error 异常归一为字符串', () => {
    expect(messageOf('boom')).toBe('boom');
    expect(messageOf(new Error('x'))).toBe('x');
  });
});
