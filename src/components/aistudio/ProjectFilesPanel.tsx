import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, ChevronDown, ChevronRight, FileText, Folder, Loader2, Plus, RefreshCw, X } from 'lucide-react';
import { listLocalWorkspaceDirectory, readLocalWorkspaceFile, scanLocalWorkspace, type LocalWorkspaceEntry, type LocalFilePreview } from '../../services/tauri';
import MarkdownView from '../features/MarkdownView';

/** 按展开目录与打开文件读取，不扫描全库或复制项目正文。父组件按 root 隔离生命周期。 */
export default function ProjectFilesPanel({ root, onAttach, onClose }: {
  root: string;
  onAttach: (path: string, name: string) => void;
  onClose: () => void;
}) {
  const [entries, setEntries] = useState<LocalWorkspaceEntry[]>([]);
  const [children, setChildren] = useState<Record<string, LocalWorkspaceEntry[]>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [preview, setPreview] = useState<LocalFilePreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);

  const refresh = async () => {
    const request = ++generation.current;
    setBusy(true); setError(null);
    try {
      const result = await scanLocalWorkspace(root);
      if (request !== generation.current) return;
      setEntries(result.entries); setChildren({}); setExpanded({}); setPreview(null);
    } catch (e) { if (request === generation.current) setError(String(e)); }
    finally { if (request === generation.current) setBusy(false); }
  };
  useEffect(() => { void refresh(); return () => { generation.current += 1; }; }, [root]); // eslint-disable-line react-hooks/exhaustive-deps

  const openEntry = async (entry: LocalWorkspaceEntry) => {
    if (entry.kind === 'directory' && children[entry.path]) {
      setExpanded((old) => ({ ...old, [entry.path]: !old[entry.path] })); return;
    }
    const request = ++generation.current;
    setBusy(true); setError(null);
    try {
      if (entry.kind === 'directory') {
        const result = await listLocalWorkspaceDirectory(root, entry.path);
        if (request !== generation.current) return;
        setChildren((old) => ({ ...old, [entry.path]: result }));
        setExpanded((old) => ({ ...old, [entry.path]: true }));
      } else {
        const result = await readLocalWorkspaceFile(root, entry.path);
        if (request === generation.current) setPreview(result);
      }
    } catch (e) { if (request === generation.current) setError(String(e)); }
    finally { if (request === generation.current) setBusy(false); }
  };
  const renderEntries = (items: LocalWorkspaceEntry[], depth = 0) => items.map((entry) => (
    <div key={entry.path}>
      <button type="button" onClick={() => void openEntry(entry)} className="flex w-full items-center gap-1.5 rounded-md py-2 pr-2 text-left text-xs hover:bg-[var(--bg-sunken)]" style={{ paddingLeft: 8 + depth * 14 }} title={entry.path} aria-expanded={entry.kind === 'directory' ? !!expanded[entry.path] : undefined}>
        {entry.kind === 'directory' ? <>{expanded[entry.path] ? <ChevronDown size={12} /> : <ChevronRight size={12} />}<Folder size={14} className="text-[var(--gold)] shrink-0" /></> : <FileText size={14} className="ml-[18px] shrink-0 text-[var(--text-tertiary)]" />}
        <span className="truncate">{entry.name}</span>
      </button>
      {expanded[entry.path] && renderEntries(children[entry.path] ?? [], depth + 1)}
    </div>
  ));
  return <aside aria-label="项目文件" className="absolute inset-y-0 right-0 z-20 flex w-[min(420px,100%)] flex-col border-l border-[var(--border-default)] bg-[var(--bg-surface)] shadow-lg min-[1400px]:static min-[1400px]:shrink-0 min-[1400px]:shadow-none">
    <header className="flex h-10 shrink-0 items-center gap-2 border-b border-[var(--border-default)] px-3 text-xs">
      {preview && <button type="button" title="返回文件列表" onClick={() => setPreview(null)} className="p-1"><ArrowLeft size={14} /></button>}
      <span className="min-w-0 flex-1 truncate font-medium" title={preview?.path ?? root}>{preview?.path ?? '项目文件'}</span>
      {busy && <Loader2 size={13} className="animate-spin" />}
      <button type="button" title="刷新项目文件" onClick={() => void refresh()} className="p-1"><RefreshCw size={13} /></button>
      <button type="button" title="关闭项目文件" onClick={onClose} className="p-1"><X size={14} /></button>
    </header>
    {error && <p role="alert" className="p-3 text-xs text-[var(--danger)]">{error}</p>}
    {preview ? <>
      <div className="flex items-center justify-between border-b border-[var(--border-default)] px-3 py-2 text-xs">
        <span className="text-[var(--text-tertiary)]">{preview.truncated ? '内容已截断' : '只读预览'}</span>
        <button type="button" onClick={() => onAttach(`${root.replace(/[\\/]$/, '')}/${preview.path}`, preview.path.split(/[\\/]/).pop()!)} className="inline-flex items-center gap-1 text-[var(--accent)]"><Plus size={13} />加入任务</button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-4 text-sm">
        {/\.(?:md|markdown|mdown)$/i.test(preview.path) ? <MarkdownView content={preview.content} lite /> : <pre className="whitespace-pre-wrap break-words text-xs leading-6">{preview.content}</pre>}
      </div>
    </> : <div className="min-h-0 flex-1 overflow-auto p-2 text-[var(--text-secondary)]">
      {renderEntries(entries)}
      {!busy && entries.length === 0 && !error && <p className="p-3 text-xs text-[var(--text-tertiary)]">文件夹为空，任务产物会保存在这里。</p>}
    </div>}
  </aside>;
}
