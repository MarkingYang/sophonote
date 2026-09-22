import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ChevronDown, ChevronRight, Folder, FolderOpen, FolderPlus, MoreHorizontal, Pencil, Trash2, X } from 'lucide-react';
import type { Article } from '../../types';
import { deleteNotebookFolder, moveNotebookNote, notebookAncestors, notebookFolderPath, saveNotebookFolder, type NotebookFolder, type NotebookTree as Tree } from '../../services/notebook';
import ContextMenu, { type CtxMenuState } from '../ui/ContextMenu';

interface Props {
  tree: Tree;
  docs: Article[];
  selectedFolder: string | null;
  selectedNote: string | null;
  searching: boolean;
  onSelectFolder: (id: string | null) => void;
  onChange: (tree: Tree) => void;
  onNewNote: (folderId: string) => void;
  renderNote: (article: Article) => ReactNode;
  moveNote: Article | null;
  onMoveDone: () => void;
  error: string | null;
  onRetry: () => void;
  loading: boolean;
}
const actionClass = 'rounded p-1.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--accent)]';

export default function NotebookTree({ tree, docs, selectedFolder, selectedNote, searching, onSelectFolder, onChange, onNewNote, renderNote, moveNote, onMoveDone, error, onRetry, loading }: Props) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [menu, setMenu] = useState<CtxMenuState | null>(null);
  const [edit, setEdit] = useState<{ id: string | null; name: string; parentId: string | null } | null>(null);
  const [destination, setDestination] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const noteFolder = tree.entries.find((entry) => entry.articleId === selectedNote)?.folderId ?? null;
  useEffect(() => {
    setExpanded((old) => new Set([...old, ...notebookAncestors(tree.folders, noteFolder), ...notebookAncestors(tree.folders, selectedFolder)]));
  }, [tree.folders, noteFolder, selectedFolder]);
  useEffect(() => {
    if (edit || moveNote) {
      dialog.current?.showModal();
      dialog.current?.querySelector<HTMLElement>(moveNote ? 'select' : 'input')?.focus();
    } else dialog.current?.close();
  }, [edit != null, moveNote]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (moveNote) { setDestination(tree.entries.find((entry) => entry.articleId === moveNote.id)?.folderId ?? null); setMessage(null); } }, [moveNote]); // eslint-disable-line react-hooks/exhaustive-deps
  const close = () => { if (!busy) { setEdit(null); onMoveDone(); setMessage(null); } };
  const create = (parentId: string | null) => { setMessage(null); setEdit({ id: null, name: '', parentId }); };
  const submit = async () => {
    if (busy) return;
    setBusy(true); setMessage(null);
    try {
      if (moveNote) {
        onChange(await moveNotebookNote(moveNote.id, destination));
        onSelectFolder(destination);
      }
      else if (edit) {
        const next = await saveNotebookFolder(edit.id, edit.name, edit.parentId);
        onChange(next);
        const folder = next.folders.find((f) => edit.id ? f.id === edit.id : f.name === edit.name.trim() && f.parentId === edit.parentId);
        if (folder) onSelectFolder(folder.id);
      }
      setEdit(null); onMoveDone();
    } catch (e) { setMessage(String(e)); }
    finally { setBusy(false); }
  };
  const remove = async (folder: NotebookFolder) => {
    try {
      onChange(await deleteNotebookFolder(folder.id));
      if (selectedFolder === folder.id) onSelectFolder(folder.parentId);
      setMessage(null);
    } catch (e) { setMessage(String(e)); }
  };
  const openMenu = (folder: NotebookFolder, x: number, y: number) => setMenu({ x, y, items: [
    { label: '新建笔记', onClick: () => onNewNote(folder.id) },
    { label: '新建子文件夹', icon: <FolderPlus size={13} />, onClick: () => create(folder.id) },
    { label: '重命名 / 移动', icon: <Pencil size={13} />, onClick: () => { setMessage(null); setEdit({ ...folder }); } },
    { label: '删除空文件夹', icon: <Trash2 size={13} />, danger: true, onClick: () => void remove(folder) },
  ] });
  const notesByFolder = new Map<string | null, Article[]>();
  const assignments = new Map(tree.entries.map((entry) => [entry.articleId, entry.folderId]));
  const visibleFolders = new Set<string>();
  for (const doc of docs) {
    const folderId = assignments.get(doc.id) ?? null;
    const notes = notesByFolder.get(folderId) ?? [];
    notes.push(doc);
    notesByFolder.set(folderId, notes);
    if (searching) for (const id of notebookAncestors(tree.folders, folderId)) visibleFolders.add(id);
  }
  const renderLevel = (parentId: string | null, ancestors: string[] = []): ReactNode => <ul className={parentId ? 'ml-3 border-l border-[var(--border-default)] pl-1' : 'space-y-1'}>
    {tree.folders.filter((f) => f.parentId === parentId && !ancestors.includes(f.id) && (!searching || visibleFolders.has(f.id))).map((folder) => {
      const open = searching || expanded.has(folder.id);
      return <li key={folder.id}>
        <div className={`group flex items-center rounded-md ${selectedFolder === folder.id ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-secondary)]'}`} onContextMenu={(event) => { event.preventDefault(); openMenu(folder, event.clientX, event.clientY); }}>
          <button type="button" className="flex min-w-0 flex-1 items-center gap-1.5 py-2 pl-1 text-left text-xs" aria-expanded={open} aria-label={`文件夹 ${folder.name}`} title={notebookFolderPath(tree.folders, folder.id)} onClick={() => {
            onSelectFolder(folder.id);
            setExpanded((old) => { const next = new Set(old); if (next.has(folder.id)) next.delete(folder.id); else next.add(folder.id); return next; });
          }}>
            {open ? <ChevronDown size={12} className="shrink-0" /> : <ChevronRight size={12} className="shrink-0" />}
            {open ? <FolderOpen size={14} className="shrink-0 text-[var(--gold)]" /> : <Folder size={14} className="shrink-0 text-[var(--gold)]" />}
            <span className="truncate">{folder.name}</span>
          </button>
          <button type="button" aria-label={`文件夹操作：${folder.name}`} className={`${actionClass} opacity-0 group-hover:opacity-100 focus:opacity-100`} onClick={(event) => { const rect = event.currentTarget.getBoundingClientRect(); openMenu(folder, rect.right, rect.bottom); }}><MoreHorizontal size={13} /></button>
        </div>
        {open && renderLevel(folder.id, [...ancestors, folder.id])}
        {open && !(notesByFolder.get(folder.id)?.length) && !tree.folders.some((f) => f.parentId === folder.id) && <p className="py-2 pl-8 text-[11px] text-[var(--text-tertiary)]">空文件夹</p>}
      </li>;
    })}
    {(notesByFolder.get(parentId) ?? []).map((doc) => <li key={doc.id}>{renderNote(doc)}</li>)}
  </ul>;
  return <>
    <div className="flex items-center gap-1 pb-2">
      <button type="button" onClick={() => onSelectFolder(null)} aria-pressed={selectedFolder == null} className={`min-w-0 flex-1 truncate rounded px-2 py-1.5 text-left text-xs ${selectedFolder == null ? 'text-[var(--accent)] bg-[var(--accent-subtle)]' : 'text-[var(--text-tertiary)]'}`} title={notebookFolderPath(tree.folders, selectedFolder)}>{selectedFolder ? notebookFolderPath(tree.folders, selectedFolder) : '全部笔记'}</button>
      <button type="button" aria-label="新建文件夹" title="新建文件夹" disabled={loading || !!error} className={actionClass} onClick={() => create(selectedFolder)}><FolderPlus size={15} /></button>
    </div>
    {(error || (message && !edit && !moveNote)) && <p role="alert" className="p-2 text-xs text-[var(--danger)]">{error || message}{error && <button className="ml-2 underline" onClick={onRetry}>重试</button>}</p>}
    {loading ? <p className="p-2 text-xs text-[var(--text-tertiary)]">加载目录…</p> : !error && <nav aria-label="笔记目录树">{renderLevel(null)}</nav>}
    <ContextMenu menu={menu} onClose={() => setMenu(null)} />
    <dialog ref={dialog} aria-label={moveNote ? '移动笔记' : edit?.id ? '编辑文件夹' : '新建文件夹'} onCancel={(e) => { e.preventDefault(); close(); }} className="m-auto w-96 max-w-[90vw] rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5 text-[var(--text-primary)] shadow-xl backdrop:bg-black/30">
      <form onSubmit={(e) => { e.preventDefault(); void submit(); }} className="space-y-4">
        <header className="flex items-center"><h3 className="flex-1 text-sm font-semibold">{moveNote ? '移动笔记' : edit?.id ? '编辑文件夹' : '新建文件夹'}</h3><button type="button" aria-label="关闭文件夹操作" disabled={busy} onClick={close} className={actionClass}><X size={15} /></button></header>
        {moveNote ? <p className="truncate text-xs text-[var(--text-secondary)]">{moveNote.title}</p> : <label className="block space-y-2 text-xs"><span>文件夹名称</span><input autoFocus required maxLength={120} aria-label="文件夹名称" value={edit?.name ?? ''} onChange={(e) => setEdit((old) => old && ({ ...old, name: e.target.value }))} className="w-full rounded-md border border-[var(--border-default)] bg-transparent p-2 outline-none focus:border-[var(--accent)]" /></label>}
        <label className="block space-y-2 text-xs"><span>所在目录</span><select aria-label="所在目录" value={(moveNote ? destination : edit?.parentId) ?? ''} onChange={(e) => { const parentId = e.target.value || null; if (moveNote) setDestination(parentId); else setEdit((old) => old && ({ ...old, parentId })); }} className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] p-2"><option value="">全部笔记（根目录）</option>{tree.folders.filter((f) => !edit?.id || !notebookAncestors(tree.folders, f.id).includes(edit.id)).map((f) => <option key={f.id} value={f.id}>{notebookFolderPath(tree.folders, f.id)}</option>)}</select></label>
        {message && <p role="alert" className="text-xs text-[var(--danger)]">{message}</p>}
        <footer className="flex justify-end gap-2 text-xs"><button type="button" disabled={busy} onClick={close} className="rounded-md px-3 py-2 hover:bg-[var(--bg-sunken)]">取消</button><button type="submit" disabled={busy || (!moveNote && !edit?.name.trim())} className="btn-primary rounded-md px-3 py-2 disabled:opacity-40">{busy ? '保存中…' : moveNote ? '移动' : '保存'}</button></footer>
      </form>
    </dialog>
  </>;
}
