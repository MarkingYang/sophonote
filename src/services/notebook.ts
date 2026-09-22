import { invoke } from '@tauri-apps/api/core';

export interface NotebookFolder { id: string; name: string; parentId: string | null }
export interface NotebookEntry { articleId: string; folderId: string }
export interface NotebookTree { folders: NotebookFolder[]; entries: NotebookEntry[] }
export const emptyNotebookTree: NotebookTree = { folders: [], entries: [] };

async function request(command: string, args?: Record<string, unknown>): Promise<NotebookTree> {
  const result = await invoke<{ success: boolean; data: NotebookTree | null; error: string | null }>(command, args);
  if (!result.success || !result.data) throw new Error(result.error || '目录读取失败');
  return result.data;
}
export const loadNotebookTree = () => request('notebook_tree');
export const saveNotebookFolder = (id: string | null, name: string, parentId: string | null) => request('notebook_folder_save', { id, name, parentId });
export const deleteNotebookFolder = (id: string) => request('notebook_folder_delete', { id });
export const moveNotebookNote = (articleId: string, folderId: string | null) => request('notebook_note_move', { articleId, folderId });

/** 全路径用于移动选择器；有界祖先遍历也保护旧数据的异常关系。 */
export function notebookFolderPath(folders: NotebookFolder[], id: string | null): string {
  const names: string[] = [];
  const seen = new Set<string>();
  while (id && !seen.has(id)) {
    seen.add(id);
    const folder = folders.find((item) => item.id === id);
    if (!folder) break;
    names.unshift(folder.name);
    id = folder.parentId;
  }
  return names.join(' / ') || '全部笔记';
}
export function notebookAncestors(folders: NotebookFolder[], id: string | null): string[] {
  const ids: string[] = [];
  while (id && !ids.includes(id)) {
    ids.push(id);
    id = folders.find((item) => item.id === id)?.parentId ?? null;
  }
  return ids;
}
