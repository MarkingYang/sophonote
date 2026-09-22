import { lazy, Suspense, useEffect, useRef, useState, type ReactNode } from 'react';
import { Check, ChevronDown, FolderOpen, Globe2, Loader2, MessageSquareText, PanelRightClose, PanelsTopLeft, Terminal, Trash2, X } from 'lucide-react';
import { getSetting, updateSetting } from '../../services/tauri';
import type { AgentAttachmentInput } from '../../stores/agentStore';
import type { WorkspacePermissionMode } from '../../services/workspaceBinding';
import VerticalResizeHandle from '../ui/VerticalResizeHandle';
import { usePageSurfaceActive } from '../layout/KeptAlivePage';
const ProjectFilesPanel = lazy(() => import('./ProjectFilesPanel'));
const AgentBrowserPanel = lazy(() => import('./AgentBrowserPanel'));
const LocalTerminal = lazy(() => import('./LocalTerminal'));

const WIDTH_KEY = 'ui:project-extension-width';
const options = [{ id: 'files', label: '文件夹', icon: FolderOpen }, { id: 'browser', label: '浏览器', icon: Globe2 }, { id: 'terminal', label: '终端', icon: Terminal }] as const;
type Extension = typeof options[number]['id'];
const iconButton = 'inline-flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-primary)]';

