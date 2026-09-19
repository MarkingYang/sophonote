import { confirmDiscard } from '../../services/confirmDiscard';
import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, Bot, Check, FileText, Folder, Loader2, RefreshCw, Save, UserRound } from 'lucide-react';
import { listOpenVikingMemories, readOpenVikingMemory, writeOpenVikingMemory, type OpenVikingMemoryDocument, type OpenVikingMemoryPage } from '../../services/tauri';

const ROOT = 'viking://~/memories';
const SCOPES = [{uri: ROOT, label:'用户记忆'}, ...[{id:'hermes',name:'Hermes'}, {id:'pi',name:'Pi'}, {id:'claude_code',name:'Claude Code'}, {id:'opencode',name:'OpenCode'}].map((e) => ({uri:`viking://~/peers/${e.id}/memories`,label:`${e.name} 记忆`}))];
const buttonClass = 'inline-flex items-center gap-1.5 rounded-lg border border-[var(--border-default)] px-3 py-2 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40';
const errorText = (e: unknown) => e instanceof Error ? e.message : '云端记忆请求失败';

export default function OpenVikingMemoryBrowser({ enabled, refreshToken, onDirtyChange }: { enabled: boolean; refreshToken: number; onDirtyChange?: (dirty: boolean) => void }) {
  const [folder, setFolder] = useState(ROOT);
  const rootUri = SCOPES.find((scope) => folder === scope.uri || folder.startsWith(`${scope.uri}/`))?.uri ?? ROOT;
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<OpenVikingMemoryPage | null>(null);
  const [document, setDocument] = useState<OpenVikingMemoryDocument | null>(null);
  const [draft, setDraft] = useState('');
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const sequence = useRef(0);
  const inFlight = useRef(false);
  const dirty = !!document && draft !== document.content;
  useEffect(() => { onDirtyChange?.(dirty); return () => onDirtyChange?.(false); }, [dirty, onDirtyChange]);
  const canLeave = async () => !dirty || await confirmDiscard();

  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  const loadFolder = async (uri: string, nextOffset = 0) => {
    if (inFlight.current || !(await canLeave()) || inFlight.current) return;
    inFlight.current = true;
    const seq = ++sequence.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const value = await listOpenVikingMemories(uri, nextOffset);
      if (sequence.current !== seq) return;
      setFolder(uri); setOffset(nextOffset); setPage(value); setDocument(null); setDraft(''); setEditing(false);
    } catch (e) { if (sequence.current === seq) setError(errorText(e)); }
    finally { if (sequence.current === seq) { setBusy(false); inFlight.current = false; } }
  };
  useEffect(() => {
    setPage(null); setDocument(null); setDraft(''); setEditing(false);
    if (enabled) void loadFolder(ROOT);
    return () => { sequence.current++; inFlight.current = false; };
    // Refresh only on saved credential changes; never discard an editor draft on render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, refreshToken]);

  const read = async (uri: string) => {
    if (inFlight.current || !(await canLeave()) || inFlight.current) return;
    inFlight.current = true;
    const seq = ++sequence.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const value = await readOpenVikingMemory(uri);
      if (seq !== sequence.current) return;
      setDocument(value); setDraft(value.content); setEditing(false);
    } catch (e) { if (seq === sequence.current) setError(errorText(e)); }
    finally { if (seq === sequence.current) { setBusy(false); inFlight.current = false; } }
  };
  const save = async () => {
    if (!document || inFlight.current || !dirty) return;
    inFlight.current = true;
    const seq = ++sequence.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const saved = await writeOpenVikingMemory(document.uri, draft, document.revision);
      if (seq !== sequence.current) return;
      setDocument(saved); setDraft(saved.content); setEditing(false); setNotice('已保存到云端，检索索引由服务异步更新。');
    } catch (e) { if (seq === sequence.current) setError(errorText(e)); }
    finally { if (seq === sequence.current) { setBusy(false); inFlight.current = false; } }
  };

  return <section className="overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]">
    <div className="flex items-center justify-between gap-3 border-b border-[var(--border-default)] px-4 py-3">
      <div><h3 className="text-sm font-semibold">云端记忆</h3><p className="mt-1 text-xs text-[var(--text-tertiary)]">当前 API Key 对应用户的长期记忆，可浏览、读取和修改。</p></div>
      <button type="button" className={buttonClass} disabled={!enabled || busy} onClick={() => void loadFolder(folder, offset)}><RefreshCw size={13} />刷新目录</button>
    </div>
    {!enabled ? <p className="p-6 text-sm text-[var(--text-tertiary)]">保存 API Key 后即可读取云端记忆。</p> : <>
      {error && <div role="alert" className="m-4 rounded-lg bg-[var(--danger-subtle)] p-3 text-sm text-[var(--danger)]">{error}<p className="mt-1 text-xs">{dirty ? '你的修改仍保留在编辑框中。' : '检查服务状态后可重试，已有记忆不会转存到本地。'}</p></div>}
      {notice && <p role="status" className="px-4 pt-3 text-xs text-[var(--success)]">{notice}</p>}
      <div className="grid min-h-72 md:grid-cols-[240px_minmax(0,1fr)]">
        <nav aria-label="云端记忆目录" className="min-w-0 border-b border-[var(--border-default)] p-4 md:border-b-0 md:border-r">
          <div className="mb-4">
            <p className="mb-2 px-2 text-xs font-medium text-[var(--text-tertiary)]">记忆来源</p>
            <div role="group" aria-label="切换记忆来源" className="grid gap-1">
              {SCOPES.map((scope) => {
                const selected = rootUri === scope.uri;
                const Icon = scope.uri === ROOT ? UserRound : Bot;
                return <button key={scope.uri} type="button" disabled={busy} aria-pressed={selected} onClick={() => void loadFolder(scope.uri)} className={`flex h-10 w-full min-w-0 items-center gap-2.5 rounded-lg px-3 text-left text-[13px] leading-5 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] disabled:opacity-40 ${selected ? 'bg-[var(--accent-subtle)] font-medium text-[var(--accent)]' : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'}`}>
                  <Icon size={15} aria-hidden="true" className="shrink-0" />
                  <span className="flex-1 whitespace-nowrap">{scope.label}</span>
                  <Check size={14} aria-hidden="true" className={`shrink-0 ${selected ? '' : 'invisible'}`} />
                </button>;
              })}
            </div>
          </div>
          <div className="mb-2 flex min-w-0 items-center gap-2 border-t border-[var(--border-default)] pt-4">
            <button type="button" className="rounded p-1 disabled:opacity-30" aria-label="上级记忆目录" disabled={folder === rootUri || busy} onClick={() => void loadFolder(folder.slice(0, folder.lastIndexOf('/')))}><ArrowLeft size={14} /></button>
            <span title={folder} className="truncate text-xs text-[var(--text-tertiary)]">{folder === rootUri ? '全部记忆' : folder.slice(rootUri.length + 1)}</span>
          </div>
          <div className="max-h-96 space-y-1 overflow-y-auto">
            {page?.entries.map((entry) => <button key={entry.uri} type="button" disabled={busy} onClick={() => void (entry.isDirectory ? loadFolder(entry.uri) : read(entry.uri))} title={entry.name} className={`flex min-h-9 w-full min-w-0 items-center gap-2 rounded-lg px-2 py-2 text-left text-xs hover:bg-[var(--bg-sunken)] disabled:opacity-40 ${document?.uri === entry.uri ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-secondary)]'}`}>
              {entry.isDirectory ? <Folder size={14} className="shrink-0" /> : <FileText size={14} className="shrink-0" />}<span className="min-w-0 truncate">{entry.name}</span>
            </button>)}
            {page && !page.entries.length && <p className="p-2 text-xs text-[var(--text-tertiary)]">此目录暂无记忆。</p>}
          </div>
          {page && (offset > 0 || page.hasMore) && <div className="mt-3 flex justify-between text-xs"><button disabled={busy || offset === 0} onClick={() => void loadFolder(folder, Math.max(0, offset - 100))}>上一页</button><span>{offset / 100 + 1}</span><button disabled={busy || !page.hasMore} onClick={() => void loadFolder(folder, offset + 100)}>下一页</button></div>}
        </nav>
        <div className="min-w-0 p-4">
          {busy && <p role="status" className="mb-3 flex items-center gap-2 text-xs text-[var(--text-tertiary)]"><Loader2 size={14} className="animate-spin" />{editing ? '正在检查并保存…' : '正在读取云端…'}</p>}
          {document ? <>
            <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
              <h4 className="break-all text-sm font-medium">{document.uri.split('/').slice(-1)[0]}{dirty && <span className="ml-2 text-xs text-[var(--warning)]">未保存</span>}</h4>
              <div className="flex gap-2">{editing ? <>
                <button className={buttonClass} disabled={busy} onClick={async () => { if (await canLeave()) { setDraft(document.content); setEditing(false); } }}>取消编辑</button>
                <button className={buttonClass} disabled={busy || !dirty} onClick={() => void save()}><Save size={13} />保存到云端</button>
              </> : <button className={buttonClass} disabled={busy} onClick={() => setEditing(true)}>编辑记忆</button>}
                <button className={buttonClass} disabled={busy} onClick={() => void read(document.uri)}>重新读取</button>
              </div>
            </div>
            {editing ? <textarea aria-label="记忆正文" value={draft} disabled={busy} onChange={(e) => setDraft(e.target.value)} className="min-h-72 w-full resize-y rounded-lg border border-[var(--border-strong)] bg-[var(--bg-sunken)] p-3 font-mono text-sm leading-6 outline-none focus:border-[var(--accent)]" /> : <pre className="max-h-[32rem] overflow-y-auto whitespace-pre-wrap break-words font-sans text-sm leading-7 text-[var(--text-secondary)]">{document.content || '（空记忆）'}</pre>}
          </> : !busy && <p className="flex min-h-48 items-center justify-center text-sm text-[var(--text-tertiary)]">选择一条记忆查看内容</p>}
        </div>
      </div>
    </>}
  </section>;
}
