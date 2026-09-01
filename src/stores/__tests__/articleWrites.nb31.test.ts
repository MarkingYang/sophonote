/**
 * NB-31：appStore 四个文章写动作的失败契约测试（审计 P0-1 整改要求 1/2/4/5）。
 *
 * 契约：
 * - 正文/标题修改：保留乐观内存更新，但后端失败必须向调用方抛出；
 *   改名失败不得执行双链级联改写。
 * - 新建/删除：后端成功后才变更内存列表；失败抛出且不产生幽灵记录。
 *
 * tauri 服务层整体 mock（其本身已对 !res.success 抛错，此处只验证 store 传播）。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import type { Article, Item } from '../../types';

const tauriMock = vi.hoisted(() => ({
  insertArticle: vi.fn(),
  updateArticle: vi.fn(),
  renameArticle: vi.fn(),
  deleteArticle: vi.fn(),
}));

vi.mock('../../services/tauri', () => tauriMock);

import { useAppStore } from '../appStore';

const mkArticle = (over: Partial<Article> = {}): Article => ({
  id: 'a1',
  title: '文档A',
  content: '内容A',
  articleType: 'manual',
  edited: false,
  createdAt: '2026-08-08T00:00:00.000Z',
  ...over,
});

beforeEach(() => {
  // 冻结 5s 防抖索引定时器：测试不触发嵌入/索引调度（fake timers 下永不到点）
  vi.useFakeTimers();
  tauriMock.insertArticle.mockReset().mockResolvedValue(undefined);
  tauriMock.updateArticle.mockReset().mockResolvedValue(undefined);
  tauriMock.renameArticle.mockReset().mockResolvedValue(undefined);
  tauriMock.deleteArticle.mockReset().mockResolvedValue(undefined);
  useAppStore.setState({ articles: [] });
  useAppStore.setState({ items: [] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('appStore.updateArticleContent（NB-31）', () => {
  it('成功：落库调用发生，内存内容更新', async () => {
    useAppStore.setState({ articles: [mkArticle()] });
    await useAppStore.getState().updateArticleContent('a1', '新内容');
    expect(tauriMock.updateArticle).toHaveBeenCalledWith('a1', '新内容');
    expect(useAppStore.getState().articles[0].content).toBe('新内容');
  });

  it('失败：必须向调用方抛出；内存保留乐观更新（编辑器当下真相），由调用方保留 dirty 重试', async () => {
    useAppStore.setState({ articles: [mkArticle()] });
    tauriMock.updateArticle.mockRejectedValueOnce(new Error('write denied'));
    await expect(useAppStore.getState().updateArticleContent('a1', '新内容')).rejects.toThrow('write denied');
    expect(useAppStore.getState().articles[0].content).toBe('新内容');
  });
});

describe('appStore.updateArticleTitle（NB-31）', () => {
  it('成功：改名落库，且双链级联改写被调用', async () => {
    useAppStore.setState({
      articles: [mkArticle(), mkArticle({ id: 'b1', title: '文档B', content: '链接 [[文档A]] 结束' })],
    });
    await useAppStore.getState().updateArticleTitle('a1', '新名字');
    expect(tauriMock.renameArticle).toHaveBeenCalledWith('a1', '新名字');
    expect(tauriMock.updateArticle).toHaveBeenCalledWith('b1', '链接 [[新名字]] 结束');
  });

  it('失败：必须抛出，且绝不执行双链级联（改名未生效，改别人只会制造不一致）', async () => {
    useAppStore.setState({
      articles: [mkArticle(), mkArticle({ id: 'b1', title: '文档B', content: '链接 [[文档A]] 结束' })],
    });
    tauriMock.renameArticle.mockRejectedValueOnce(new Error('rename denied'));
    await expect(useAppStore.getState().updateArticleTitle('a1', '新名字')).rejects.toThrow('rename denied');
    expect(tauriMock.updateArticle).not.toHaveBeenCalled();
    expect(useAppStore.getState().articles.find((a) => a.id === 'b1')?.content).toBe('链接 [[文档A]] 结束');
  });
});

describe('appStore.saveArticle（NB-31）', () => {
  it('成功：记录进入内存列表', async () => {
    const a = mkArticle({ id: 'n1', title: '新笔记' });
    await useAppStore.getState().saveArticle(a);
    expect(tauriMock.insertArticle).toHaveBeenCalledWith(a);
    expect(useAppStore.getState().articles.some((x) => x.id === 'n1')).toBe(true);
  });

  it('失败：必须抛出，内存列表不得出现幽灵记录（调用方据此不跳转）', async () => {
    tauriMock.insertArticle.mockRejectedValueOnce(new Error('insert failed'));
    await expect(useAppStore.getState().saveArticle(mkArticle({ id: 'n1' }))).rejects.toThrow('insert failed');
    expect(useAppStore.getState().articles).toHaveLength(0);
  });
});

describe('appStore.deleteArticle（NB-31）', () => {
  it('成功：记录从内存列表移除', async () => {
    useAppStore.setState({ articles: [mkArticle()] });
    await useAppStore.getState().deleteArticle('a1');
    expect(tauriMock.deleteArticle).toHaveBeenCalledWith('a1');
    expect(useAppStore.getState().articles).toHaveLength(0);
  });

  it('失败：必须抛出，记录保留在列表（防假删除）', async () => {
    useAppStore.setState({ articles: [mkArticle()] });
    tauriMock.deleteArticle.mockRejectedValueOnce(new Error('delete failed'));
    await expect(useAppStore.getState().deleteArticle('a1')).rejects.toThrow('delete failed');
    expect(useAppStore.getState().articles).toHaveLength(1);
  });
});

describe('发现详情按需缓存（ISSUE-044）', () => {
  it('分页发现条目可注入首页有界 items 缓存，详情不再因找不到 item 而空白', () => {
    const historical: Item = {
      id: 'historical-item',
      sourceId: 'aihot',
      type: 'article',
      title: '历史高质量动态',
      url: 'https://example.com/item',
      description: '已过滤且有深度解读',
      publishedAt: '2026-01-01T00:00:00Z',
      fetchedAt: '2026-01-01T00:00:00Z',
      status: 'unread',
    };
    useAppStore.setState({ items: [] });
    useAppStore.getState().addItems([historical]);
    expect(useAppStore.getState().items.find((item) => item.id === historical.id)).toEqual(historical);
  });

  it('按 itemId 精确读取的深度文章可新增并覆盖缓存记录', () => {
    const first = mkArticle({ id: 'deep-1', itemId: 'historical-item', articleType: 'deep-dive', content: '旧正文' });
    const latest = { ...first, content: 'Markdown 完整正文' };
    useAppStore.getState().upsertArticle(first);
    useAppStore.getState().upsertArticle(latest);
    expect(useAppStore.getState().articles).toHaveLength(1);
    expect(useAppStore.getState().articles[0].content).toBe('Markdown 完整正文');
  });
});
