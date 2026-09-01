import { useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type MouseEvent, type ReactNode } from 'react';
import {
  Archive,
  Check,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderOpen,
  FolderPlus,
  MessageSquareText,
  MoreHorizontal,
  Pin,
  Plus,
  Search,
  X,
} from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import EmptyState from '../components/ui/EmptyState';
import { useAgentStore, type AgentThread } from '../stores/agentStore';
import { useSurfaceAgentStore } from '../components/layout/KeptAlivePage';
import {
  THREAD_WORKSPACE_KEY_PREFIX,
  authorizeWorkspace,
  loadWorkspaceBinding,
  peekWorkspaceBinding,
  saveWorkspaceBinding,
  withWorkspacePermission,
  type WorkspaceBinding,
  type WorkspacePermissionMode,
} from '../services/workspaceBinding';
import ProjectChatPanel from '../components/aistudio/ProjectChatPanel';

/** 行间距（space-y-1 = 0.25rem），动态折叠测算行高时并入 */
const ROW_GAP = 4;
/** 省略号行高度测算常量（py-1.5 + 文本行 + gap） */
const ELLIPSIS_ROW_HEIGHT = 30;

/** 侧栏列表只在本会话第一次进入时拉取；热切换用 store 现有数据画稳首帧。 */
let conversationSidebarHydrated = false;