/** 只拥有布局；Chat 始终保留挂载，扩展不会重建 Thread 或取消 Run。 */
export default function ProjectExtensions({ root, permissionMode, onAttach, children }: {
  root: string;
  permissionMode: WorkspacePermissionMode;
  onAttach: (attachment: AgentAttachmentInput) => void;
  children: ReactNode;
}) {
  const pageActive = usePageSurfaceActive();
  const [extension, setExtension] = useState<Extension | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [width, setWidth] = useState(440);
  const [availableWidth, setAvailableWidth] = useState(1000);
  const [dragging, setDragging] = useState(false);
  const [clearToken, setClearToken] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const host = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const preferenceChanged = useRef(false);
  useEffect(() => {
    if (!pageActive && extension === 'terminal') setExtension(null);
  }, [pageActive, extension]);
  useEffect(() => {
    let alive = true;
    void getSetting(WIDTH_KEY).then((raw) => {
      const value = Number(raw);
      if (alive && !preferenceChanged.current && raw && Number.isFinite(value)) setWidth(Math.max(300, Math.min(1200, value)));
    }).catch(() => {});
    return () => { alive = false; };
  }, []);
  useEffect(() => {
    const node = host.current;
    if (!node) return;
    const observer = new ResizeObserver(([entry]) => setAvailableWidth(Math.round(entry.contentRect.width)));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!menuOpen) return;
    menu.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"]')?.focus();
    const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) setMenuOpen(false); };
    window.addEventListener('pointerdown', outside);
    return () => window.removeEventListener('pointerdown', outside);
  }, [menuOpen]);
  const overlay = availableWidth < 720;
  const maxWidth = Math.max(240, overlay ? availableWidth - 24 : availableWidth - 360);
  const minWidth = Math.min(300, maxWidth);
  const visibleWidth = Math.min(width, maxWidth);
  const active = options.find((item) => item.id === extension);
  const close = () => { setExtension(null); setMenuOpen(false); setError(null); trigger.current?.focus(); };
  const changeWidth = (value: number) => { preferenceChanged.current = true; setWidth(value); };
  const addFile = (path: string, name: string) => onAttach({ id: crypto.randomUUID(), kind: 'file', path, name });

  return <div ref={host} className="flex h-full min-h-0 min-w-0 flex-1 flex-col">
    <header className="flex h-9 shrink-0 items-center gap-2 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-3">
      <span className="inline-flex h-7 items-center gap-1.5 rounded-md bg-[var(--accent-subtle)] px-2 text-xs font-medium text-[var(--accent)]"><MessageSquareText size={12} />AI 对话</span>
      <div ref={menu} className="relative ml-auto" onKeyDown={(event) => {
        if (event.key === 'Escape') { setMenuOpen(false); trigger.current?.focus(); }
        if (menuOpen && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
          event.preventDefault();
          const buttons = [...menu.current!.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
          const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
          buttons[(index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus();
        }
      }}>
        <button ref={trigger} type="button" aria-label="扩展" aria-haspopup="menu" aria-expanded={menuOpen} onClick={() => setMenuOpen(!menuOpen)} className={`inline-flex h-7 items-center gap-1.5 rounded-md px-2 text-xs ${extension ? 'text-[var(--accent)]' : 'text-[var(--text-secondary)]'} hover:bg-[var(--bg-sunken)]`}><PanelsTopLeft size={13} />扩展<ChevronDown size={11} /></button>
        {menuOpen && <div role="menu" aria-label="项目扩展" className="absolute right-0 top-full z-40 mt-1 w-40 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-1 shadow-lg">
          {options.map(({ id, label, icon: Icon }) => <button key={id} type="button" role="menuitemradio" aria-checked={extension === id} onClick={() => { setExtension(id); setMenuOpen(false); setError(null); trigger.current?.focus(); }} className="flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] focus:bg-[var(--bg-sunken)]"><Icon size={14} /><span className="flex-1">{label}</span>{extension === id && <Check size={12} className="text-[var(--accent)]" />}</button>)}
          {extension && <button type="button" role="menuitem" onClick={close} className="mt-1 flex w-full items-center gap-2 border-t border-[var(--border-default)] px-2 py-2 text-left text-xs text-[var(--text-tertiary)]"><PanelRightClose size={14} />收起扩展</button>}
        </div>}
      </div>
    </header>
    <div className="relative flex min-h-0 flex-1">
      <div className="min-w-0 flex-1">{children}</div>
      {active && <div className={overlay ? 'absolute inset-y-0 right-0 z-20 flex shadow-xl' : 'flex min-h-0 shrink-0'}>
        <VerticalResizeHandle value={visibleWidth} min={minWidth} max={maxWidth} defaultValue={440} direction={-1} onDragStateChange={setDragging} onChange={changeWidth} onCommit={(value) => { void updateSetting(WIDTH_KEY, String(Math.round(value))).catch((e) => setError(String(e))); }} label="调整扩展区域宽度" />
        <aside aria-label={`${active.label}扩展`} className="flex min-h-0 min-w-0 flex-col bg-[var(--bg-surface)]" style={{ width: visibleWidth }}>
          <header className="flex h-9 shrink-0 items-center gap-2 border-b border-[var(--border-default)] px-3"><active.icon size={13} className="text-[var(--text-tertiary)]" /><span className="flex-1 text-xs font-medium">{active.label}</span>{extension === 'terminal' && <button type="button" title="清空终端" className={iconButton} onClick={() => setClearToken((n) => n + 1)}><Trash2 size={12} /></button>}<button type="button" aria-label="关闭扩展" className={iconButton} onClick={close}><X size={14} /></button></header>
          {error && <p role="alert" className="px-3 py-2 text-xs text-[var(--danger)]">{error}</p>}
          <div className="min-h-0 flex-1 overflow-hidden"><Suspense fallback={<div className="flex h-full items-center justify-center"><Loader2 size={18} className="animate-spin text-[var(--text-tertiary)]" /></div>}>
            {extension === 'files' && <ProjectFilesPanel root={root} onAttach={addFile} />}
            {extension === 'browser' && <AgentBrowserPanel connectAgent={false} active={!menuOpen && !dragging} onAddToChat={(target) => target.localPath ? addFile(target.localPath, target.name) : onAttach({ id: crypto.randomUUID(), kind: 'url', url: target.url, name: target.name })} />}
            {extension === 'terminal' && pageActive && (permissionMode === 'plan' ? <p className="p-4 text-xs text-[var(--text-tertiary)]">计划模式下终端不可用，请在对话输入区切换权限模式。</p> : <LocalTerminal root={root} permissionMode={permissionMode} clearToken={clearToken} onError={setError} />)}
          </Suspense></div>
        </aside>
      </div>}
    </div>
  </div>;
}
