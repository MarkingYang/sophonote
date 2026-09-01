// ============================================================
// DEC-036 · 工作室 = Cursor 式 IDE-first 工作空间
// 左栏只展示项目与真实会话；中栏编辑本地代码/Markdown；右栏由会话驱动 Agent。
// 项目不再维护私有“项目文档”，本地目录中的 Markdown 就是项目文档。
// ============================================================
import { useEffect, useMemo, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import {
  Archive,
  Check,
  ChevronRight,
  FolderOpen,
  FolderPlus,
  MessageSquarePlus,
  MessageSquareText,
  MoreVertical,
  PanelLeftClose,
  PanelLeftOpen,
  Pencil,
  Pin,
  Rows2,
  Unlink,
  X,
} from 'lucide-react';
import { useProjectStore } from '../../stores/projectStore';
import {
  isPlaceholderThreadTitle,
  resolveProjectThreadId,
  useAgentStore,
  type AgentThread,
} from '../../stores/agentStore';
import { getSetting, updateSetting } from '../../services/tauri';
import {
  PROJECT_WORKSPACE_KEY_PREFIX,
  authorizeWorkspace,
  loadWorkspaceBinding,
  saveWorkspaceBinding,
  withWorkspacePermission,
  type WorkspaceBinding,
  type WorkspacePermissionMode,
} from '../../services/workspaceBinding';
import EmptyState from '../ui/EmptyState';
import VerticalResizeHandle from '../ui/VerticalResizeHandle';
import ResizableSplitPane from '../ui/ResizableSplitPane';
import LocalWorkspacePanel from './LocalWorkspacePanel';
import ProjectChatPanel from './ProjectChatPanel';

const EXPANDED_PROJECTS_KEY = 'ui:projects-expanded';
const PROJECT_TREE_COLLAPSED_KEY = 'ui:project-tree-collapsed-vscode';
const PROJECT_TREE_WIDTH_KEY = 'ui:project-tree-width';
const PROJECT_AGENT_WIDTH_KEY = 'ui:project-agent-width-vscode';
const DEFAULT_TREE_WIDTH = 232;
const DEFAULT_AGENT_WIDTH = 360;
const MIN_TREE_WIDTH = 200;

function readSet(value: string): Set<string> {
  try {
    return new Set(JSON.parse(value) as string[]);
  } catch {
    return new Set();
  }
}

function formatThreadTime(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  if (date.toDateString() === now.toDateString()) {
    return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  }
  return date.toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' });
}

function projectInitial(name: string): string {
  const firstLetterOrNumber = Array.from(name.trim()).find((character) => /[\p{L}\p{N}]/u.test(character));
  return firstLetterOrNumber?.toLocaleUpperCase() ?? '?';
}

export default function ProjectMode({ agentCollapsed = false }: { agentCollapsed?: boolean }) {
  const {
    projects,
    selectedProjectId,
    loaded,
    load,
    select,
    createProject,
    renameProject,
    setPinned,
    removeProject,
  } = useProjectStore();

  const threads = useAgentStore((state) => state.threads);
  const historyThreads = useAgentStore((state) => state.historyThreads);
  const selectedAgentThreadId = useAgentStore((state) => state.selectedThreadId);
  const runningByThread = useAgentStore((state) => state.runningRunByThreadId);
  const loadThreads = useAgentStore((state) => state.loadThreads);
  const reopenThread = useAgentStore((state) => state.reopenThread);
  const archiveThread = useAgentStore((state) => state.archiveThread);
  const selectThread = useAgentStore((state) => state.selectThread);
  const loadThreadHistory = useAgentStore((state) => state.loadThreadHistory);

  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const [treeCollapsed, setTreeCollapsed] = useState(true);
  const [treeWidth, setTreeWidth] = useState(DEFAULT_TREE_WIDTH);
  const [agentWidth, setAgentWidth] = useState(DEFAULT_AGENT_WIDTH);
  const [workspaceWidth, setWorkspaceWidth] = useState(() => window.innerWidth);
  const workspaceRef = useRef<HTMLDivElement>(null);

  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState('');
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const [menuProjectId, setMenuProjectId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [projectThreadId, setProjectThreadId] = useState<string | null>(null);
  const [draftProjectId, setDraftProjectId] = useState<string | null>(null);
  const [chatSplit, setChatSplit] = useState(false);
  const [splitChatThreadId, setSplitChatThreadId] = useState<string | null>(null);
  const [splitChatDraft, setSplitChatDraft] = useState(false);
  const [projectWorkspace, setProjectWorkspace] = useState<WorkspaceBinding | null>(null);

  useEffect(() => {
    if (!loaded) void load();
  }, [load, loaded]);

  useEffect(() => {
    Promise.all([
      getSetting(EXPANDED_PROJECTS_KEY),
      getSetting(PROJECT_TREE_COLLAPSED_KEY),
      getSetting(PROJECT_TREE_WIDTH_KEY),
      getSetting(PROJECT_AGENT_WIDTH_KEY),
    ]).then(([expanded, collapsed, savedTreeWidth, savedAgentWidth]) => {
      if (expanded) setExpandedProjects(readSet(expanded));
      setTreeCollapsed(collapsed == null ? true : collapsed === '1');
      const nextTreeWidth = Number(savedTreeWidth);
      if (Number.isFinite(nextTreeWidth)) setTreeWidth(Math.max(MIN_TREE_WIDTH, Math.min(440, nextTreeWidth)));
      const nextAgentWidth = Number(savedAgentWidth);
      if (Number.isFinite(nextAgentWidth)) setAgentWidth(Math.max(300, Math.min(720, nextAgentWidth)));
    }).catch(() => {});
  }, []);

  useEffect(() => {
    const host = workspaceRef.current;
    if (!host) return;
    let frame = 0;
    const observer = new ResizeObserver(([entry]) => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setWorkspaceWidth(Math.round(entry.contentRect.width)));
    });
    observer.observe(host);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, []);

  const selectedProject = projects.find((project) => project.id === selectedProjectId) ?? null;

  const threadsByProject = useMemo(() => {
    const unique = new Map<string, AgentThread>();
    for (const thread of [...threads, ...historyThreads]) {
      if (
        thread.projectId &&
        !thread.archivedAt &&
        thread.latestRunId &&
        !isPlaceholderThreadTitle(thread.title)
      ) unique.set(thread.id, thread);
    }
    const grouped = new Map<string, AgentThread[]>();
    for (const project of projects) {
      grouped.set(
        project.id,
        [...unique.values()]
          .filter((thread) => thread.projectId === project.id)
          .sort((left, right) => right.updatedAt - left.updatedAt),
      );
    }
    return grouped;
  }, [historyThreads, projects, threads]);

  useEffect(() => {
    if (!selectedProjectId) {
      setProjectThreadId(null);
      setDraftProjectId(null);
      setChatSplit(false);
      setSplitChatThreadId(null);
      setSplitChatDraft(false);
      setProjectWorkspace(null);
      return;
    }
    setChatSplit(false);
    setSplitChatThreadId(null);
    setSplitChatDraft(false);
    setDraftProjectId((current) => current === selectedProjectId ? current : null);
    setExpandedProjects((current) => {
      if (current.has(selectedProjectId)) return current;
      const next = new Set(current).add(selectedProjectId);
      void updateSetting(EXPANDED_PROJECTS_KEY, JSON.stringify([...next]));
      return next;
    });
    void Promise.all([
      loadThreads(selectedProjectId, 'active'),
      loadThreads(selectedProjectId, 'history'),
    ]);
    let active = true;
    setProjectWorkspace(null);
    loadWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${selectedProjectId}`)
      .then((binding) => { if (active) setProjectWorkspace(binding); });
    return () => { active = false; };
  }, [loadThreads, selectedProjectId]);

  useEffect(() => {
    if (!selectedProjectId) return;
    if (draftProjectId === selectedProjectId) {
      if (projectThreadId !== null) setProjectThreadId(null);
      return;
    }
    const available = threadsByProject.get(selectedProjectId) ?? [];
    const preferred = available.find((thread) => thread.id === selectedAgentThreadId && !thread.closedAt)
      ?? available.find((thread) => thread.id === projectThreadId && !thread.closedAt)
      ?? available.find((thread) => !thread.closedAt)
      ?? null;
    if (preferred?.id !== projectThreadId) setProjectThreadId(preferred?.id ?? null);
  }, [draftProjectId, projectThreadId, selectedAgentThreadId, selectedProjectId, threadsByProject]);

  const toggleTree = () => {
    setTreeCollapsed((current) => {
      const next = !current;
      void updateSetting(PROJECT_TREE_COLLAPSED_KEY, next ? '1' : '0');
      return next;
    });
  };

  const toggleProject = (projectId: string) => {
    select(projectId);
    setExpandedProjects((current) => {
      const next = new Set(current);
      if (selectedProjectId === projectId && next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      void updateSetting(EXPANDED_PROJECTS_KEY, JSON.stringify([...next]));
      return next;
    });
  };

  const submitProject = async () => {
    const name = newName.trim();
    if (!name) return;
    const created = await createProject(name);
    if (created) {
      setCreating(false);
      setNewName('');
      setExpandedProjects((current) => {
        const next = new Set(current).add(created.id);
        void updateSetting(EXPANDED_PROJECTS_KEY, JSON.stringify([...next]));
        return next;
      });
    }
  };

  const submitRename = async (projectId: string) => {
    const name = renameDraft.trim();
    if (name) await renameProject(projectId, name);
    setRenamingId(null);
  };

  const createProjectThread = async (projectId: string) => {
    if (projectId !== selectedProjectId) select(projectId);
    setDraftProjectId(projectId);
    setProjectThreadId(null);
    selectThread(null);
  };

  const archiveProjectThread = async (projectId: string, threadId: string) => {
    if (runningByThread[threadId]) return;
    const archived = await archiveThread(threadId, projectId);
    if (!archived) return;
    if (splitChatThreadId === threadId) {
      setChatSplit(false);
      setSplitChatThreadId(null);
    }
    if (projectThreadId !== threadId) return;
    const nextThreadId = resolveProjectThreadId(
      useAgentStore.getState().threads,
      projectId,
      null,
    );
    setProjectThreadId(nextThreadId);
    selectThread(nextThreadId);
    if (nextThreadId) await loadThreadHistory(nextThreadId);
  };

  const openProjectThread = async (projectId: string, thread: AgentThread) => {
    if (projectId !== selectedProjectId) select(projectId);
    setDraftProjectId(null);
    if (thread.closedAt && !(await reopenThread(thread.id, projectId))) return;
    selectThread(thread.id);
    setProjectThreadId(thread.id);
    await loadThreadHistory(thread.id);
  };

  const chooseWorkspace = async () => {
    if (!selectedProjectId) return;
    const chosen = await open({ directory: true, multiple: false, title: '打开本地项目' });
    if (!chosen || Array.isArray(chosen)) return;
    const binding = await authorizeWorkspace(chosen);
    setProjectWorkspace(binding);
    await saveWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${selectedProjectId}`, binding);
  };

  const clearWorkspace = async () => {
    setProjectWorkspace(null);
    if (selectedProjectId) {
      await saveWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${selectedProjectId}`, null);
    }
  };

  const changePermission = async (mode: WorkspacePermissionMode) => {
    if (!selectedProjectId || !projectWorkspace) return;
    const next = withWorkspacePermission(projectWorkspace, mode);
    setProjectWorkspace(next);
    await saveWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${selectedProjectId}`, next);
  };

  const confirmRemoveProject = async (projectId: string) => {
    setDeleteError(null);
    setDeletingId(projectId);
    try {
      await removeProject(projectId);
      // 本地工作区本身不做任何文件操作，仅清除 SophoNote 保存的路径授权书签。
      try {
        await saveWorkspaceBinding(`${PROJECT_WORKSPACE_KEY_PREFIX}${projectId}`, null);
      } catch (error) {
        console.error('Failed to clear removed project workspace binding:', error);
      }
      useAgentStore.setState((state) => ({
        threads: state.threads.map((thread) => (
          thread.projectId === projectId ? { ...thread, projectId: null } : thread
        )),
        historyThreads: state.historyThreads.map((thread) => (
          thread.projectId === projectId ? { ...thread, projectId: null } : thread
        )),
      }));
      setExpandedProjects((current) => {
        if (!current.has(projectId)) return current;
        const next = new Set(current);
        next.delete(projectId);
        void updateSetting(EXPANDED_PROJECTS_KEY, JSON.stringify([...next]));
        return next;
      });
      setMenuProjectId(null);
    } catch (error) {
      setDeleteError(error instanceof Error ? error.message : '移除失败');
    } finally {
      setDeletingId(null);
    }
  };

  const toggleChatSplit = async () => {
    if (!selectedProject) return;
    if (chatSplit) {
      setChatSplit(false);
      setSplitChatDraft(false);
      return;
    }
    const alternate = (threadsByProject.get(selectedProject.id) ?? [])
      .find((thread) => thread.id !== projectThreadId && !thread.closedAt);
    let nextThreadId = alternate?.id ?? null;
    setSplitChatDraft(nextThreadId == null);
    setSplitChatThreadId(nextThreadId);
    setChatSplit(true);
  };

  const showAgent = !agentCollapsed && selectedProject != null;
  const visibleTreeWidth = treeCollapsed ? 40 : Math.min(treeWidth, Math.max(MIN_TREE_WIDTH, workspaceWidth - 680));
  const maxAgentWidth = Math.max(300, Math.min(720, workspaceWidth - visibleTreeWidth - 420));
  const visibleAgentWidth = Math.min(agentWidth, maxAgentWidth);
  const agentOverlay = workspaceWidth < 900;

  return (
    <div ref={workspaceRef} className="relative flex min-h-0 flex-1 overflow-hidden bg-[var(--bg-canvas)]">
      <aside
        className="relative z-30 flex shrink-0 flex-col border-r border-[var(--border-default)] bg-[var(--bg-sunken)]"
        style={{ width: visibleTreeWidth }}
      >
        <div className="flex h-10 shrink-0 items-center justify-between border-b border-[var(--border-default)] px-2">
          {!treeCollapsed && <span className="px-1 text-xs font-semibold text-[var(--text-secondary)]">项目</span>}
          <div className="ml-auto flex items-center gap-0.5">
            {!treeCollapsed && (
              <button
                onClick={() => setCreating(true)}
                className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-primary)]"
                title="新建项目"
              >
                <FolderPlus size={15} />
              </button>
            )}
            <button
              onClick={toggleTree}
              className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-primary)]"
              title={treeCollapsed ? '展开项目栏' : '收起项目栏'}
            >
              {treeCollapsed ? <PanelLeftOpen size={15} /> : <PanelLeftClose size={15} />}
            </button>
          </div>
        </div>

        {treeCollapsed && (
          <div className="flex min-h-0 flex-1 flex-col items-center gap-1 overflow-y-auto py-2">
            {/* 用户指令（2026-08-20）：项目折叠后加号放在最上面 */}
            <button type="button" onClick={() => { setTreeCollapsed(false); setCreating(true); }} className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]" title="新建项目">
              <FolderPlus size={15} />
            </button>
            {projects.length > 0 && <div className="my-0.5 h-px w-5 shrink-0 bg-[var(--border-default)]" aria-hidden="true" />}
            {projects.map((project) => (
              <button
                key={project.id}
                type="button"
                onClick={() => select(project.id)}
                className={`flex h-8 w-8 items-center justify-center rounded-md ${project.id === selectedProjectId ? 'bg-[var(--bg-selected)] text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]'}`}
                title={project.name}
                aria-label={project.name}
              >
                <span className="relative flex h-6 w-6 items-center justify-center" aria-hidden="true">
                  <FolderOpen size={22} strokeWidth={1.6} />
                  <span className="absolute inset-x-0 top-[10px] text-center text-[8px] font-bold leading-none tracking-tight">
                    {projectInitial(project.name)}
                  </span>
                </span>
              </button>
            ))}
          </div>
        )}

        {!treeCollapsed && (
          <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
            {creating && (
              <div className="mb-2 flex items-center gap-1 rounded-lg border border-[var(--accent-border)] bg-[var(--bg-surface)] p-1.5">
                <input
                  autoFocus
                  value={newName}
                  onChange={(event) => setNewName(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') void submitProject();
                    if (event.key === 'Escape') setCreating(false);
                  }}
                  placeholder="项目名称"
                  className="min-w-0 flex-1 bg-transparent px-1 text-xs text-[var(--text-primary)] outline-none"
                />
                <button onClick={() => void submitProject()} className="p-1 text-[var(--success)]"><Check size={13} /></button>
                <button onClick={() => setCreating(false)} className="p-1 text-[var(--text-tertiary)]"><X size={13} /></button>
              </div>
            )}

            {projects.map((project) => {
              const projectThreads = threadsByProject.get(project.id) ?? [];
              const expanded = expandedProjects.has(project.id);
              const active = project.id === selectedProjectId;
              return (
                <div key={project.id} className="mb-0.5">
                  <div className={`group flex h-8 items-center gap-1 rounded-md px-1.5 ${active ? 'bg-[var(--bg-selected)] text-[var(--text-primary)]' : 'text-[var(--text-secondary)] hover:bg-[var(--bg-surface)]'}`}>
                    <button
                      onClick={() => toggleProject(project.id)}
                      className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                    >
                      <ChevronRight size={13} className={`shrink-0 transition-transform ${expanded ? 'rotate-90' : ''}`} />
                      <FolderOpen size={14} className="shrink-0" />
                      {renamingId === project.id ? (
                        <input
                          autoFocus
                          value={renameDraft}
                          onClick={(event) => event.stopPropagation()}
                          onChange={(event) => setRenameDraft(event.target.value)}
                          onBlur={() => void submitRename(project.id)}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') void submitRename(project.id);
                            if (event.key === 'Escape') setRenamingId(null);
                          }}
                          className="min-w-0 flex-1 bg-transparent text-xs outline-none"
                        />
                      ) : (
                        <span className="truncate text-xs font-medium">{project.name}</span>
                      )}
                      {project.pinned && <Pin size={10} className="shrink-0 fill-current text-[var(--text-tertiary)]" />}
                    </button>
                    <button
                      onClick={() => void createProjectThread(project.id)}
                      className="hidden p-1 text-[var(--text-tertiary)] hover:text-[var(--text-primary)] group-hover:block"
                      title="新建会话"
                    >
                      <MessageSquarePlus size={13} />
                    </button>
                    <button
                      onClick={() => {
                        setMenuProjectId(menuProjectId === project.id ? null : project.id);
                        setDeleteError(null);
                      }}
                      className="hidden p-1 text-[var(--text-tertiary)] hover:text-[var(--text-primary)] group-hover:block"
                      title="更多"
                    >
                      <MoreVertical size={13} />
                    </button>
                  </div>

                  {expanded && projectThreads.length > 0 && (
                    <div className="ml-5 border-l border-[var(--border-default)] py-0.5 pl-1.5">
                      {projectThreads.map((thread) => {
                        const selected = active && projectThreadId === thread.id;
                        return (
                          <div
                            key={thread.id}
                            className={`group/thread flex h-7 w-full items-center gap-1.5 rounded-md px-2 text-left ${selected ? 'bg-[var(--bg-selected)] text-[var(--text-primary)]' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]'}`}
                          >
                            <button
                              type="button"
                              onClick={() => void openProjectThread(project.id, thread)}
                              className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                            >
                              <MessageSquareText size={12} className="shrink-0" />
                              <span className="min-w-0 flex-1 truncate text-[11px]">{thread.title || '未命名会话'}</span>
                            </button>
                            {runningByThread[thread.id] ? (
                              <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-[var(--accent)]" title="运行中" />
                            ) : (
                              <>
                                <span className="shrink-0 text-[9px] group-hover/thread:hidden">{formatThreadTime(thread.updatedAt)}</span>
                                <button
                                  type="button"
                                  onClick={() => void archiveProjectThread(project.id, thread.id)}
                                  className="hidden shrink-0 rounded p-0.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] group-hover/thread:block"
                                  title="归档"
                                >
                                  <Archive size={11} />
                                </button>
                              </>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}

                  {menuProjectId === project.id && (
                    <div className="mx-1 my-1 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-1 shadow-[var(--shadow-md)]">
                      <button onClick={() => { setMenuProjectId(null); void createProjectThread(project.id); }} className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]">
                        <MessageSquarePlus size={13} />新建会话
                      </button>
                      <button onClick={() => { setRenamingId(project.id); setRenameDraft(project.name); setMenuProjectId(null); }} className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]">
                        <Pencil size={13} />重命名
                      </button>
                      <button onClick={() => { setMenuProjectId(null); void setPinned(project.id, !project.pinned); }} className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]">
                        <Pin size={13} />{project.pinned ? '取消置顶' : '置顶项目'}
                      </button>
                      <button
                        disabled={deletingId === project.id}
                        onClick={() => void confirmRemoveProject(project.id)}
                        className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-xs text-[var(--danger)] hover:bg-[var(--danger-subtle)] disabled:opacity-50"
                        title={deleteError ?? undefined}
                      >
                        <Unlink size={13} />
                        {deletingId === project.id ? '移除中…' : '移除项目'}
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </aside>

      {!treeCollapsed && (
        <VerticalResizeHandle
          value={treeWidth}
          min={MIN_TREE_WIDTH}
          max={440}
          defaultValue={DEFAULT_TREE_WIDTH}
          onChange={setTreeWidth}
          onCommit={(width) => void updateSetting(PROJECT_TREE_WIDTH_KEY, String(width))}
          label="调整项目栏宽度"
        />
      )}

      <main className="relative z-0 flex min-w-0 flex-1 flex-col overflow-hidden bg-[var(--bg-canvas)]">
        {selectedProject ? (
          <LocalWorkspacePanel
            root={projectWorkspace?.root ?? null}
            onChooseRoot={() => void chooseWorkspace()}
            onClearRoot={() => void clearWorkspace()}
            permissionMode={projectWorkspace?.permissionMode ?? 'ask'}
            className="h-full"
          />
        ) : (
          <EmptyState
            icon={FolderOpen}
            title="选择一个项目开始工作"
            desc="项目连接本地代码目录；Markdown 与代码都直接在原目录中编辑。"
            action={projects.length === 0 ? (
              <button onClick={() => setCreating(true)} className="btn-primary px-3 py-1.5 text-xs">
                新建项目
              </button>
            ) : undefined}
          />
        )}
      </main>

      {showAgent && (
        <>
          {!agentOverlay && (
            <VerticalResizeHandle
              value={agentWidth}
              min={300}
              max={maxAgentWidth}
              defaultValue={DEFAULT_AGENT_WIDTH}
              direction={-1}
              onChange={setAgentWidth}
              onCommit={(width) => void updateSetting(PROJECT_AGENT_WIDTH_KEY, String(width))}
              label="调整 AI 助手宽度"
            />
          )}
          <aside
            className={`${agentOverlay ? 'absolute inset-y-0 right-0 z-50 shadow-[-16px_0_40px_rgba(0,0,0,0.12)]' : 'relative z-10'} min-w-0 shrink-0 overflow-hidden border-l border-[var(--border-default)] bg-[var(--bg-surface)]`}
            style={{ width: visibleAgentWidth }}
          >
            <button
              type="button"
              onClick={() => void toggleChatSplit()}
              className={`absolute right-2 top-2 z-30 flex h-7 w-7 items-center justify-center rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] hover:bg-[var(--bg-sunken)] ${chatSplit ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'}`}
              title={chatSplit ? '关闭会话分屏' : '向下拆分会话'}
              aria-label={chatSplit ? '关闭会话分屏' : '向下拆分会话'}
            >
              <Rows2 size={13} />
            </button>
            <ResizableSplitPane
              direction="vertical"
              label="调整会话分屏高度"
              enabled={chatSplit}
              minFirst={200}
              minSecond={200}
              first={(
                <ProjectChatPanel
                  projectId={selectedProject.id}
                  projectName={selectedProject.name}
                  threadId={projectThreadId}
                  draftSession={draftProjectId === selectedProject.id}
                  onDraftSessionChange={(draft) => {
                    setDraftProjectId(draft ? selectedProject.id : null);
                    if (draft) {
                      setProjectThreadId(null);
                      selectThread(null);
                    }
                  }}
                  onThreadCreated={(threadId) => {
                    setDraftProjectId(null);
                    setProjectThreadId(threadId);
                  }}
                  showThreadNavigation={false}
                  showBrowserTab={false}
                  workspaceRoot={projectWorkspace?.root ?? null}
                  workspacePermissionMode={projectWorkspace?.permissionMode ?? 'ask'}
                  onWorkspacePermissionModeChange={(mode) => void changePermission(mode)}
                  onClearWorkspace={() => void clearWorkspace()}
                  emptyHint="描述任务，AI 会读取当前项目、提出修改并等待你审查。"
                  composerPlaceholder="让 AI 修改代码、运行命令或解释项目…"
                />
              )}
              second={splitChatThreadId || splitChatDraft ? (
                <ProjectChatPanel
                  projectId={selectedProject.id}
                  projectName={selectedProject.name}
                  threadId={splitChatThreadId}
                  draftSession={splitChatDraft}
                  onDraftSessionChange={(draft) => {
                    setSplitChatDraft(draft);
                    if (draft) setSplitChatThreadId(null);
                  }}
                  onThreadCreated={(threadId) => {
                    setSplitChatDraft(false);
                    setSplitChatThreadId(threadId);
                  }}
                  showThreadNavigation
                  showBrowserTab={false}
                  workspaceRoot={projectWorkspace?.root ?? null}
                  workspacePermissionMode={projectWorkspace?.permissionMode ?? 'ask'}
                  onWorkspacePermissionModeChange={(mode) => void changePermission(mode)}
                  onClearWorkspace={() => void clearWorkspace()}
                  emptyHint="描述另一个任务…"
                  composerPlaceholder="在分屏中继续另一个任务…"
                />
              ) : <div />}
            />
          </aside>
        </>
      )}
    </div>
  );
}
