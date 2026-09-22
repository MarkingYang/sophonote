import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { loadNotebookTree, saveNotebookFolder, deleteNotebookFolder, moveNotebookNote, notebookFolderPath, notebookAncestors } from '../notebook';
import { getArticles, insertArticle } from '../tauri';
import type { Article } from '../../types';
const tree = { folders: [{ id: 'a', name: '研究', parentId: null }, { id: 'b', name: '材料', parentId: 'a' }], entries: [] };
beforeEach(() => { vi.clearAllMocks(); vi.mocked(invoke).mockResolvedValue({ success: true, data: tree }); });
describe('笔记目录 invoke 契约', () => {
  it('保持可空根目录和 camelCase 参数', async () => {
    expect(await loadNotebookTree()).toEqual(tree);
    await saveNotebookFolder(null, '研究', null);
    expect(invoke).toHaveBeenLastCalledWith('notebook_folder_save', { id: null, name: '研究', parentId: null });
    await saveNotebookFolder('b', '材料', 'a');
    expect(invoke).toHaveBeenLastCalledWith('notebook_folder_save', { id: 'b', name: '材料', parentId: 'a' });
    await moveNotebookNote('n', null);
    expect(invoke).toHaveBeenLastCalledWith('notebook_note_move', { articleId: 'n', folderId: null });
    await deleteNotebookFolder('a');
    expect(invoke).toHaveBeenLastCalledWith('notebook_folder_delete', { id: 'a' });
  });
  it('错误上抛，创建笔记同时传入归属，完整笔记不受报告数量上限影响', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ success: false, data: null, error: '目录不为空' });
    await expect(deleteNotebookFolder('a')).rejects.toThrow('目录不为空');
    const article = { id: 'n' } as Article;
    await insertArticle(article, 'b');
    expect(invoke).toHaveBeenLastCalledWith('db_insert_article', { article, folderId: 'b' });
    await getArticles(200, true);
    expect(invoke).toHaveBeenLastCalledWith('db_get_articles', { limit: 200, includeNotebook: true });
  });
  it('深层目录路径可用，异常循环不会阻塞界面', () => {
    expect(notebookFolderPath(tree.folders, 'b')).toBe('研究 / 材料');
    expect(notebookAncestors(tree.folders, 'b')).toEqual(['b', 'a']);
    expect(notebookAncestors([{ id: 'a', name: 'a', parentId: 'a' }], 'a')).toEqual(['a']);
  });
});