function displayTitle(thread: AgentThread): string {
  const title = thread.title?.trim() || '新会话';
  return title.length > 34 ? `${title.slice(0, 34)}…` : title;
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

interface RowMenuState {
  threadId: string;
  stage: 'root' | 'collections' | 'create';
}

/**
 * 全局快捷会话：一条 SophoNote Thread 对应一项可追踪任务。
 * 侧栏三段：置顶 / 收藏夹 / 最近（进行中+历史按更新时间合并倒序）。
 * 「最近」超出可视区域时才以省略号收起，点击展开后滚动查看。
 */
export default function Conversation() {
  const threads = useSurfaceAgentStore((state) => state.threads);
  const historyThreads = useSurfaceAgentStore((state) => state.historyThreads);
  const selectedThreadId = useSurfaceAgentStore((state) => state.selectedThreadId);
  const runningRunByThreadId = useSurfaceAgentStore((state) => state.runningRunByThreadId);
  const collections = useSurfaceAgentStore((state) => state.collections);
  const loadThreads = useAgentStore((state) => state.loadThreads);
  const loadCollections = useAgentStore((state) => state.loadCollections);
  const createCollection = useAgentStore((state) => state.createCollection);
  const setThreadPinned = useAgentStore((state) => state.setThreadPinned);
  const setThreadCollection = useAgentStore((state) => state.setThreadCollection);
  const loadThreadHistory = useAgentStore((state) => state.loadThreadHistory);
  const createThread = useAgentStore((state) => state.createThread);
  const reopenThread = useAgentStore((state) => state.reopenThread);
  const archiveThread = useAgentStore((state) => state.archiveThread);
  const selectThread = useAgentStore((state) => state.selectThread);

  const [query, setQuery] = useState('');
  const [expanded, setExpanded] = useState(false);
  const [menu, setMenu] = useState<RowMenuState | null>(null);
  const [menuDraft, setMenuDraft] = useState('');
  const [openCollections, setOpenCollections] = useState<Record<string, boolean>>({});
  const [sectionCreateOpen, setSectionCreateOpen] = useState(false);
  const [sectionDraft, setSectionDraft] = useState('');
  const [fit, setFit] = useState<{ shown: number; hidden: number } | null>(null);
  const [workspaceBinding, setWorkspaceBinding] = useState<WorkspaceBinding | null>(() => {
    const threadId = useAgentStore.getState().selectedThreadId;
    if (!threadId) return null;
    const cached = peekWorkspaceBinding(`${THREAD_WORKSPACE_KEY_PREFIX}${threadId}`);
    return cached === undefined ? null : cached;
  });
  const scrollRef = useRef<HTMLDivElement>(null);
  const recentListRef = useRef<HTMLDivElement>(null);

  const active = useMemo(
    () => threads.filter((thread) => thread.projectId == null && thread.closedAt == null && thread.archivedAt == null),
    [threads]
  );
  const history = useMemo(
    () => historyThreads.filter((thread) => thread.projectId == null && thread.archivedAt == null),
    [historyThreads]
  );
  // 同一 Thread 可能在 active/history 刷新交界期短暂重复，侧栏始终按 id 去重。
  const all = useMemo(() => {
    const unique = new Map<string, AgentThread>();
    [...history, ...active].forEach((thread) => unique.set(thread.id, thread));
    return [...unique.values()];
  }, [active, history]);
  const needle = query.trim().toLowerCase();
  const matches = (thread: AgentThread) => !needle || thread.title.toLowerCase().includes(needle);

  const pinned = useMemo(
    () =>
      all
        .filter((thread) => thread.pinnedAt != null)
        .sort((a, b) => (b.pinnedAt ?? 0) - (a.pinnedAt ?? 0)),
    [all]
  );
  const visiblePinned = useMemo(() => pinned.filter(matches), [pinned, needle]); // eslint-disable-line react-hooks/exhaustive-deps
  // 分组唯一归属：置顶 > 收藏夹 > 最近。同一 Thread 不得在两个位置重复渲染，
  // 否则按 threadId 控制的行菜单也会在两处同时打开。
  const recent = useMemo(
    () =>
      all
        .filter((thread) => thread.pinnedAt == null && thread.collectionId == null)
        .sort((a, b) => b.updatedAt - a.updatedAt),
    [all]
  );
  const visibleRecent = useMemo(() => recent.filter(matches), [recent, needle]); // eslint-disable-line react-hooks/exhaustive-deps
  const currentId = active.some((thread) => thread.id === selectedThreadId)
    ? selectedThreadId
    : active[0]?.id ?? null;

  useEffect(() => {
    if (conversationSidebarHydrated) return;
    conversationSidebarHydrated = true;
    const state = useAgentStore.getState();
    const jobs: Promise<unknown>[] = [];
    if (state.threads.length === 0) jobs.push(loadThreads(undefined, 'active'));
    if (state.historyThreads.length === 0) jobs.push(loadThreads(undefined, 'history'));
    jobs.push(loadCollections());
    void Promise.all(jobs);
  }, [loadThreads, loadCollections]);

  useEffect(() => {
    if (!currentId) {
      setWorkspaceBinding((prev) => (prev == null ? prev : null));
      return;
    }
    const key = `${THREAD_WORKSPACE_KEY_PREFIX}${currentId}`;
    const cached = peekWorkspaceBinding(key);
    if (cached !== undefined) {
      setWorkspaceBinding((prev) => (prev === cached ? prev : cached));
      return;
    }
    let active = true;
    loadWorkspaceBinding(key).then((value) => {
      if (active) setWorkspaceBinding(value);
    });
    return () => { active = false; };
  }, [currentId]);

  // 动态折叠：只有列表超出可视区域才出现省略号；条数按容器剩余高度拟合。
  const layoutDepsKey = [
    visibleRecent.length,
    expanded,
    visiblePinned.length,
    collections.length,
    JSON.stringify(openCollections),
  ].join('|');
  useLayoutEffect(() => {
    const compute = () => {
      const container = scrollRef.current;
      const list = recentListRef.current;
      if (!container || !list) return;
      const total = visibleRecent.length;
      if (total === 0) {
        setFit((prev) => (prev ? null : prev));
        return;
      }
      const firstRow = list.querySelector<HTMLElement>('[data-thread-row]');
      if (!firstRow) return;
      const rowH = firstRow.offsetHeight + ROW_GAP;
      const listTop = list.offsetTop;
      const available = container.clientHeight - listTop + container.scrollTop;
      if (expanded || total * rowH <= available) {
        // hidden=0 时保持 null，避免无意义 setState 连带重绘整块 ProjectChatPanel。
        setFit((prev) => (prev == null || prev.hidden === 0 ? prev : { shown: total, hidden: 0 }));
        return;
      }
      const shown = Math.min(total, Math.max(1, Math.floor((available - ELLIPSIS_ROW_HEIGHT - ROW_GAP) / rowH)));
      const hidden = total - shown;
      setFit((prev) => (prev && prev.shown === shown && prev.hidden === hidden ? prev : { shown, hidden }));
    };
    compute();
    const container = scrollRef.current;
    if (!container || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(compute);
    observer.observe(container);
    return () => observer.disconnect();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layoutDepsKey]);

  const openThread = async (threadId: string) => {
    selectThread(threadId);
    await loadThreadHistory(threadId);
  };

  const newThread = async () => {
    const id = await createThread(undefined, '新会话');
    if (id) await openThread(id);
  };

  const chooseWorkspace = async () => {
    let threadId = currentId;
    if (!threadId) {
      threadId = await createThread(undefined, '新会话');
      if (!threadId) return;
      await openThread(threadId);
    }
    const selected = await open({ directory: true, multiple: false, title: '选择本地项目目录' });
    if (!selected || Array.isArray(selected)) return;
    const binding = await authorizeWorkspace(selected);
    setWorkspaceBinding(binding);
    await saveWorkspaceBinding(`${THREAD_WORKSPACE_KEY_PREFIX}${threadId}`, binding);
  };

  const clearWorkspace = async () => {
    setWorkspaceBinding(null);
    if (currentId) await saveWorkspaceBinding(`${THREAD_WORKSPACE_KEY_PREFIX}${currentId}`, null);
  };

  const setWorkspacePermission = async (mode: WorkspacePermissionMode) => {
    if (!workspaceBinding || !currentId) return;
    const next = withWorkspacePermission(workspaceBinding, mode);
    setWorkspaceBinding(next);
    await saveWorkspaceBinding(`${THREAD_WORKSPACE_KEY_PREFIX}${currentId}`, next);
  };

  const restoreHistory = async (thread: AgentThread) => {
    const ok = await reopenThread(thread.id, undefined);
    if (ok) await openThread(thread.id);
  };

  // 归档对活跃/历史会话均适用：后端同时落 archived_at 与 closed_at，行从两个列表消失。
  const archiveRow = async (threadId: string, event: MouseEvent) => {
    event.stopPropagation();
    await archiveThread(threadId, undefined);
  };

  const submitSectionCreate = async (event: FormEvent) => {
    event.preventDefault();
    const created = await createCollection(sectionDraft);
    if (created) {
      setSectionDraft('');
      setSectionCreateOpen(false);
      setOpenCollections((state) => ({ ...state, [created.id]: true }));
    }
  };

  const submitMenuCreate = async (event: FormEvent, thread: AgentThread) => {
    event.preventDefault();
    const created = await createCollection(menuDraft);
    if (created) {
      await setThreadCollection(thread.id, created.id);
      setMenu(null);
    }
  };

  const menuItem = (icon: ReactNode, label: string, onClick: () => void, key?: string) => (
    <button
      key={key ?? label}
      type="button"
      onClick={onClick}
      className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]"
    >
      {icon}
      <span className="flex-1 min-w-0 truncate">{label}</span>
    </button>
  );

  const renderMenu = (thread: AgentThread) => (
    <>
      <div className="fixed inset-0 z-40" onClick={() => setMenu(null)} />
      <div className="absolute right-2 top-9 z-50 w-44 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] py-1 text-xs">
        {menu?.stage === 'root' && (
          <>
            {menuItem(
              <Pin size={12} className="text-[var(--text-tertiary)]" />,
              thread.pinnedAt != null ? '取消置顶' : '置顶',
              () => {
                void setThreadPinned(thread.id, thread.pinnedAt == null);
                setMenu(null);
              }
            )}
            {menuItem(
              <Folder size={12} className="text-[var(--text-tertiary)]" />,
              thread.collectionId != null ? '移动收藏夹' : '加入收藏夹',
              () => setMenu({ threadId: thread.id, stage: 'collections' })
            )}
          </>
        )}
        {menu?.stage === 'collections' && (
          <>
            {collections.length === 0 && (
              <p className="px-3 py-1.5 text-[var(--text-tertiary)]">暂无分类，先新建一个</p>
            )}
            {collections.map((col) =>
              menuItem(
                thread.collectionId === col.id ? (
                  <Check size={12} className="text-[var(--accent)]" />
                ) : (
                  <Folder size={12} className="text-[var(--text-tertiary)]" />
                ),
                col.name,
                () => {
                  void setThreadCollection(thread.id, col.id);
                  setMenu(null);
                },
                col.id
              )
            )}
            {thread.collectionId != null &&
              menuItem(
                <X size={12} className="text-[var(--text-tertiary)]" />,
                '移出收藏夹',
                () => {
                  void setThreadCollection(thread.id, null);
                  setMenu(null);
                }
              )}
            <div className="my-1 border-t border-[var(--border-default)]" />
            {menuItem(
              <FolderPlus size={12} className="text-[var(--text-tertiary)]" />,
              '新建分类',
              () => {
                setMenuDraft('');
                setMenu({ threadId: thread.id, stage: 'create' });
              }
            )}
          </>
        )}
        {menu?.stage === 'create' && (
          <form className="px-2 py-1.5" onSubmit={(event) => void submitMenuCreate(event, thread)}>
            <input
              autoFocus
              value={menuDraft}
              onChange={(event) => setMenuDraft(event.target.value)}
              placeholder="分类名称，回车创建并加入"
              className="w-full rounded-md border border-[var(--border-strong)] bg-[var(--bg-surface)] px-2 py-1 text-xs text-[var(--text-primary)] placeholder:text-[var(--text-disabled)] focus:outline-none focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
            />
          </form>
        )}
      </div>
    </>
  );

  const renderThreadRow = (thread: AgentThread, indented = false) => {
    const isCurrent = thread.id === currentId;
    const running = runningRunByThreadId[thread.id] != null;
    const closed = thread.closedAt != null;
    const menuOpen = menu?.threadId === thread.id;
    return (
      <div key={thread.id} className="relative" data-thread-row>
        <button
          type="button"
          onClick={() => void (closed ? restoreHistory(thread) : openThread(thread.id))}
          className={`group w-full rounded-lg px-2.5 py-2 text-left transition-colors ${
            isCurrent
              ? 'bg-[var(--accent-subtle)] shadow-[var(--shadow-sm)] ring-1 ring-[var(--accent-border)]'
              : 'hover:bg-[var(--bg-surface)]'
          } ${indented ? 'pl-5' : ''}`}
        >
          <div className="flex items-center gap-2">
            <span
              className={`h-1.5 w-1.5 rounded-full shrink-0 ${
                running
                  ? 'bg-[var(--accent)] animate-pulse'
                  : closed
                    ? 'border border-[var(--border-strong)]'
                    : 'bg-[var(--border-strong)]'
              }`}
            />
            <span className={`flex-1 min-w-0 truncate text-xs ${closed ? 'text-[var(--text-tertiary)]' : isCurrent ? 'font-semibold text-[var(--accent)]' : 'font-medium text-[var(--text-secondary)]'}`}>
              {displayTitle(thread)}
            </span>
            <span
              role="button"
              tabIndex={0}
              onClick={(event) => {
                event.stopPropagation();
                setMenu(menuOpen ? null : { threadId: thread.id, stage: 'root' });
              }}
              className={`rounded p-0.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] ${
                menuOpen ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
              }`}
              title="更多操作"
            >
              <MoreHorizontal size={11} />
            </span>
            <span
              role="button"
              tabIndex={0}
              onClick={(event) => void archiveRow(thread.id, event)}
              className="opacity-0 group-hover:opacity-100 rounded p-0.5 text-[var(--text-tertiary)] hover:bg-[var(--warning-subtle)] hover:text-[var(--warning)]"
              title="归档"
            >
              <Archive size={11} />
            </span>
          </div>
          <p className="mt-1 pl-3.5 text-xs text-[var(--text-tertiary)]">
            {running
              ? 'Hermes 正在执行'
              : `${closed ? '历史 · ' : ''}${formatTime(thread.updatedAt)}`}
          </p>
        </button>
        {menuOpen && renderMenu(thread)}
      </div>
    );
  };

  const sectionHeader = (label: string, action?: ReactNode) => (
    <p className="px-2 pb-1.5 flex items-center justify-between text-xs font-medium uppercase tracking-wider text-[var(--text-tertiary)]">
      <span>{label}</span>
      {action}
    </p>
  );

  const hiddenCount = !expanded && fit ? fit.hidden : 0;
  const shownRecent = hiddenCount > 0 ? visibleRecent.slice(0, fit?.shown ?? visibleRecent.length) : visibleRecent;
  const nothingAtAll = active.length === 0 && history.length === 0;

  return (
    <div className="flex h-full min-w-0 bg-[var(--bg-canvas)]">
      <aside className="w-64 shrink-0 border-r border-[var(--border-default)] bg-[var(--bg-sunken)] flex flex-col">
        <header className="h-10 shrink-0 border-b border-[var(--border-default)] px-3 flex items-center justify-between" data-tauri-drag-region>
          <div className="flex items-center gap-2 min-w-0" data-tauri-drag-region>
            <MessageSquareText size={14} className="text-[var(--accent)]" />
            <span className="text-xs font-semibold text-[var(--text-primary)]">会话</span>
          </div>
          <button
            type="button"
            onClick={() => void newThread()}
            className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--accent)]"
            title="新建会话"
          >
            <Plus size={15} />
          </button>
        </header>

        <div className="p-3 border-b border-[var(--border-default)]">
          <div className="relative">
            <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索会话…"
              className="w-full rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] py-1.5 pl-7 pr-2 text-xs text-[var(--text-primary)] placeholder:text-[var(--text-disabled)] focus:outline-none focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
            />
          </div>
        </div>

        <div ref={scrollRef} className="relative flex-1 min-h-0 overflow-y-auto px-2 py-3">
          {nothingAtAll ? (
            <EmptyState
              icon={MessageSquareText}
              title="还没有快捷任务"
              action={
                <button
                  type="button"
                  onClick={() => void newThread()}
                  className="btn-primary px-3 py-1.5 text-xs"
                >
                  新建第一个快捷任务
                </button>
              }
            />
          ) : (
            <div className="space-y-4">
              {visiblePinned.length > 0 && (
                <section>
                  {sectionHeader('置顶')}
                  <div className="space-y-1">{visiblePinned.map((thread) => renderThreadRow(thread))}</div>
                </section>
              )}

              <section>
                {sectionHeader(
                  '收藏夹',
                  <button
                    type="button"
                    onClick={() => {
                      setSectionDraft('');
                      setSectionCreateOpen((open) => !open);
                    }}
                    className="rounded p-0.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--accent)]"
                    title="新建收藏夹分类"
                  >
                    <Plus size={12} />
                  </button>
                )}
                <div className="space-y-1">
                  {collections.length === 0 && !sectionCreateOpen && (
                    <p className="px-2 py-1 text-xs text-[var(--text-tertiary)]">暂无分类，点右上 + 新建</p>
                  )}
                  {collections.map((col) => {
                    const members = all
                      .filter((thread) => thread.collectionId === col.id && thread.pinnedAt == null)
                      .filter(matches)
                      .sort((a, b) => b.updatedAt - a.updatedAt);
                    const open = openCollections[col.id] === true;
                    return (
                      <div key={col.id}>
                        <button
                          type="button"
                          onClick={() => setOpenCollections((state) => ({ ...state, [col.id]: !open }))}
                          className="group w-full flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-left hover:bg-[var(--bg-surface)]"
                        >
                          {open ? <ChevronDown size={11} className="text-[var(--text-tertiary)]" /> : <ChevronRight size={11} className="text-[var(--text-tertiary)]" />}
                          <Folder size={12} className="text-[var(--gold)]" />
                          <span className="flex-1 min-w-0 truncate text-xs font-medium text-[var(--text-secondary)]">{col.name}</span>
                          <span className="text-xs text-[var(--text-tertiary)]">{members.length}</span>
                        </button>
                        {open && (
                          <div className="space-y-1">
                            {members.length === 0 ? (
                              <p className="pl-7 py-1 text-xs text-[var(--text-tertiary)]">暂无会话；在会话行 ⋯ 菜单可加入本分类</p>
                            ) : (
                              members.map((thread) => renderThreadRow(thread, true))
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {sectionCreateOpen && (
                    <form className="px-2 py-1" onSubmit={(event) => void submitSectionCreate(event)}>
                      <input
                        autoFocus
                        value={sectionDraft}
                        onChange={(event) => setSectionDraft(event.target.value)}
                        onBlur={() => {
                          if (!sectionDraft.trim()) setSectionCreateOpen(false);
                        }}
                        placeholder="分类名称，回车创建"
                        className="w-full rounded-md border border-[var(--border-strong)] bg-[var(--bg-surface)] px-2 py-1 text-xs text-[var(--text-primary)] placeholder:text-[var(--text-disabled)] focus:outline-none focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
                      />
                    </form>
                  )}
                </div>
              </section>

              <section>
                {sectionHeader('最近')}
                {visibleRecent.length === 0 ? (
                  <p className="px-2 py-2 text-xs text-[var(--text-tertiary)]">
                    {needle ? '没有匹配的会话' : '暂无最近会话'}
                  </p>
                ) : (
                  <div ref={recentListRef} className="space-y-1">
                    {shownRecent.map((thread) => renderThreadRow(thread))}
                    {hiddenCount > 0 && (
                      <button
                        type="button"
                        onClick={() => setExpanded(true)}
                        className="w-full flex items-center justify-center gap-1 rounded-lg px-2.5 py-1.5 text-xs text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]"
                        title="展开后滑动查看历史会话"
                      >
                        <MoreHorizontal size={12} />
                        更多历史会话（{hiddenCount}）
                      </button>
                    )}
                  </div>
                )}
              </section>
            </div>
          )}
        </div>
      </aside>

      <main className="min-w-0 flex-1 flex flex-col bg-[var(--bg-canvas)]">
        <header className="h-10 shrink-0 border-b border-[var(--border-default)] px-4 flex items-center gap-3" data-tauri-drag-region>
          <div className="min-w-0" data-tauri-drag-region>
            <p className="truncate text-xs font-semibold text-[var(--text-primary)]">
              {active.find((thread) => thread.id === currentId)?.title?.trim() || '新会话'}
            </p>
          </div>
          <button
            type="button"
            onClick={() => void chooseWorkspace()}
            className={`ml-auto h-7 max-w-56 rounded-md px-2.5 inline-flex items-center gap-1.5 text-xs transition-colors ${workspaceBinding ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'}`}
            title={workspaceBinding?.root ?? '连接本地项目'}
          >
            <FolderOpen size={12} />
            <span className="truncate">{workspaceBinding?.name ?? '打开项目'}</span>
          </button>
        </header>
        <div className="flex-1 min-h-0 min-w-0">
          <div className="h-full min-w-[340px]">
            <ProjectChatPanel
              layout="workspace"
              projectId={null}
              threadId={currentId}
              workspaceRoot={workspaceBinding?.root ?? null}
              workspacePermissionMode={workspaceBinding?.permissionMode ?? 'ask'}
              onWorkspacePermissionModeChange={(mode) => void setWorkspacePermission(mode)}
              onClearWorkspace={() => void clearWorkspace()}
              showThreadNavigation={false}
              emptyHint={workspaceBinding
                ? `以「${workspaceBinding.name}」为范围，描述要实现、修复或检查的任务。`
                : '描述目标或打开本地项目，Agent 会执行代码理解、修改与验证。'}
              composerPlaceholder={workspaceBinding ? '描述要在项目中完成的任务…' : '描述要完成的任务…'}
            />
          </div>
        </div>
      </main>
    </div>
  );
}
