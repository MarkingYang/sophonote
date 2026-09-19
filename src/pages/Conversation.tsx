import { lazy, Suspense, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { Archive, ArrowLeft, ChevronRight, FolderKanban, FolderOpen, FolderPlus, Loader2, MoreHorizontal, Pin, Plus, Search, X } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { isPlaceholderThreadTitle, useAgentStore, type AgentAttachmentInput, type AgentThread } from '../stores/agentStore';
import { useProjectStore } from '../stores/projectStore';
import { useSurfaceAgentStore } from '../components/layout/KeptAlivePage';
import { PROJECT_WORKSPACE_KEY_PREFIX, authorizeWorkspace, loadWorkspaceBinding, peekWorkspaceBinding, saveWorkspaceBinding, withWorkspacePermission, type WorkspaceBinding, type WorkspacePermissionMode } from '../services/workspaceBinding';
import ProjectChatPanel from '../components/aistudio/ProjectChatPanel';
import ProjectHome from '../components/aistudio/ProjectHome';

const ProjectFilesPanel = lazy(() => import('../components/aistudio/ProjectFilesPanel'));
const titleOf = (thread: AgentThread) => isPlaceholderThreadTitle(thread.title) ? '新任务' : thread.title;
const statusOf = (thread: AgentThread) => ({ completed: '已完成', failed: '失败', cancelled: '已取消', running: '待继续' })[thread.status] ?? '历史';
const timeOf = (ms: number) => new Date(ms).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
const engineOf = (thread: AgentThread) => ({ hermes: 'Hermes', pi: 'Pi', claude_code: 'Claude Code', opencode: 'OpenCode' })[thread.engine ?? 'hermes'];
const buttonClass = 'rounded-md p-1.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--accent)] disabled:opacity-40';

/** 项目组织目录和任务；Thread/Run/原生历史继续由 agentStore 唯一管理。 */
export default function Conversation() {
  const threads = useSurfaceAgentStore((s) => s.threads);
  const historyThreads = useSurfaceAgentStore((s) => s.historyThreads);
  const selectedThreadId = useSurfaceAgentStore((s) => s.selectedThreadId);
  const running = useSurfaceAgentStore((s) => s.runningRunByThreadId);
  const projects = useProjectStore((s) => s.projects);
  const projectId = useProjectStore((s) => s.selectedProjectId);
  const loadProjects = useProjectStore((s) => s.load);
  const [query, setQuery] = useState('');
  const [home, setHome] = useState(true);
  const [createMode, setCreateMode] = useState<'new' | 'existing' | null>(null);
  const lastThreadByProject = useRef<Record<string, string>>({});
  const projectsLoading = useProjectStore((s) => s.loading);
  const [draft, setDraft] = useState(false);
  const [viewKey, setViewKey] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [menuId, setMenuId] = useState<string | null>(null);
  const [showFiles, setShowFiles] = useState(false);
  const [fileRequest, setFileRequest] = useState<{ nonce: number; attachment: AgentAttachmentInput } | null>(null);
  const [creating, setCreating] = useState(false);
  const [projectName, setProjectName] = useState('');
  const [projectFolder, setProjectFolder] = useState('');
  const dialogRef = useRef<HTMLDialogElement>(null);
  const selectionGeneration = useRef(0);
  const all = useMemo(() => [...new Map([...historyThreads, ...threads].filter((t) => t.archivedAt == null && t.projectId != null).map((t) => [t.id, t])).values()].sort((a, b) => b.updatedAt - a.updatedAt), [threads, historyThreads]);
  const current = !draft ? all.find((t) => t.id === selectedThreadId && t.projectId === projectId) ?? null : null;
  const project = projects.find((p) => p.id === projectId);
  const workspaceKey = projectId ? `${PROJECT_WORKSPACE_KEY_PREFIX}${projectId}` : null;
  const [workspace, setWorkspace] = useState<{ key: string | null; binding: WorkspaceBinding | null }>({ key: null, binding: null });
  const cached = workspaceKey ? peekWorkspaceBinding(workspaceKey) : null;
  const bindingReady = !workspaceKey || workspace.key === workspaceKey || cached !== undefined;
  const binding = workspace.key === workspaceKey ? workspace.binding : cached ?? null;

  useEffect(() => { void loadProjects(); }, [loadProjects]);
  // 每个 scope 的列表在 store 内合并，不覆盖其它项目任务。
  const projectIds = projects.map((p) => p.id).join('|');
  useEffect(() => {
    const store = useAgentStore.getState();
    void Promise.all(projects.map((p) => p.id).flatMap((id) => [store.loadThreads(id, 'active'), store.loadThreads(id, 'history')]));
  }, [projectIds]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!workspaceKey) { setWorkspace({ key: null, binding: null }); return; }
    let alive = true;
    void loadWorkspaceBinding(workspaceKey).then((value) => { if (alive) setWorkspace({ key: workspaceKey, binding: value }); });
    return () => { alive = false; };
  }, [workspaceKey]);
  useEffect(() => {
    if (creating) dialogRef.current?.showModal();
    else dialogRef.current?.close();
  }, [creating]);

  const returnHome = () => {
    selectionGeneration.current += 1;
    setHome(true); setShowFiles(false); setMenuId(null); setError(null);
  };
  useEffect(() => {
    window.addEventListener('sophonote:projects-home', returnHome);
    return () => window.removeEventListener('sophonote:projects-home', returnHome);
  }, []);

  const selectProject = (id: string, newTask = false) => {
    if (!useProjectStore.getState().projects.some((item) => item.id === id)) { returnHome(); return; }
    setHome(false); setQuery(''); setShowFiles(false);
    const remembered = lastThreadByProject.current[id];
    const thread = !newTask && all.find((item) => item.id === remembered && item.projectId === id);
    if (thread) { void openThread(thread); return; }
    selectionGeneration.current += 1;
    useProjectStore.getState().select(id);
    useAgentStore.getState().selectThread(null);
    setDraft(true); setViewKey((v) => v + 1); setFileRequest(null); setMenuId(null); setError(null);
    if (newTask) delete lastThreadByProject.current[id];
  };
  const openThread = async (thread: AgentThread) => {
    if (!thread.projectId || !projects.some((item) => item.id === thread.projectId)) { returnHome(); return; }
    const generation = ++selectionGeneration.current;
    setError(null); setMenuId(null);
    if (thread.closedAt != null && !(await useAgentStore.getState().reopenThread(thread.id, thread.projectId ?? undefined))) {
      setError('无法打开历史任务，请重试'); return;
    }
    if (generation !== selectionGeneration.current) return;
    useProjectStore.getState().select(thread.projectId);
    useAgentStore.getState().selectThread(thread.id);
    setHome(false); setDraft(false); setViewKey((v) => v + 1); setFileRequest(null);
    lastThreadByProject.current[thread.projectId] = thread.id;
    await useAgentStore.getState().loadThreadHistory(thread.id);
  };
  const chooseFolder = async (forNewProject: boolean) => {
    const key = workspaceKey;
    const generation = selectionGeneration.current;
    setBusy(true); setError(null);
    try {
      const selected = await open({ directory: true, multiple: false, title: forNewProject && createMode === 'new' ? '新建文件夹后选取' : '选择项目文件夹', canCreateDirectories: !forNewProject || createMode === 'new' });
      if (!selected || Array.isArray(selected) || generation !== selectionGeneration.current) return;
      if (forNewProject) {
        setProjectFolder(selected);
        setProjectName((name) => name || selected.split(/[\\/]/).filter(Boolean).pop() || '新项目');
        return;
      }
      if (!key) return;
      const authorized = await authorizeWorkspace(selected);
      await saveWorkspaceBinding(key, authorized);
      if (generation === selectionGeneration.current) setWorkspace({ key, binding: authorized });
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const createProject = async (event: FormEvent) => {
    event.preventDefault();
    if (busy || !projectName.trim() || !projectFolder) return;
    setBusy(true); setError(null);
    try {
      const created = await useProjectStore.getState().createProject(projectName, projectFolder);
      if (!created) throw new Error('创建项目失败，请检查文件夹后重试');
      const key = `${PROJECT_WORKSPACE_KEY_PREFIX}${created.id}`;
      const value = await loadWorkspaceBinding(key);
      setWorkspace({ key, binding: value });
      setCreating(false); selectProject(created.id);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const setPermission = async (mode: WorkspacePermissionMode) => {
    if (!workspaceKey || !binding) return;
    try {
      const next = withWorkspacePermission(binding, mode);
      await saveWorkspaceBinding(workspaceKey, next);
      setWorkspace({ key: workspaceKey, binding: next });
    } catch (e) { setError(String(e)); }
  };
  const archive = async (thread: AgentThread) => {
    setMenuId(null);
    if (!(await useAgentStore.getState().archiveThread(thread.id, thread.projectId ?? undefined))) setError('归档任务失败，请重试');
    else if (current?.id === thread.id && thread.projectId) selectProject(thread.projectId, true);
  };
  const startCreate = () => { setCreateMode(null); setProjectName(''); setProjectFolder(''); setError(null); setCreating(true); };
  const needle = query.trim().toLowerCase();
  const matches = (thread: AgentThread) => !needle || titleOf(thread).toLowerCase().includes(needle);
  const scoped = all.filter((t) => t.projectId === projectId && matches(t));
  const pinned = scoped.filter((t) => t.pinnedAt != null);
  const recent = scoped.filter((t) => t.pinnedAt == null);
  const row = (thread: AgentThread) => <div key={thread.id} className="sn-task-row group relative flex items-start" data-selected={current?.id === thread.id || undefined}>
    <button type="button" onClick={() => void openThread(thread)} aria-current={current?.id === thread.id ? 'page' : undefined} className="sn-task-open min-w-0 flex-1 text-left">
      <p className={`line-clamp-2 text-[13px] leading-5 ${current?.id === thread.id ? 'font-semibold text-[var(--accent)]' : 'text-[var(--text-secondary)]'}`}>{titleOf(thread)}</p>
      <p className="mt-1 truncate text-[11px] text-[var(--text-tertiary)]">{running[thread.id] ? `${engineOf(thread)} 正在执行` : `${statusOf(thread)} · ${timeOf(thread.updatedAt)}`}</p>
    </button>
    <button type="button" title={`任务操作：${titleOf(thread)}`} aria-expanded={menuId === thread.id} onClick={() => setMenuId(menuId === thread.id ? null : thread.id)} className={`${buttonClass} mt-1 opacity-0 group-hover:opacity-100 focus:opacity-100 ${menuId === thread.id ? 'opacity-100' : ''}`}><MoreHorizontal size={14} /></button>
    {menuId === thread.id && <>
      <button type="button" aria-label="关闭任务菜单" className="fixed inset-0 z-30 cursor-default" onClick={() => setMenuId(null)} />
      <div className="absolute right-0 top-9 z-40 w-52 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-1 shadow-lg text-xs">
        <button type="button" onClick={() => { void useAgentStore.getState().setThreadPinned(thread.id, thread.pinnedAt == null); setMenuId(null); }} className="flex w-full items-center gap-2 rounded p-2 hover:bg-[var(--bg-sunken)]"><Pin size={12} />{thread.pinnedAt != null ? '取消置顶' : '置顶任务'}</button>
        <button type="button" disabled={!!running[thread.id]} onClick={() => void archive(thread)} className="flex w-full items-center gap-2 rounded p-2 hover:bg-[var(--bg-sunken)] disabled:opacity-40"><Archive size={12} />归档任务</button>
      </div>
    </>}
  </div>;

  return <div className="flex h-full min-w-0 bg-[var(--bg-canvas)] text-[var(--text-primary)]">
    {home || !project ? <ProjectHome projects={projects} threads={all} loading={projectsLoading} onCreate={startCreate} onOpen={selectProject} /> : <>
    <aside aria-label="项目会话目录" className="sn-workspace-sidebar sn-project-sidebar flex shrink-0 flex-col border-r border-[var(--border-default)] bg-[var(--bg-sunken)]">
      <header className="flex h-10 shrink-0 items-center border-b border-[var(--border-default)] px-3" data-tauri-drag-region>
        <button type="button" onClick={returnHome} className={`${buttonClass} inline-flex items-center gap-2 text-xs`}><ArrowLeft size={14} />全部项目</button>
        <button type="button" title="创建项目" onClick={startCreate} className={`${buttonClass} ml-auto`}><FolderPlus size={15} /></button>
      </header>
      <div className="space-y-3 border-b border-[var(--border-default)] px-4 py-4">
        <div className="flex items-center gap-2"><FolderKanban size={16} className="shrink-0 text-[var(--accent)]" /><h2 className="min-w-0 flex-1 truncate py-2 text-sm font-medium" title={project.name}>{project.name}</h2></div>
        <button type="button" onClick={() => selectProject(project.id, true)} className="flex w-full items-center justify-center gap-2 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] py-2 text-xs hover:text-[var(--accent)]"><Plus size={14} />新建任务</button>
        <div className="relative"><Search size={12} className="absolute left-2.5 top-2.5 text-[var(--text-tertiary)]" /><input aria-label="搜索当前项目任务" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索任务会话…" className="w-full rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] py-1.5 pl-7 pr-2 text-xs focus:outline-none focus:border-[var(--accent)]" /></div>
      </div>
      <div className="min-h-0 flex-1 space-y-4 overflow-y-auto p-3">
        {pinned.length > 0 && <section><p className="px-2 py-2 text-xs text-[var(--text-tertiary)]">置顶会话</p>{pinned.map(row)}</section>}
        {recent.length > 0 && <section><p className="px-2 py-2 text-xs text-[var(--text-tertiary)]">项目会话</p>{recent.map(row)}</section>}
        {needle && scoped.length === 0 && <p className="px-2 py-3 text-xs text-[var(--text-tertiary)]">没有匹配的任务会话</p>}
      </div>
      {binding && <div title={binding.root} className="flex items-center gap-2 border-t border-[var(--border-default)] px-4 py-3 text-xs text-[var(--text-tertiary)]"><FolderOpen size={13} className="shrink-0" /><span className="truncate">{binding.name}</span></div>}
    </aside>
    <main className="relative flex min-w-0 flex-1 flex-col">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-[var(--border-default)] px-4" data-tauri-drag-region>
        <div className="min-w-0 flex-1 truncate text-xs font-semibold">{project && <span className="font-normal text-[var(--text-tertiary)]">{project.name} / </span>}{current ? titleOf(current) : '新任务'}</div>
        <button type="button" disabled={busy || (!binding && !!(current && running[current.id]))} onClick={() => { if (binding) setShowFiles(!showFiles); else if (workspaceKey) void chooseFolder(false); else startCreate(); }} title={binding?.root ?? '选择项目文件夹'} className={`${buttonClass} inline-flex max-w-56 items-center gap-1.5 text-xs`}><FolderOpen size={13} /><span className="truncate">{binding ? binding.name : projectId ? '选择文件夹' : '创建项目'}</span></button>
      </header>
      {error && !creating && <div role="alert" className="flex items-center gap-2 border-b border-[var(--border-default)] px-4 py-2 text-xs text-[var(--danger)]"><span className="flex-1">{error}</span><button type="button" aria-label="关闭错误提示" onClick={() => setError(null)}><X size={13} /></button></div>}
      <div className="relative flex min-h-0 flex-1">
        <div className="min-w-0 flex-1">
          {!bindingReady ? <div className="flex h-full items-center justify-center"><Loader2 size={18} className="animate-spin text-[var(--text-tertiary)]" /></div> : projectId && !binding ? <div className="flex h-full flex-col items-center justify-center gap-3 text-sm text-[var(--text-secondary)]"><FolderOpen size={32} /><p>为项目选择资料与交付文件夹</p><button type="button" onClick={() => void chooseFolder(false)} className="btn-primary px-4 py-2 text-xs">选择文件夹</button></div> : <ProjectChatPanel
            key={viewKey}
            layout="workspace" projectId={projectId} projectName={project?.name ?? '项目'} threadId={current?.id ?? null}
            draftSession={draft || !current} onDraftSessionChange={setDraft}
            onThreadCreated={(id) => { lastThreadByProject.current[projectId ?? ''] = id; useAgentStore.getState().selectThread(id); setDraft(false); }}
            workspaceRoot={binding?.root ?? null} workspacePermissionMode={binding?.permissionMode ?? 'ask'}
            onWorkspacePermissionModeChange={(mode) => void setPermission(mode)}
            fileContextRequest={fileRequest} showThreadNavigation={false} showEmptyState={false}
            emptyHint={binding ? `读取「${binding.name}」中的资料，描述要完成的任务。交付产物保存在此文件夹。` : '描述要完成的任务，或创建项目组织资料与交付产物。'}
            composerPlaceholder="描述要完成的任务…"
          />}
        </div>
        {showFiles && binding && <Suspense fallback={<Loader2 size={18} className="m-4 animate-spin" />}><ProjectFilesPanel key={binding.root} root={binding.root} onClose={() => setShowFiles(false)} onAttach={(path, name) => { setFileRequest({ nonce: Date.now(), attachment: { id: crypto.randomUUID(), kind: 'file', name, path } }); setShowFiles(false); }} /></Suspense>}
      </div>
    </main>
    </>}
    <dialog ref={dialogRef} onCancel={(event) => { if (busy) event.preventDefault(); else setCreating(false); }} onClose={() => setCreating(false)} className="m-auto w-[600px] max-w-[90vw] rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-0 text-[var(--text-primary)] shadow-xl backdrop:bg-black/35 backdrop:backdrop-blur-sm" aria-labelledby="create-project-title">
      <form onSubmit={(event) => void createProject(event)} className="space-y-5 p-7">
        <div className="flex items-center"><h2 id="create-project-title" className="flex-1 text-xl font-semibold">创建项目</h2><button type="button" disabled={busy} aria-label="关闭创建项目" className={buttonClass} onClick={() => setCreating(false)}><X size={16} /></button></div>
        <p className="text-sm leading-6 text-[var(--text-secondary)]">为持续推进的工作建立一个专属空间。资料与交付产物保存在本地文件夹中，任务会话按项目组织。</p>
        {!createMode ? <div className="space-y-3">
          <button type="button" aria-label="新建文件夹" onClick={() => setCreateMode('new')} className="flex w-full items-center gap-4 rounded-xl border border-[var(--border-default)] p-5 text-left hover:bg-[var(--bg-sunken)]"><span className="rounded-lg border border-[var(--border-default)] p-3"><Plus size={22} /></span><span className="flex-1"><span className="block text-sm font-medium">新建文件夹</span><span className="mt-1 block text-xs text-[var(--text-tertiary)]">从零开始，为项目准备一个新的本地文件夹。</span></span><ChevronRight size={18} /></button>
          <button type="button" aria-label="使用已有文件夹" onClick={() => setCreateMode('existing')} className="flex w-full items-center gap-4 rounded-xl border border-[var(--border-default)] p-5 text-left hover:bg-[var(--bg-sunken)]"><span className="rounded-lg border border-[var(--border-default)] p-3"><FolderOpen size={22} /></span><span className="flex-1"><span className="block text-sm font-medium">使用已有文件夹</span><span className="mt-1 block text-xs text-[var(--text-tertiary)]">从现有资料开始，继续在熟悉的目录中工作。</span></span><ChevronRight size={18} /></button>
        </div> : <>
        <button type="button" disabled={busy} onClick={() => { setCreateMode(null); setProjectFolder(''); }} className="inline-flex items-center gap-1.5 text-xs text-[var(--text-tertiary)]"><ArrowLeft size={13} />选择其他方式</button>
        <label className="block space-y-2 text-xs"><span>项目名称</span><input autoFocus required maxLength={120} value={projectName} disabled={busy} onChange={(e) => setProjectName(e.target.value)} placeholder="例如：行业研究" className="w-full rounded-lg border border-[var(--border-strong)] bg-[var(--bg-canvas)] px-3 py-2 outline-none focus:border-[var(--accent)]" /></label>
        <div className="space-y-2 text-xs"><p>本地文件夹</p><button type="button" disabled={busy} onClick={() => void chooseFolder(true)} className="flex w-full items-center gap-2 rounded-lg border border-[var(--border-strong)] px-3 py-2 text-left"><FolderOpen size={15} className="shrink-0 text-[var(--accent)]" /><span className="break-all">{projectFolder || '选择文件夹…'}</span></button><p className="text-[var(--text-tertiary)]">{createMode === 'new' ? '在系统窗口中点击“新建文件夹”，创建后选取。' : '直接使用已有文件夹，保留其中的文件。'}</p></div>
        {error && <p role="alert" className="text-xs text-[var(--danger)]">{error}</p>}
        <div className="flex justify-end gap-2"><button type="button" disabled={busy} onClick={() => setCreating(false)} className="rounded-lg px-3 py-2 text-xs hover:bg-[var(--bg-sunken)]">取消</button><button type="submit" disabled={busy || !projectName.trim() || !projectFolder} className="btn-primary inline-flex items-center gap-2 px-4 py-2 text-xs disabled:opacity-40">{busy && <Loader2 size={13} className="animate-spin" />}创建项目</button></div>
        </>}
      </form>
    </dialog>
  </div>;
}
