import { describe, expect, it, vi } from 'vitest';
import { DocumentDraftQueue, type DocumentDraftWriters } from '../documentDraftQueue';

function writers(overrides: Partial<DocumentDraftWriters> = {}): DocumentDraftWriters {
  return {
    writeContent: vi.fn().mockResolvedValue(undefined),
    writeTitle: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}

describe('DocumentDraftQueue（事件驱动、按文档隔离保存）', () => {
  it('不同文档各自持有草稿和保存基线，不会串写', async () => {
    const queue = new DocumentDraftQueue();
    queue.seed({ documentId: 'a', markdown: 'A0', title: 'A' });
    queue.seed({ documentId: 'b', markdown: 'B0', title: 'B' });
    queue.update({ documentId: 'a', markdown: 'A1', title: 'A' });
    queue.update({ documentId: 'b', markdown: 'B1', title: 'B' });
    const writeContent = vi.fn().mockResolvedValue(undefined);
    const targetWriters = writers({ writeContent });

    await Promise.all([queue.flush('a', targetWriters), queue.flush('b', targetWriters)]);

    expect(writeContent).toHaveBeenCalledWith('a', 'A1');
    expect(writeContent).toHaveBeenCalledWith('b', 'B1');
    expect(queue.get('a')?.dirty).toBe(false);
    expect(queue.get('b')?.dirty).toBe(false);
  });

  it('同文档写入期间继续输入：复用一个 flush Promise，并在循环内合并到最新 generation', async () => {
    const queue = new DocumentDraftQueue();
    queue.seed({ documentId: 'a', markdown: 'A0', title: 'A' });
    queue.update({ documentId: 'a', markdown: 'A1', title: 'A' });

    let releaseFirst!: () => void;
    const writeContent = vi.fn()
      .mockImplementationOnce(() => new Promise<void>((resolve) => { releaseFirst = resolve; }))
      .mockResolvedValue(undefined);
    const targetWriters = writers({ writeContent });

    const firstFlush = queue.flush('a', targetWriters);
    const sameFlush = queue.flush('a', targetWriters);
    expect(sameFlush).toBe(firstFlush);
    expect(writeContent).toHaveBeenCalledTimes(1);

    queue.update({ documentId: 'a', markdown: 'A2', title: 'A' });
    releaseFirst();
    await firstFlush;

    expect(writeContent).toHaveBeenNthCalledWith(1, 'a', 'A1');
    expect(writeContent).toHaveBeenNthCalledWith(2, 'a', 'A2');
    expect(queue.get('a')).toMatchObject({ markdown: 'A2', dirty: false, error: null });
  });

  it('写入失败保留 dirty 与错误，后续 flush 可重试成功', async () => {
    const queue = new DocumentDraftQueue();
    queue.seed({ documentId: 'a', markdown: 'A0', title: 'A' });
    queue.update({ documentId: 'a', markdown: 'A1', title: 'A' });
    const writeContent = vi.fn()
      .mockRejectedValueOnce(new Error('disk full'))
      .mockResolvedValueOnce(undefined);
    const targetWriters = writers({ writeContent });

    await expect(queue.flush('a', targetWriters)).resolves.toBe(false);
    expect(queue.get('a')).toMatchObject({ dirty: true, error: 'disk full' });

    await expect(queue.flush('a', targetWriters)).resolves.toBe(true);
    expect(queue.get('a')).toMatchObject({ dirty: false, error: null });
    expect(writeContent).toHaveBeenCalledTimes(2);
  });

  it('未变化的文档 flush 不触发任何写入', async () => {
    const queue = new DocumentDraftQueue();
    queue.seed({ documentId: 'a', markdown: 'A0', title: 'A' });
    const targetWriters = writers();

    await expect(queue.flush('a', targetWriters)).resolves.toBe(true);
    expect(targetWriters.writeContent).not.toHaveBeenCalled();
    expect(targetWriters.writeTitle).not.toHaveBeenCalled();
  });
});
