import { act, createElement, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/notebook', async (original) => ({ ...await original<typeof import('../../../services/notebook')>(), saveNotebookFolder: vi.fn(), moveNotebookNote: vi.fn() }));
import NotebookTree from '../NotebookTree';
import { saveNotebookFolder, moveNotebookNote, type NotebookTree as Tree } from '../../../services/notebook';
import type { Article } from '../../../types';
const initial: Tree = { folders: [{ id: 'a', name: '研究', parentId: null }, { id: 'b', name: '材料', parentId: 'a' }], entries: [{ articleId: 'n', folderId: 'b' }] };
const note = { id: 'n', title: '深层笔记' } as Article;
let host: HTMLDivElement; let root: Root;
const newNote = vi.fn();
const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent === label || node.getAttribute('aria-label') === label)!;
const click = async (label: string) => { await act(async () => button(label).click()); };
function Harness({ searching = false, moving = false }) {
  const [tree, setTree] = useState(initial); const [folder, setFolder] = useState<string | null>(null); const [move, setMove] = useState(moving ? note : null);
  return createElement(NotebookTree, { tree, docs: [note], selectedFolder: folder, selectedNote: null, searching, onSelectFolder: setFolder, onChange: setTree, onNewNote: newNote, renderNote: (a) => createElement('span', null, a.title), moveNote: move, onMoveDone: () => setMove(null), error: null, loading: false, onRetry: vi.fn() });
}
beforeEach(() => {
  vi.clearAllMocks(); Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  HTMLDialogElement.prototype.showModal = function () { this.open = true; };
  HTMLDialogElement.prototype.close = function () { this.open = false; };
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });
describe('多级笔记目录', () => {
  it('逐级展开深层笔记，选中的目录是创建位置', async () => {
    await act(async () => root.render(createElement(Harness)));
    expect(host.textContent).not.toContain('深层笔记');
    await click('文件夹 研究'); await click('文件夹 材料');
    expect(host.textContent).toContain('深层笔记');
    await click('文件夹操作：材料'); await click('新建笔记');
    expect(newNote).toHaveBeenCalledWith('b');
    await click('新建文件夹');
    expect((host.querySelector('select') as HTMLSelectElement).value).toBe('b');
    const input = host.querySelector('input')!;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, '证据'); input.dispatchEvent(new Event('input', { bubbles: true })); });
    vi.mocked(saveNotebookFolder).mockResolvedValue({ ...initial, folders: [...initial.folders, { id: 'c', name: '证据', parentId: 'b' }] });
    await act(async () => host.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    expect(saveNotebookFolder).toHaveBeenCalledWith(null, '证据', 'b');
    expect(host.querySelector('dialog')?.open).toBe(false);
    expect(button('文件夹 证据')).toBeDefined();
  });
  it('搜索直接展示匹配笔记的祖先目录', async () => {
    await act(async () => root.render(createElement(Harness, { searching: true })));
    expect(host.textContent).toContain('深层笔记');
    expect(button('文件夹 研究').getAttribute('aria-expanded')).toBe('true');
  });
  it('移动失败保留原目录并允许重试到根目录', async () => {
    await act(async () => root.render(createElement(Harness, { moving: true })));
    const select = host.querySelector('select')!;
    await act(async () => { select.value = ''; select.dispatchEvent(new Event('change', { bubbles: true })); });
    vi.mocked(moveNotebookNote).mockRejectedValueOnce(new Error('写入失败')).mockResolvedValueOnce({ ...initial, entries: [] });
    await click('移动');
    expect(host.querySelector('dialog')?.open).toBe(true);
    expect(host.querySelector('[role="alert"]')?.textContent).toContain('写入失败');
    await click('移动');
    expect(moveNotebookNote).toHaveBeenLastCalledWith('n', null);
    expect(host.querySelector('dialog')?.open).toBe(false);
    expect(host.querySelector('nav')?.textContent).toContain('深层笔记');
  });
});
