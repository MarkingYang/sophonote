// ============================================================
// Track B · 智能体演进（AG-15 · Phase 2 Chat · ProjectChatPanel）
// 实施基线：docs/architecture.md 事件协议 + §六 RunStore
//
// 职责：
// - 与后端 agent_run_start 交互（调用 useAgentStore.startRun）
// - 订阅 on_event Channel，实时渲染事件流
// - 从事件流归约消息列表（useAgentStore.messagesOfThread）
// - 断线恢复（useAgentStore.replayEvents / loadThreadHistory）
//
// AG-01：多会话 tab / 历史 / 归档；能力管理统一放在设置页。
// AG-16：Chat 折叠仍在 AIStudio 首行；本组件始终为「展开态」。
// ============================================================
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type DragEvent, type MouseEvent, type ReactNode } from 'react';
import { useShallow } from 'zustand/react/shallow';
import {
  ArrowUp, ArrowDown, Loader2, Send, Square, Wrench, Check, X, Sparkles,
  Plus, Clock, Archive, FileText, FolderOpen, Image, Clipboard,
  Link2, ChevronDown, ChevronLeft, ChevronRight, Boxes, Search, AlertTriangle, Server,
  MessageSquareText, ShieldAlert, Zap,
  Globe2,
} from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { confirm as confirmDialog, open } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import {
  DEFAULT_THREAD_HISTORY_TTL_DAYS,
  THREAD_HISTORY_TTL_DAYS_KEY,
  isPlaceholderThreadTitle,
  resolveProjectThreadId,
  useAgentStore,
  type AgentMessage,
  type AgentThread,
  type RunContext,
  type RunSkillRef,
  type FocusDocumentInput,
  type AgentAttachmentInput,
  type AgentAttachmentKind,
  type AgentEvent,
} from '../../stores/agentStore';
import * as tauri from '../../services/tauri';
import {
  pickArtifactView,
  type ArtifactView,
  type DiffPreviewPayload,
  type RenamePreviewPayload,
  type ToolCard,
} from '../../services/agentToolCards';
import { useAppStore } from '../../stores/appStore';
import { useProjectStore } from '../../stores/projectStore';
import {
  groupToolCardsByRunId,
  timelineToolCards,
  toolDisplayName,
  toolStepSummary,
} from '../../services/agentProcessRail';
import {
  shouldRenderAreaBContent,
  type AssistantPhase,
} from '../../services/agentMessagePhase';
import { splitStreamingMarkdown } from '../../services/streamingMarkdown';
import { recordAgentRenderCommit } from '../../services/agentStreamPerf';
import {
  hermesCapabilities,
  hermesBrowserManage,
  hermesMcpAdd,
  hermesMcpCatalog,
  hermesMcpCatalogInstall,
  hermesMcpOAuthStart,
  hermesMcpOAuthStatus,
  hermesMcpRemove,
  hermesMcpSetEnabled,
  hermesMcpTest,
  hermesSkillInstall,
  hermesSkillHubPreview,
  hermesSkillArchive,
  hermesSkillDocument,
  hermesSkillDocumentSave,
  hermesSkillSetEnabled,
  hermesSkillsHub,
  hermesToolsetSetEnabled,
  hermesTerminalBackendSelect,
  type HermesSkillInfo,
  type HermesModelOptions,
  type HermesCapabilities,
  type HermesHubPage,
  type HermesHubPreview,
  type HermesMcpCatalog,
  type HermesMcpProbe,
  type HermesMcpServerCreate,
  hermesSessionSetYolo,
  hermesSessionSlash,
  hermesSessionSurface,
  type HermesCommandInfo,
  type HermesReferenceInfo,
  type HermesConnectionStatus,
  type HermesSessionSurface,
} from '../../services/tauri';
import {
  peekHermesCapabilities,
  peekHermesModelOptions,
  rememberHermesCapabilities,
  rememberHermesModelOptions,
  resolveStoredHermesSelection,
} from '../../services/hermesRuntimeCache';
import MarkdownView from '../features/MarkdownView';
import {
  activeChangeSession,
  continuationContextForChange,
  useChangeSessionStore,
} from '../../stores/changeSessionStore';
import { changePhaseFromStatus } from '../../services/changeSession';
import {
  isTimelineNearBottom,
  latestTimelineStart,
  previousTimelineStart,
  threadEventRevision,
} from '../../services/agentTimelineFollow';
import { HermesInputRequest } from './HermesInputRequest';
import EmptyState from '../ui/EmptyState';
import { useSurfaceAgentStore } from '../layout/KeptAlivePage';
import AgentBrowserPanel from './AgentBrowserPanel';
import type { WorkspacePermissionMode } from '../../services/workspaceBinding';
import {
  canUseComposerHistory,
  capabilityMatches,
  composerHistoryStep,
  composerItems,
  detectComposerTrigger,
  droppedPathKind,
  droppedPathName,
  findTimelineMatches,
  isSessionControlCommand,
  physicalPointInCssRect,
  rememberComposerHistory,
  replaceComposerTrigger,
  type HermesComposerItem,
} from '../../services/hermesComposer';

interface ProjectChatPanelProps {
  /** null = 全局快捷会话；string = 项目内会话。 */
  projectId?: string | null;
  /** Change Session 的前端作用域；笔记本无 projectId，但仍需接收左侧文档 Patch。 */
  changeScopeId?: string | null;
  projectName?: string;
  threadId?: string | null;
  /** 项目会话草稿：首次发送前不创建 Thread。 */
  draftSession?: boolean;
  onDraftSessionChange?: (draft: boolean) => void;
  onThreadCreated?: (threadId: string) => void;
  /** 独立会话页已有左侧历史栏时，隐藏面板自己的 tab/history 头。 */
  showThreadNavigation?: boolean;
  /** 非项目场景的空态与输入提示。 */
  emptyHint?: string;
  composerPlaceholder?: string;
  /** AG-26：当前编辑器选区的范围 chip（null = 全项目对话；由宿主捕获后经此注入） */
  selection?: RunContext | null;
  /** AG-31：chip 行号展示（Markdown 源码行，best-effort；null 回落摘录） */
  selectionLines?: [number, number] | null;
  /** 移除 composer 上的范围 chip（X 按钮 / 发送后消费） */
  onClearSelection?: () => void;
  /** 当前中栏文档；用于把后续 Query 自动绑定到尚未落定的同一变更会话。 */
  activeDocumentId?: string | null;
  /** 当前中栏文档标题（由父 Surface 展示；正文通过 resolver 在发送时读取） */
  activeDocumentTitle?: string | null;
  /** 发送时读取编辑器最新草稿；避免按键级把整篇正文放入 React state。 */
  resolveActiveDocumentContext?: () => Promise<FocusDocumentInput | null>;
  /** 用户从左侧显式把项目加入会话；发送项目清单工作副本，不隐式读取成员正文。 */
  includeProjectContext?: boolean;
  /** 移除显式项目范围。 */
  onClearProjectContext?: () => void;
  /** 本地项目目录：作为本会话每一轮的持续文件夹上下文，不随发送清空。 */
  workspaceRoot?: string | null;
  /** 当前工作区权限模式；由绑定工作区的 Chat / 项目持久化。 */
  workspacePermissionMode?: WorkspacePermissionMode;
  /** 权限模式只能从会话 Surface 调整；文件/终端工作面只消费该值。 */
  onWorkspacePermissionModeChange?: (mode: WorkspacePermissionMode) => void;
  /** 移除持续项目目录。 */
  onClearWorkspace?: () => void;
  /** 工作室将 Browser 放入 IDE 省略号时关闭会话内重复入口。 */
  showBrowserTab?: boolean;
  /** workspace = 中央主画布，不绘制旧右栏分隔线。 */
  layout?: 'panel' | 'workspace';
}

const EMPTY_MESSAGES: AgentMessage[] = [];
const EMPTY_TOOL_CARDS: ToolCard[] = [];
const EMPTY_RUN_IDS: string[] = [];
const SOPHONOTE_MARKDOWN_WRITING_SKILL = 'sophonote-markdown-writing';
export type CapabilityTab = 'skills' | 'tools' | 'mcp' | 'hub';

const CAPABILITY_MENU_ITEMS: ReadonlyArray<{
  key: CapabilityTab;
  label: string;
  icon: typeof Boxes;
}> = [
  { key: 'skills', label: 'Skill', icon: Boxes },
  { key: 'tools', label: 'Tools', icon: Wrench },
  { key: 'mcp', label: 'MCP', icon: Server },
  { key: 'hub', label: 'Browse Hub', icon: Search },
];

function attachmentId(): string {
  return typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `attachment-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function attachmentName(path: string): string {
  return droppedPathName(path);
}

function blobAsDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result || ''));
    reader.onerror = () => reject(reader.error ?? new Error('读取图片失败'));
    reader.readAsDataURL(blob);
  });
}

/** 项目 Chat 面板：用户输入 → 调用 Agent → 渲染事件流 */
export default function ProjectChatPanel({
  projectId,
  changeScopeId: requestedChangeScopeId,
  projectName = '工作室',
  threadId: propThreadId,
  draftSession = false,
  onDraftSessionChange,
  onThreadCreated,
  selection = null,
  selectionLines = null,
  onClearSelection,
  activeDocumentId = null,
  activeDocumentTitle = null,
  resolveActiveDocumentContext,
  includeProjectContext = false,
  onClearProjectContext,
  workspaceRoot = null,
  workspacePermissionMode = 'ask',
  onWorkspacePermissionModeChange,
  onClearWorkspace,
  showBrowserTab = true,
  showThreadNavigation = true,
  emptyHint,
  composerPlaceholder,
  layout = 'panel',
}: ProjectChatPanelProps) {
  const scopedProjectId = projectId ?? null;
  const changeScopeId = requestedChangeScopeId ?? projectId ?? null;
  const hasProjectScope = scopedProjectId != null;
  // 精确订阅 Chat 真正消费的 slice。禁止无 selector 订阅整个 AgentStore；事件持久化、
  // 恢复标记等内部状态变化不应让长消息列表重新协调。
  const threads = useSurfaceAgentStore((state) => state.threads);
  const historyThreads = useSurfaceAgentStore((state) => state.historyThreads);
  const selectedThreadId = useSurfaceAgentStore((state) => state.selectedThreadId);
  const runningRunByThreadId = useSurfaceAgentStore((state) => state.runningRunByThreadId);
  const historyLoadingByThreadId = useSurfaceAgentStore((state) => state.historyLoadingByThreadId);
  const resumingRunByThreadId = useSurfaceAgentStore((state) => state.resumingRunByThreadId);
  const degraded = useSurfaceAgentStore((state) => state.degraded);
  const startRun = useAgentStore((state) => state.startRun);
  const loadThreads = useAgentStore((state) => state.loadThreads);
  const loadThreadHistory = useAgentStore((state) => state.loadThreadHistory);
  const cancelRun = useAgentStore((state) => state.cancelRun);
  const forgetThreadView = useAgentStore((state) => state.forgetThreadView);
  const respondApproval = useAgentStore((state) => state.respondApproval);
  const respondClarify = useAgentStore((state) => state.respondClarify);
  const createThread = useAgentStore((state) => state.createThread);
  const closeThread = useAgentStore((state) => state.closeThread);
  const reopenThread = useAgentStore((state) => state.reopenThread);
  const archiveThread = useAgentStore((state) => state.archiveThread);
  const selectThread = useAgentStore((state) => state.selectThread);
  const gcThreads = useAgentStore((state) => state.gcThreads);
  const loadArticles = useAppStore((state) => state.loadArticles);
  const loadProjects = useProjectStore((state) => state.load);

  // 本地追踪当前活跃 Thread（解决 startRun 创建新 Thread 后的线程切换）
  const [activeThreadId, setActiveThreadId] = useState<string | null>(propThreadId ?? selectedThreadId ?? null);
  const [localDraftSession, setLocalDraftSession] = useState(false);
  const isDraftSession = draftSession || localDraftSession;
  // 新建 Thread 需要一次 Tauri 往返。在新 id 回到前显式展示空白会话，禁止
  // resolveProjectThreadId 回退到旧 Thread；ref 同时拦住快速连点产生多个空 Tab。
  const creatingThreadRef = useRef(false);
  const [creatingThread, setCreatingThread] = useState(false);
  // AG-17 窗口重挂载恢复：同一 Thread 只恢复一次（threads 会因事件/刷新频繁变化）
  const historyRestoredRef = useRef<string | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  // DEC-014：Composer 只保存本轮待发送选择；路径读取/预算校验在 Rust。
  const [attachments, setAttachments] = useState<AgentAttachmentInput[]>([]);
  const [surfaceTab, setSurfaceTab] = useState<'chat' | 'browser'>('chat');
  const [browserConnected, setBrowserConnected] = useState(
    () => peekHermesCapabilities()?.browserConnected ?? false,
  );
  const [attachmentMenuOpen, setAttachmentMenuOpen] = useState(false);
  const [localPermissionMode, setLocalPermissionMode] = useState<WorkspacePermissionMode>(workspacePermissionMode);
  const [urlEditorOpen, setUrlEditorOpen] = useState(false);
  const [urlDraft, setUrlDraft] = useState('');
  const [composerError, setComposerError] = useState<string | null>(null);
  const [sessionNotice, setSessionNotice] = useState<string | null>(null);
  const [sessionSurface, setSessionSurface] = useState<HermesSessionSurface | null>(null);
  const [composerPrefill, setComposerPrefill] = useState<{ nonce: number; text: string } | null>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [findIndex, setFindIndex] = useState(0);
  const [dropActive, setDropActive] = useState(false);
  const nativeDropAtRef = useRef(0);
  const chatPanelRef = useRef<HTMLElement | null>(null);
  const [hermesModels, setHermesModels] = useState<HermesModelOptions | null>(() => peekHermesModelOptions());
  const [hermesModelError, setHermesModelError] = useState<string | null>(null);
  const [selectedHermesProvider, setSelectedHermesProvider] = useState(
    () => resolveStoredHermesSelection(peekHermesModelOptions()).provider,
  );
  const [selectedHermesModel, setSelectedHermesModel] = useState(
    () => resolveStoredHermesSelection(peekHermesModelOptions()).model,
  );
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const timelineViewportRef = useRef<HTMLDivElement>(null);
  const timelineFollowingRef = useRef(true);
  const initialScrollThreadRef = useRef<string | null>(null);
  const prependAnchorRef = useRef<{ scrollHeight: number; scrollTop: number } | null>(null);
  const [timelineWindow, setTimelineWindow] = useState<{ threadId: string | null; start: number }>({
    threadId: null,
    start: 0,
  });
  const [showJumpToLatest, setShowJumpToLatest] = useState(false);
  const effectivePermissionMode = onWorkspacePermissionModeChange
    ? workspacePermissionMode
    : localPermissionMode;
  const changePermissionMode = onWorkspacePermissionModeChange ?? setLocalPermissionMode;

  useEffect(() => {
    setLocalPermissionMode(workspacePermissionMode);
  }, [workspacePermissionMode]);

  const activeProjectThreads = useMemo(
    () => threads.filter((t) => (
      t.projectId === scopedProjectId &&
      t.closedAt == null &&
      t.archivedAt == null &&
      (!hasProjectScope || (!!t.latestRunId && !isPlaceholderThreadTitle(t.title)))
    )),
    [hasProjectScope, threads, scopedProjectId]
  );
  const projectHistoryThreads = useMemo(
    () => historyThreads.filter((t) => t.projectId === scopedProjectId),
    [historyThreads, scopedProjectId]
  );

  // 挂载：可选归档 GC + 拉活跃/历史（TTL 不在 UI 暴露）
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const alreadyHasThreads = useAgentStore.getState().threads.length > 0;
      try {
        if (!alreadyHasThreads) {
          const raw = await tauri.getSetting(THREAD_HISTORY_TTL_DAYS_KEY);
          const n = Number(raw);
          const days = Number.isFinite(n) && n >= 0 ? Math.floor(n) : DEFAULT_THREAD_HISTORY_TTL_DAYS;
          if (days > 0) await gcThreads(days);
        }
      } catch {
        /* 永久保留：不跑 GC */
      }
      if (cancelled || alreadyHasThreads) return;
      await loadThreads(projectId ?? undefined, 'active');
      await loadThreads(projectId ?? undefined, 'history');
    })();
    return () => { cancelled = true; };
  }, [projectId, loadThreads, gcThreads]);

  // projectId / 外部 thread 变化时重置本地 Thread 状态；首挂已用 props 初始化，跳过以免多一次整板 commit。
  const projectScopeReadyRef = useRef(false);
  useEffect(() => {
    if (!projectScopeReadyRef.current) {
      projectScopeReadyRef.current = true;
      return;
    }
    setActiveThreadId(propThreadId ?? null);
    setLocalDraftSession(false);
    historyRestoredRef.current = null;
    setHistoryOpen(false);
    setAttachments([]);
    setAttachmentMenuOpen(false);
    setUrlEditorOpen(false);
    setComposerError(null);
    setModelMenuOpen(false);
  }, [projectId, propThreadId]);

  // Hermes Runtime 是模型目录与当前选择的唯一真相源。SophoNote 只保留一个
  // Surface 级 UI 偏好；若条目已从 Runtime 清单移除则回退 Runtime 当前值。
  const refreshHermesModels = useCallback(() => {
    let cancelled = false;
    setHermesModelError(null);
    tauri.hermesModelOptions()
      .then((options) => {
        rememberHermesModelOptions(options);
        if (cancelled) return;
        setHermesModels((prev) => (
          prev && JSON.stringify(prev) === JSON.stringify(options) ? prev : options
        ));
        const selection = resolveStoredHermesSelection(options);
        setSelectedHermesProvider((prev) => (prev === selection.provider ? prev : selection.provider));
        setSelectedHermesModel((prev) => (prev === selection.model ? prev : selection.model));
        if (selection.provider && selection.model) {
          window.localStorage.setItem('sophonote.hermes.provider', selection.provider);
          window.localStorage.setItem('sophonote.hermes.model', selection.model);
        } else {
          window.localStorage.removeItem('sophonote.hermes.provider');
          window.localStorage.removeItem('sophonote.hermes.model');
        }
      })
      .catch((error) => {
        if (cancelled) return;
        if (peekHermesModelOptions()) return;
        setHermesModels(null);
        setSelectedHermesProvider('');
        setSelectedHermesModel('');
        setHermesModelError(error instanceof Error ? error.message : String(error));
      });
    return () => { cancelled = true; };
  }, []);
  useEffect(() => {
    if (peekHermesModelOptions()) return;
    return refreshHermesModels();
  }, [refreshHermesModels]);
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void tauri.listenHermesStatusChanged((status) => {
      if (status === 'connected') refreshHermesModels();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshHermesModels]);

  const rememberHermesModel = useCallback((provider: string, model: string) => {
    const row = hermesModels?.providers.find((item) => item.slug === provider);
    if (row?.authenticated !== true || !row.models.includes(model)) return;
    setSelectedHermesProvider(provider);
    setSelectedHermesModel(model);
    setModelMenuOpen(false);
    window.localStorage.setItem('sophonote.hermes.provider', provider);
    window.localStorage.setItem('sophonote.hermes.model', model);
  }, [hermesModels]);

  // AG-17 窗口重挂载恢复：绑定项目下第一个有内容的活跃 Thread
  useEffect(() => {
    if (isDraftSession || creatingThread || activeThreadId || propThreadId) return;
    const thread = activeProjectThreads.find((t) => t.latestRunId);
    if (!thread) return;
    if (historyRestoredRef.current === thread.id) return;
    historyRestoredRef.current = thread.id;
    setActiveThreadId(thread.id);
    selectThread(thread.id);
    void loadThreadHistory(thread.id);
  }, [activeProjectThreads, projectId, activeThreadId, propThreadId, loadThreadHistory, selectThread, creatingThread, isDraftSession]);

  // 当前 Thread 的消息列表
  // 项目切换后的首帧可能还保留上一个项目的本地 activeThreadId；解析时再次
  // 校验 Thread.projectId，保证旧会话永远不会被提交到新项目。
  const requestedThreadId = activeThreadId ?? propThreadId;
  const explicitThreadId = requestedThreadId && threads.some((thread) => (
    thread.id === requestedThreadId && thread.projectId === scopedProjectId
  )) ? requestedThreadId : null;
  const currentThreadId = (creatingThread || (isDraftSession && !activeThreadId))
    ? null
    : resolveProjectThreadId(
        explicitThreadId ? threads : activeProjectThreads,
        scopedProjectId,
        explicitThreadId ?? selectedThreadId
      );
  const messages = useSurfaceAgentStore((state) =>
    currentThreadId ? state.messagesByThreadId[currentThreadId] ?? EMPTY_MESSAGES : EMPTY_MESSAGES
  );
  // AG-21：当前 Thread 的工具结果卡（只消费 structured/uiArtifact，不解析 model_text）
  const toolCards = useSurfaceAgentStore((state) =>
    currentThreadId ? state.toolCardsByThreadId[currentThreadId] ?? EMPTY_TOOL_CARDS : EMPTY_TOOL_CARDS
  );
  const projectTreeRefreshRef = useRef<string | null>(null);
  const completedProjectTreeCard = useMemo(
    () => [...toolCards].reverse().find((card) =>
      card.name === 'sophonote_project_tree' && card.status === 'completed'
    ) ?? null,
    [toolCards]
  );
  useEffect(() => {
    if (!completedProjectTreeCard || projectTreeRefreshRef.current === completedProjectTreeCard.callId) return;
    projectTreeRefreshRef.current = completedProjectTreeCard.callId;
    void Promise.all([loadArticles(), loadProjects()]);
  }, [completedProjectTreeCard, loadArticles, loadProjects]);
  const currentRunIds = useSurfaceAgentStore((state) =>
    currentThreadId ? state.runIdsByThreadId[currentThreadId] ?? EMPTY_RUN_IDS : EMPTY_RUN_IDS
  );
  // ISSUE-027：只订阅当前 Thread 的事件数组。后台会话有 token 到达时，
  // useShallow 会保持该对象引用稳定，既不重渲染长时间线，也不触发贴底。
  const eventsByRunId = useSurfaceAgentStore(useShallow((state) => Object.fromEntries(
    currentRunIds.map((runId) => [runId, state.eventsByRunId[runId] ?? []])
  )));
  const pendingHermesInput = useMemo(() => {
    for (let runIndex = currentRunIds.length - 1; runIndex >= 0; runIndex--) {
      const runId = currentRunIds[runIndex];
      const events = eventsByRunId[runId] ?? [];
      if (events.some((event) =>
        event.payload.type === 'run_completed' ||
        event.payload.type === 'run_failed' ||
        event.payload.type === 'run_cancelled'
      )) continue;
      for (let eventIndex = events.length - 1; eventIndex >= 0; eventIndex--) {
        const payload = events[eventIndex].payload;
        if (payload.type === 'approval_required' || payload.type === 'clarify_required') {
          return { runId, payload };
        }
      }
    }
    return null;
  }, [currentRunIds, eventsByRunId]);
  const visibleEventRevision = useMemo(
    () => threadEventRevision(currentRunIds, eventsByRunId),
    [currentRunIds, eventsByRunId]
  );
  // 同 run 工具进过程轨；时间线仅挂 diff 等富卡，避免与过程轨重复
  const toolCardsByRunId = useMemo(() => groupToolCardsByRunId(toolCards), [toolCards]);
  const timeline = useMemo<TimelineItem[]>(() => {
    const runStartedAt = new Map(
      messages
        .filter((message) => message.role === 'user')
        .map((message) => [message.runId, message.createdAt])
    );
    return [
      ...messages.map((message) => ({
        kind: 'message' as const,
        at: message.createdAt,
        // 流式气泡在终态时 id 会从 `:streaming` 变为落库 seq。
        // 助手消息以 runId 为稳定 key，避免定稿瞬间卸载/重建整个 Markdown。
        key: message.role === 'assistant' ? `${message.runId}:assistant` : message.id,
        message,
        durationMs: message.role === 'assistant' && runStartedAt.has(message.runId)
          ? Math.max(0, message.createdAt - runStartedAt.get(message.runId)!)
          : undefined,
      })),
      ...timelineToolCards(toolCards).map((card) => ({
        kind: 'tool' as const,
        at: card.startedAt,
        key: `tool-${card.callId}`,
        card,
      })),
    ].sort((a, b) => a.at - b.at);
  }, [messages, toolCards]);
  const latestStart = latestTimelineStart(timeline.length);
  const storedTimelineStart = timelineWindow.threadId === currentThreadId
    ? Math.min(timelineWindow.start, latestStart)
    : latestStart;
  // 跟随态只保留最近窗口；历史阅读态保留用户已经向前加载的范围。
  const timelineStart = timelineFollowingRef.current
    ? Math.max(storedTimelineStart, latestStart)
    : storedTimelineStart;
  const visibleTimeline = useMemo(
    () => timeline.slice(timelineStart),
    [timeline, timelineStart]
  );
  // AG-18：当前 Thread 进行中的 Run（停止按钮可见性 + 取消目标）
  const runningRunId = currentThreadId ? runningRunByThreadId[currentThreadId] : undefined;
  const historyLoading = currentThreadId
    ? (historyLoadingByThreadId[currentThreadId] ?? 0) > 0
    : false;
  const resumingRunId = currentThreadId ? resumingRunByThreadId[currentThreadId] : undefined;
  const conversationLocked = historyLoading || runningRunId != null || resumingRunId != null;
  const findItems = useMemo(
    () => timeline.map((item) => ({
      key: item.key,
      text: item.kind === 'message'
        ? item.message.content
        : [item.card.name, item.card.error, item.card.uiArtifact?.fallbackMarkdown].filter(Boolean).join(' '),
    })),
    [timeline],
  );
  const findMatches = useMemo(() => findTimelineMatches(findItems, findQuery), [findItems, findQuery]);
  const findMatchKey = findOpen && findMatches.length > 0
    ? findItems[findMatches[Math.min(findIndex, findMatches.length - 1)] ?? 0]?.key
    : null;

  useEffect(() => {
    if (!currentThreadId || runningRunId) return;
    let cancelled = false;
    hermesSessionSurface(currentThreadId)
      .then((surface) => { if (!cancelled) setSessionSurface(surface); })
      .catch(() => { if (!cancelled) setSessionSurface(null); });
    return () => { cancelled = true; };
  }, [currentThreadId, runningRunId]);

  useEffect(() => {
    if (!findOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setFindOpen(false);
        return;
      }
      if (event.key === 'Enter' && findMatches.length > 0) {
        event.preventDefault();
        const delta = event.shiftKey ? -1 : 1;
        setFindIndex((current) => (current + delta + findMatches.length) % findMatches.length);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [findOpen, findMatches.length]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== 'f') return;
      const root = chatPanelRef.current;
      if (!root?.contains(document.activeElement)) return;
      event.preventDefault();
      setFindOpen(true);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (!findMatchKey || !timelineViewportRef.current) return;
    const node = timelineViewportRef.current.querySelector(`[data-timeline-key="${findMatchKey.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"]`);
    node?.scrollIntoView({ block: 'center' });
  }, [findMatchKey]);

  // AG-20：事件流显式降级提示（缺口补不齐 / 未知协议 / 坏事件；不静默）
  const degradedReason = useMemo(() => {
    if (!currentThreadId) return null;
    const threadLevel = degraded[`thread:${currentThreadId}`];
    if (threadLevel) return threadLevel;
    for (const runId of currentRunIds) {
      if (degraded[runId]) return degraded[runId];
    }
    return null;
  }, [currentThreadId, currentRunIds, degraded]);

  const scrollTimelineToBottom = useCallback(() => {
    const viewport = timelineViewportRef.current;
    if (!viewport) return;
    viewport.scrollTop = viewport.scrollHeight;
  }, []);

  const handleTimelineScroll = useCallback(() => {
    const viewport = timelineViewportRef.current;
    if (!viewport) return;
    const following = isTimelineNearBottom(viewport);
    timelineFollowingRef.current = following;
    setShowJumpToLatest(!following);

    if (
      !following &&
      viewport.scrollTop <= 64 &&
      timelineStart > 0 &&
      prependAnchorRef.current == null
    ) {
      prependAnchorRef.current = {
        scrollHeight: viewport.scrollHeight,
        scrollTop: viewport.scrollTop,
      };
      setTimelineWindow({
        threadId: currentThreadId,
        start: previousTimelineStart(timelineStart),
      });
    }
  }, [currentThreadId, timelineStart]);

  const jumpToLatest = useCallback(() => {
    timelineFollowingRef.current = true;
    setShowJumpToLatest(false);
    setTimelineWindow({ threadId: currentThreadId, start: latestStart });
    requestAnimationFrame(scrollTimelineToBottom);
  }, [currentThreadId, latestStart, scrollTimelineToBottom]);

  // 切换会话时默认从最新内容开始；历史尚未装载时保留一次初始化贴底。
  const timelineThreadRef = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    timelineFollowingRef.current = true;
    initialScrollThreadRef.current = currentThreadId;
    prependAnchorRef.current = null;
    if (timelineThreadRef.current === undefined) {
      timelineThreadRef.current = currentThreadId;
      return;
    }
    if (timelineThreadRef.current === currentThreadId) return;
    timelineThreadRef.current = currentThreadId;
    setShowJumpToLatest(false);
    // 捕获切换当下的最近窗口；这里不依赖 timeline.length，
    // 否则新消息到达会把正在阅读历史的用户强制拉回底部。
    setTimelineWindow({ threadId: currentThreadId, start: latestTimelineStart(timeline.length) });
    // timeline.length 只取会话切换这一刻的快照，刻意不作为依赖。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentThreadId]);

  // 向前挂载历史后，用高度差恢复原阅读锚点，避免内容在用户眼前跳动。
  useLayoutEffect(() => {
    const anchor = prependAnchorRef.current;
    const viewport = timelineViewportRef.current;
    if (!anchor || !viewport) return;
    viewport.scrollTop = anchor.scrollTop + viewport.scrollHeight - anchor.scrollHeight;
    prependAnchorRef.current = null;
  }, [currentThreadId, timelineStart]);

  useEffect(() => {
    if (
      !currentThreadId ||
      historyLoading ||
      initialScrollThreadRef.current !== currentThreadId ||
      timeline.length === 0
    ) return;
    const frame = requestAnimationFrame(() => {
      scrollTimelineToBottom();
      initialScrollThreadRef.current = null;
    });
    return () => cancelAnimationFrame(frame);
  }, [currentThreadId, historyLoading, timeline.length, scrollTimelineToBottom]);

  // 只有当前 Thread 的可见事件变更才会进入此 effect。用户上滑后暂停，
  // 即使当前 Run 继续生成也不抢滚动；回到底部后下一事件恢复跟随。
  useLayoutEffect(() => {
    if (!currentThreadId) return;
    recordAgentRenderCommit(currentThreadId);
    if (timelineFollowingRef.current) scrollTimelineToBottom();
  }, [currentThreadId, visibleEventRevision, timeline.length, scrollTimelineToBottom]);

  const sessions = useChangeSessionStore((state) => state.sessions);
  const activeOperationByDocument = useChangeSessionStore((state) => state.activeOperationByDocument);
  const loadChangeSessions = useChangeSessionStore((state) => state.loadProject);
  const adoptProposal = useChangeSessionStore((state) => state.adoptProposal);
  useEffect(() => {
    // 持久化恢复仍按真实 project 查询；笔记本 live Patch 由同一 Run 的 diff 事件采纳。
    if (projectId) void loadChangeSessions(projectId);
  }, [projectId, loadChangeSessions]);

  // 工具完成只负责把提案登记到统一状态源。新提案到达时，store 会按文档自动替换旧提案。
  const latestDiffCard = useMemo(() => {
    for (let index = toolCards.length - 1; index >= 0; index--) {
      const view = pickArtifactView(toolCards[index]);
      if (view.mode === 'diff') return { card: toolCards[index], preview: view.preview };
    }
    return null;
  }, [toolCards]);
  const adoptedOperationRef = useRef<string | null>(null);
  useEffect(() => {
    if (!changeScopeId || !latestDiffCard || adoptedOperationRef.current === latestDiffCard.preview.operationId) return;
    adoptedOperationRef.current = latestDiffCard.preview.operationId;
    const context = messages.find(
      (message) => message.runId === latestDiffCard.card.runId && message.role === 'user'
    )?.context ?? null;
    void adoptProposal(latestDiffCard.preview, {
      projectId: changeScopeId,
      threadId: latestDiffCard.card.threadId,
      runId: latestDiffCard.card.runId,
      context,
      createdAt: latestDiffCard.card.completedAt ?? latestDiffCard.card.startedAt,
    });
  }, [latestDiffCard, messages, adoptProposal, changeScopeId]);

  // 时间线中已可见的 diff 提案 id（tool 卡的 uiArtifact 解析）
  const visiblePatchOps = useMemo(() => {
    const s = new Set<string>();
    for (const c of toolCards) {
      const v = pickArtifactView(c);
      if (v.mode === 'diff') s.add(v.preview.operationId);
    }
    return s;
  }, [toolCards]);
  const orphanPatches = useMemo(
    () => Object.values(sessions).filter(
      (session) =>
        changeScopeId != null && currentThreadId != null &&
        session.projectId === changeScopeId && session.threadId === currentThreadId &&
        session.phase === 'proposed' &&
        !visiblePatchOps.has(session.operationId)
    ).sort((a, b) => b.createdAt - a.createdAt),
    [sessions, changeScopeId, currentThreadId, visiblePatchOps]
  );

  const currentChange = activeDocumentId
    ? activeChangeSession({ sessions, activeOperationByDocument }, activeDocumentId)
    : null;
  const continuationContext = useMemo(
    () => continuationContextForChange(currentChange),
    [currentChange]
  );

  // ---- AG-27：Skill 激活与管理 ----
  // 三层清单（bundled + user + workspace）随项目加载；workspace 层按项目隔离，
  // Skill 清单来自 Hermes Runtime；SophoNote 不持有 Skill 正文或启用态。
  const [hermesSkills, setHermesSkills] = useState<HermesSkillInfo[]>(
    () => peekHermesCapabilities()?.skills ?? [],
  );
  const [activeSkill, setActiveSkill] = useState<string | null>(null);
  const autoSelectionSkillKeyRef = useRef<string | null>(null);
  const dismissedSelectionSkillKeyRef = useRef<string | null>(null);
  const selectionSkillKey = selection
    ? `${selection.articleId}:${selection.selectedTextHash}`
    : null;

  const [hermesCapabilitySnapshot, setHermesCapabilitySnapshot] = useState<HermesCapabilities | null>(
    () => peekHermesCapabilities(),
  );
  const capabilityProjectKeyRef = useRef<string | null>(null);
  const refreshSkills = useCallback(() => {
    hermesCapabilities()
      .then((snapshot) => {
        rememberHermesCapabilities(snapshot);
        setHermesCapabilitySnapshot((prev) => (
          prev && JSON.stringify(prev) === JSON.stringify(snapshot) ? prev : snapshot
        ));
        setHermesSkills((prev) => (
          JSON.stringify(prev) === JSON.stringify(snapshot.skills) ? prev : snapshot.skills
        ));
        setBrowserConnected((prev) => (prev === snapshot.browserConnected ? prev : snapshot.browserConnected));
      })
      .catch(() => setHermesSkills((prev) => (prev.length === 0 ? prev : [])));
  }, []);
  useEffect(() => {
    const key = projectId ?? '';
    if (peekHermesCapabilities() && capabilityProjectKeyRef.current === null) {
      capabilityProjectKeyRef.current = key;
      return;
    }
    if (capabilityProjectKeyRef.current === key && peekHermesCapabilities()) return;
    capabilityProjectKeyRef.current = key;
    refreshSkills();
    setActiveSkill(null);
  }, [projectId, refreshSkills]);

  // Runtime 清单刷新后不存在的 Skill 自动清除。
  useEffect(() => {
    if (activeSkill && !hermesSkills.some((skill) => skill.name === activeSkill)) {
      setActiveSkill(null);
    }
  }, [hermesSkills, activeSkill]);

  // 用户明确把编辑器选区加入 Chat 时，默认启用 Markdown 写作 Skill。
  // Skill 正文仍由 Hermes Runtime 加载；SophoNote 只选择原生命令名。
  useEffect(() => {
    if (!selectionSkillKey) {
      if (
        autoSelectionSkillKeyRef.current &&
        activeSkill === SOPHONOTE_MARKDOWN_WRITING_SKILL
      ) {
        setActiveSkill(null);
      }
      autoSelectionSkillKeyRef.current = null;
      dismissedSelectionSkillKeyRef.current = null;
      return;
    }
    if (
      activeSkill == null &&
      dismissedSelectionSkillKeyRef.current !== selectionSkillKey &&
      hermesSkills.some((skill) => skill.name === SOPHONOTE_MARKDOWN_WRITING_SKILL)
    ) {
      autoSelectionSkillKeyRef.current = selectionSkillKey;
      setActiveSkill(SOPHONOTE_MARKDOWN_WRITING_SKILL);
    }
  }, [activeSkill, hermesSkills, selectionSkillKey]);

  const pickSkill = useCallback((name: string | null) => {
    if (selectionSkillKey) {
      dismissedSelectionSkillKeyRef.current =
        name === SOPHONOTE_MARKDOWN_WRITING_SKILL ? null : selectionSkillKey;
    }
    autoSelectionSkillKeyRef.current = null;
    setActiveSkill(name);
    setAttachmentMenuOpen(false);
  }, [selectionSkillKey]);

  const activeSkillInfo = useMemo(
    () => hermesSkills.find((skill) => skill.name === activeSkill) ?? null,
    [hermesSkills, activeSkill]
  );

  const appendAttachments = useCallback((incoming: AgentAttachmentInput[]) => {
    setAttachments((current) => {
      const attachmentKey = (item: AgentAttachmentInput) =>
        `${item.kind}:${item.path ?? item.url ?? item.dataUrl ?? item.id}`;
      const keys = new Set(current.map(attachmentKey));
      const next = [...current];
      for (const item of incoming) {
        const key = attachmentKey(item);
        if (!keys.has(key)) {
          keys.add(key);
          next.push(item);
        }
      }
      return next.slice(0, 20);
    });
    setComposerError(null);
  }, []);

  const pickLocalAttachments = useCallback(async (kind: 'file' | 'image' | 'folder') => {
    try {
      const selected = await open({
        multiple: true,
        directory: kind === 'folder',
        filters: kind === 'image'
          ? [{ name: '图片', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'] }]
          : undefined,
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      appendAttachments(paths.map((path) => ({
        id: attachmentId(),
        kind,
        name: attachmentName(path),
        path,
      })));
      setAttachmentMenuOpen(false);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : '无法打开系统选择器');
    }
  }, [appendAttachments]);

  const appendPastedImage = useCallback(async (blob: Blob, name = '粘贴图片') => {
    if (!blob.type.startsWith('image/')) return;
    try {
      const dataUrl = await blobAsDataUrl(blob);
      appendAttachments([{
        id: attachmentId(),
        kind: 'image',
        name,
        dataUrl,
      }]);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : '读取粘贴图片失败');
    }
  }, [appendAttachments]);

  const appendDroppedFiles = useCallback((files: FileList | File[]) => {
    const incoming: AgentAttachmentInput[] = [];
    const blobs: Array<Promise<void>> = [];
    for (const file of Array.from(files)) {
      const path = (file as File & { path?: string }).path;
      if (path) {
        incoming.push({
          id: attachmentId(),
          kind: droppedPathKind(path),
          name: attachmentName(path),
          path,
        });
        continue;
      }
      if (file.type.startsWith('image/')) {
        blobs.push(appendPastedImage(file, file.name || '拖放图片'));
      }
    }
    if (incoming.length > 0) appendAttachments(incoming);
    if (blobs.length > 0) void Promise.all(blobs);
  }, [appendAttachments, appendPastedImage]);

  const appendDroppedPaths = useCallback((paths: string[]) => {
    if (paths.length === 0) return;
    appendAttachments(paths.map((path) => ({
      id: attachmentId(),
      kind: droppedPathKind(path),
      name: attachmentName(path),
      path,
    })));
  }, [appendAttachments]);

  const acceptHtmlDrop = useCallback((event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDropActive(false);
    if (Date.now() - nativeDropAtRef.current < 400) return;
    if (event.dataTransfer.files.length > 0) appendDroppedFiles(event.dataTransfer.files);
  }, [appendDroppedFiles]);

  useEffect(() => {
    if (surfaceTab === 'browser' || !isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const dropHitsPanel = (position: { x: number; y: number }) => {
      const root = chatPanelRef.current;
      if (!root) return false;
      return physicalPointInCssRect(position, root.getBoundingClientRect(), window.devicePixelRatio || 1);
    };
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'enter' || event.payload.type === 'over') {
        setDropActive(dropHitsPanel(event.payload.position));
        return;
      }
      if (event.payload.type === 'leave') {
        setDropActive(false);
        return;
      }
      if (event.payload.type === 'drop') {
        setDropActive(false);
        if (!dropHitsPanel(event.payload.position)) return;
        nativeDropAtRef.current = Date.now();
        appendDroppedPaths(event.payload.paths);
      }
    }).then((nextUnlisten) => {
      if (disposed) nextUnlisten();
      else unlisten = nextUnlisten;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [appendDroppedPaths, surfaceTab]);

  const pasteImageFromClipboard = useCallback(async () => {
    try {
      const clipboard = await navigator.clipboard.read();
      for (const item of clipboard) {
        const type = item.types.find((candidate) => candidate.startsWith('image/'));
        if (!type) continue;
        await appendPastedImage(await item.getType(type));
        setAttachmentMenuOpen(false);
        return;
      }
      setComposerError('剪贴板中没有图片，也可以在输入框中直接按 ⌘V');
    } catch {
      setComposerError('无法直接读取剪贴板，请在输入框中按 ⌘V 粘贴图片');
    }
  }, [appendPastedImage]);

  const commitUrl = useCallback(() => {
    const value = urlDraft.trim();
    if (!/^https?:\/\/\S+$/i.test(value)) {
      setComposerError('请输入以 http:// 或 https:// 开头的 URL');
      return;
    }
    appendAttachments([{
      id: attachmentId(),
      kind: 'url',
      name: value,
      url: value,
    }]);
    setUrlDraft('');
    setUrlEditorOpen(false);
    setAttachmentMenuOpen(false);
  }, [appendAttachments, urlDraft]);

  // 发送消息
  const handleSend = async (text: string) => {
    if ((!text.trim() && attachments.length === 0) || conversationLocked) return false;
    if (!selectedHermesProvider || !selectedHermesModel) {
      setComposerError('当前没有已配置且可用的模型，请到「设置 → AI 模型」完成配置。');
      return false;
    }
    // Skill 名只作为 Hermes 原生命令引用透传；发送后保留供多轮复用。
    let focusDocument: FocusDocumentInput | null = null;
    // 显式选区/变更续接优先，绝不把范围静默扩大到整篇。没有选区时，界面
    // 可见的当前文档 chip 才解析发送时草稿并交给 Hermes 原生 file.attach。
    if (activeDocumentId && !includeProjectContext && !(selection ?? continuationContext)) {
      try {
        focusDocument = await resolveActiveDocumentContext?.() ?? null;
        if (!focusDocument) {
          setComposerError('当前文档已切换或暂时无法读取，请确认文档后重试。');
          return false;
        }
      } catch (error) {
        setComposerError(`读取当前文档失败：${String(error)}`);
        return false;
      }
    }
    // selection 由 Rust 通过 Hermes 原生 file.attach 提交；SophoNote 不拼接提示词。
    const slashName = text.trim().match(/^\/([^\s]+)/)?.[1]?.toLocaleLowerCase() ?? null;
    const hermesCommand = slashName && (
      hermesCapabilitySnapshot?.commands.some((item) => item.name.replace(/^\//, '').toLocaleLowerCase() === slashName) ||
      hermesSkills.some((item) => item.name.toLocaleLowerCase() === slashName)
    ) ? text.trim() : null;
    const runAttachments = workspaceRoot
      ? [
          {
            id: `workspace:${workspaceRoot}`,
            kind: 'folder' as const,
            name: attachmentName(workspaceRoot),
            path: workspaceRoot,
          },
          ...attachments.filter((attachment) => !(attachment.kind === 'folder' && attachment.path === workspaceRoot)),
        ]
      : attachments;
    const result = await startRun(
      currentThreadId,
      text,
      projectId ?? undefined,
      selection ?? continuationContext,
      hermesCommand ? null : activeSkill,
      focusDocument,
      runAttachments,
      selectedHermesModel,
      selectedHermesProvider,
      hermesCommand,
      includeProjectContext,
      workspaceRoot,
      effectivePermissionMode,
    );
    if (result) {
      // 新 Thread 创建后切换到该 Thread
      setLocalDraftSession(false);
      onDraftSessionChange?.(false);
      setActiveThreadId(result.threadId);
      selectThread(result.threadId);
      onThreadCreated?.(result.threadId);
      void loadThreads(projectId ?? undefined, 'active');
      // chip 已随本次 Run 消费（上下文进入 run_started 事件，消息头可回溯）
      onClearSelection?.();
      setAttachments([]);
      setComposerError(null);
      return true;
    }
    return false;
  };

  const switchToThread = async (threadId: string) => {
    setLocalDraftSession(false);
    onDraftSessionChange?.(false);
    setAttachments([]);
    setComposerError(null);
    setActiveThreadId(threadId);
    selectThread(threadId);
    historyRestoredRef.current = threadId;
    await loadThreadHistory(threadId);
  };

  const handleNewSession = async () => {
    if (hasProjectScope) {
      setLocalDraftSession(true);
      onDraftSessionChange?.(true);
      setActiveThreadId(null);
      selectThread(null);
      setAttachments([]);
      setComposerError(null);
      return;
    }
    if (creatingThreadRef.current) return;
    creatingThreadRef.current = true;
    const previousThreadId = currentThreadId;
    setCreatingThread(true);
    setActiveThreadId(null);
    selectThread(null);
    setAttachments([]);
    setComposerError(null);
    try {
      const id = await createThread(projectId ?? undefined, '新会话');
      if (!id) {
        setActiveThreadId(previousThreadId);
        selectThread(previousThreadId);
        return;
      }
      setActiveThreadId(id);
      historyRestoredRef.current = id;
    } finally {
      creatingThreadRef.current = false;
      setCreatingThread(false);
    }
  };

  const toggleSessionYolo = useCallback(async () => {
    if (!currentThreadId) {
      setComposerError('发送第一轮后即可开关本轮 YOLO');
      return;
    }
    if (conversationLocked) {
      setComposerError('当前回合尚未结束，暂时不能切换 YOLO');
      return;
    }
    try {
      const enabled = await hermesSessionSetYolo(currentThreadId, !sessionSurface?.yolo);
      setSessionSurface((current) => current
        ? { ...current, yolo: enabled }
        : { yolo: enabled, contextUsed: null, contextMax: null, contextPercent: null });
      setSessionNotice(enabled ? '本轮 YOLO 已打开：危险命令将自动批准' : '本轮 YOLO 已关闭：危险命令需要审批');
      setComposerError(null);
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : '无法切换 YOLO');
    }
  }, [conversationLocked, currentThreadId, sessionSurface?.yolo]);

  const runHermesUndo = useCallback(async (command: string) => {
    if (!currentThreadId) {
      setComposerError('当前会话还没有可撤回的 Hermes 对话');
      return;
    }
    if (conversationLocked) {
      setComposerError('当前回合尚未结束，请先停止后再 /undo');
      return;
    }
    try {
      const result = await hermesSessionSlash(currentThreadId, command);
      if (result.kind === 'prefill') {
        forgetThreadView(currentThreadId);
        await loadThreadHistory(currentThreadId);
        setComposerPrefill({ nonce: Date.now(), text: result.message });
        setSessionNotice(result.notice ?? '已撤回上一轮 Hermes 对话');
        setComposerError(null);
        return;
      }
      setComposerError(result.message || 'Hermes 命令已执行');
    } catch (error) {
      setComposerError(error instanceof Error ? error.message : '无法执行 /undo');
    }
  }, [conversationLocked, currentThreadId, forgetThreadView, loadThreadHistory]);

  const handleComposerCommand = useCallback((command: string): boolean => {
    const name = command.trim().split(/\s+/, 1)[0]?.toLocaleLowerCase();
    if (name === '/new') { void handleNewSession(); return true; }
    if (name === '/model' || name === '/models') { setModelMenuOpen(true); return true; }
    if (name === '/stop') {
      if (runningRunId) void cancelRun(runningRunId);
      return true;
    }
    const control = isSessionControlCommand(command);
    if (control === 'undo') {
      void runHermesUndo(command);
      return true;
    }
    if (control === 'yolo') {
      void toggleSessionYolo();
      return true;
    }
    return false;
  }, [cancelRun, handleNewSession, runningRunId, runHermesUndo, toggleSessionYolo]);

  const handleComposerReference = useCallback((reference: string): string => {
    if (reference === '@file:') { void pickLocalAttachments('file'); return ''; }
    if (reference === '@folder:') { void pickLocalAttachments('folder'); return ''; }
    if (reference === '@url:') {
      setUrlEditorOpen(true);
      setAttachmentMenuOpen(false);
      return '';
    }
    return `${reference} `;
  }, [pickLocalAttachments]);

  const handleCloseSession = async (threadId: string, e?: MouseEvent) => {
    e?.stopPropagation();
    const ok = await closeThread(threadId, projectId ?? undefined);
    if (!ok) return;
    const next = resolveProjectThreadId(
      useAgentStore.getState().threads,
      scopedProjectId,
      null
    );
    if (next) {
      await switchToThread(next);
    } else {
      setActiveThreadId(null);
      selectThread(null);
    }
  };

  const handleOpenHistorySession = async (thread: AgentThread) => {
    setHistoryOpen(false);
    const ok = await reopenThread(thread.id, projectId ?? undefined);
    if (!ok) return;
    await switchToThread(thread.id);
  };

  const handleArchiveHistory = async (threadId: string, e: MouseEvent) => {
    e.stopPropagation();
    await archiveThread(threadId, projectId ?? undefined);
  };

  const threadTitle = (t: AgentThread) => {
    const raw = (t.title || '').trim() || '新会话';
    return raw.length > 18 ? `${raw.slice(0, 18)}…` : raw;
  };

  return (
    <aside
      ref={chatPanelRef}
      className={`w-full h-full bg-[var(--bg-surface)] flex flex-col relative ${layout === 'panel' ? 'border-l border-[var(--border-default)]' : ''}`}
    >
      {showBrowserTab && <header className="h-9 shrink-0 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-2 flex items-center gap-1">
        <button type="button" onClick={() => setSurfaceTab('chat')} className={`h-7 rounded-md px-2.5 inline-flex items-center gap-1.5 text-xs font-medium ${surfaceTab === 'chat' ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]'}`}>
          <MessageSquareText size={12} /> AI 对话
        </button>
        <button type="button" onClick={() => setSurfaceTab('browser')} className={`h-7 rounded-md px-2.5 inline-flex items-center gap-1.5 text-xs font-medium ${surfaceTab === 'browser' ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]'}`}>
          <Globe2 size={12} /> 浏览器
          <span className={`h-1.5 w-1.5 rounded-full ${browserConnected ? 'bg-[var(--success)]' : 'bg-[var(--border-strong)]'}`} />
        </button>
      </header>}
      {showBrowserTab && surfaceTab === 'browser' ? (
        <AgentBrowserPanel
          onConnectionChange={setBrowserConnected}
          onAddToChat={(target) => {
            appendAttachments([target.localPath ? {
              id: attachmentId(),
              kind: 'file',
              name: target.name,
              path: target.localPath,
            } : {
              id: attachmentId(),
              kind: 'url',
              name: target.name,
              url: target.url,
            }]);
            setSurfaceTab('chat');
          }}
        />
      ) : (
      <>
      {/* AG-01：左会话 tab · 右 + / 历史；能力管理位于设置页。 */}
      {showThreadNavigation && <header className="px-2 h-10 border-b border-[var(--border-default)] flex items-center gap-1 shrink-0 min-w-0">
        <div className="flex-1 min-w-0 flex items-center gap-1 overflow-x-auto">
          {activeProjectThreads.length === 0 ? (
            <div className="flex items-center gap-1.5 px-2 text-[var(--text-tertiary)] shrink-0">
              <Sparkles size={13} className="text-[var(--accent)]" />
              <span className="text-xs text-[var(--text-tertiary)]">新会话</span>
            </div>
          ) : (
            activeProjectThreads.map((t) => {
              const active = t.id === currentThreadId;
              return (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => void switchToThread(t.id)}
                  className={`group flex items-center gap-1 max-w-[160px] shrink-0 rounded-md px-2 py-1 text-xs transition-colors ${
                    active
                      ? 'bg-[var(--bg-sunken)] text-[var(--text-primary)]'
                      : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
                  }`}
                  title={t.title || '新会话'}
                >
                  <Sparkles size={11} className={active ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'} />
                  <span className="truncate">{threadTitle(t)}</span>
                  <span
                    role="button"
                    tabIndex={0}
                    onClick={(e) => void handleCloseSession(t.id, e)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        void handleCloseSession(t.id);
                      }
                    }}
                    className="ml-0.5 p-0.5 rounded text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] opacity-70 group-hover:opacity-100"
                    title="关闭到历史"
                  >
                    <X size={11} />
                  </span>
                </button>
              );
            })
          )}
        </div>
        <div className="flex items-center gap-0.5 shrink-0 relative">
          <button
            type="button"
            onClick={() => void handleNewSession()}
            disabled={creatingThread}
            className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] disabled:cursor-wait disabled:opacity-50"
            title="新建会话"
          >
            {creatingThread ? <Loader2 size={14} className="animate-spin" /> : <Plus size={14} />}
          </button>
          <button
            type="button"
            onClick={() => {
              setHistoryOpen((v) => !v);
              void loadThreads(projectId ?? undefined, 'history');
            }}
            className={`w-7 h-7 rounded-md flex items-center justify-center hover:bg-[var(--bg-sunken)] ${
              historyOpen ? 'text-[var(--accent)] bg-[var(--bg-sunken)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'
            }`}
            title="历史会话"
          >
            <Clock size={14} />
          </button>
          {historyOpen && (
            <>
              <div className="fixed inset-0 z-30" onClick={() => setHistoryOpen(false)} />
              <div className="absolute right-0 top-9 z-40 w-72 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] py-1 max-h-80 overflow-y-auto">
                <p className="px-3 py-1.5 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider">
                  历史会话
                </p>
                {projectHistoryThreads.length === 0 ? (
                  <p className="px-3 py-4 text-xs text-[var(--text-tertiary)] text-center">暂无历史会话</p>
                ) : (
                  projectHistoryThreads.map((t) => (
                    <div
                      key={t.id}
                      className="flex items-center gap-1 px-2 py-1.5 hover:bg-[var(--bg-sunken)]"
                    >
                      <button
                        type="button"
                        onClick={() => void handleOpenHistorySession(t)}
                        className="flex-1 min-w-0 text-left text-xs text-[var(--text-secondary)] truncate px-1"
                        title="恢复并打开"
                      >
                        {t.title?.trim() || '未命名会话'}
                      </button>
                      <button
                        type="button"
                        onClick={(e) => void handleArchiveHistory(t.id, e)}
                        className="shrink-0 p-1 rounded text-[var(--text-tertiary)] hover:text-[var(--warning)] hover:bg-[var(--warning-subtle)]"
                        title="归档"
                      >
                        <Archive size={12} />
                      </button>
                    </div>
                  ))
                )}
              </div>
            </>
          )}

        </div>
      </header>}

      <div
        className={`relative flex-1 min-h-0 ${dropActive ? 'ring-2 ring-inset ring-[var(--accent)]' : ''}`}
        onDragOver={(event) => {
          event.preventDefault();
          setDropActive(true);
        }}
        onDragLeave={(event) => {
          if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
          setDropActive(false);
        }}
        onDrop={acceptHtmlDrop}
      >
      {findOpen && (
        <ConversationFindBar
          query={findQuery}
          matchCount={findMatches.length}
          activeIndex={findIndex}
          onQueryChange={(value) => {
            setFindQuery(value);
            setFindIndex(0);
          }}
          onNext={() => setFindIndex((current) => findMatches.length === 0 ? 0 : (current + 1) % findMatches.length)}
          onPrev={() => setFindIndex((current) => findMatches.length === 0 ? 0 : (current - 1 + findMatches.length) % findMatches.length)}
          onClose={() => setFindOpen(false)}
        />
      )}
      <div
        ref={timelineViewportRef}
        onScroll={handleTimelineScroll}
        className="absolute inset-0 overflow-y-auto p-3"
      >
        {degradedReason && (
          <div className="mb-2 px-3 py-1.5 rounded-lg bg-[var(--warning-subtle)] border border-[var(--gold-border)] text-xs text-[var(--warning)] leading-relaxed">
            事件流已降级：{degradedReason}
          </div>
        )}
        {orphanPatches.length > 0 && (
          <div className="mb-3 space-y-2">
            <p className="text-xs font-medium text-[var(--text-tertiary)] px-1">
              待处理修改（自历史记录恢复）
            </p>
            {orphanPatches.map((session) => (
              <DiffResultNotice key={session.operationId} preview={session.preview} />
            ))}
          </div>
        )}
        {timeline.length === 0 && orphanPatches.length === 0 && runningRunId == null ? (
          <EmptyState
            icon={MessageSquareText}
            title={emptyHint ?? (hasProjectScope
              ? '以当前本地项目为工作范围，用自然语言完成代码与文档任务。'
              : '直接向 Hermes Agent 描述任务，过程与结果会保留在当前会话中。')}
          />
        ) : (
          <ChatTimeline
            items={visibleTimeline}
            toolCardsByRunId={toolCardsByRunId}
            eventsByRunId={eventsByRunId}
            running={runningRunId != null}
            highlightKey={findMatchKey}
          />
        )}
      </div>
      {showJumpToLatest && (
        <button
          type="button"
          onClick={jumpToLatest}
          className="absolute bottom-3 left-1/2 -translate-x-1/2 z-10 h-8 px-3 rounded-full border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-md)] text-xs text-[var(--text-secondary)] hover:text-[var(--accent)] hover:border-[var(--accent-border)] flex items-center gap-1.5 backdrop-blur"
          aria-label="回到最新消息"
        >
          <ArrowDown size={13} /> 回到最新
        </button>
      )}
      </div>

      <div
        className="p-3 border-t border-[var(--border-default)] shrink-0"
        onDragOver={(event) => {
          event.preventDefault();
          setDropActive(true);
        }}
        onDrop={acceptHtmlDrop}
      >
        {sessionNotice && (
          <p className="mb-2 text-xs text-[var(--text-tertiary)]">{sessionNotice}</p>
        )}
        {pendingHermesInput && (
          <HermesInputRequest
            request={pendingHermesInput}
            onApproval={(choice) => respondApproval(pendingHermesInput.runId, choice)}
            onClarify={(requestId, answer) =>
              respondClarify(pendingHermesInput.runId, requestId, answer)
            }
          />
        )}
        <div className="relative">
          {attachmentMenuOpen && (
            <AttachmentPickerPopup
              onPickFile={() => void pickLocalAttachments('file')}
              onPickFolder={() => void pickLocalAttachments('folder')}
              onPickImage={() => void pickLocalAttachments('image')}
              onPasteImage={() => void pasteImageFromClipboard()}
              onPickUrl={() => {
                setUrlEditorOpen(true);
                setAttachmentMenuOpen(false);
              }}
              skills={hermesSkills}
              activeSkill={activeSkill}
              onPickSkill={pickSkill}
              onClose={() => setAttachmentMenuOpen(false)}
            />
          )}
          {urlEditorOpen && (
            <UrlAttachmentEditor
              value={urlDraft}
              onChange={setUrlDraft}
              onSubmit={commitUrl}
              onClose={() => setUrlEditorOpen(false)}
            />
          )}
          {modelMenuOpen && (
            <HermesModelPicker
              providers={hermesModels?.providers ?? []}
              activeProvider={selectedHermesProvider}
              activeModel={selectedHermesModel}
              error={hermesModelError}
              onPick={rememberHermesModel}
              onRetry={() => { refreshHermesModels(); }}
              onClose={() => setModelMenuOpen(false)}
            />
          )}
          <ChatComposer
            placeholder={
              historyLoading
                ? '正在恢复上一轮会话…'
                : resumingRunId
                  ? '正在接续上一轮回复，完成前暂不能继续提问…'
                  : runningRunId
                    ? '回复生成中，完成前暂不能继续提问…'
                    : selection
                      ? '对选中内容下指令…'
                      : continuationContext
                        ? '继续调整当前修改…'
                        : composerPlaceholder ?? (hasProjectScope ? '问项目…' : '向 Hermes Agent 下达任务…')
            }
            onSend={handleSend}
            commands={hermesCapabilitySnapshot?.commands ?? []}
            skills={hermesSkills}
            references={hermesCapabilitySnapshot?.references ?? []}
            onPickSkill={pickSkill}
            onPickReference={handleComposerReference}
            onCommand={handleComposerCommand}
            prefill={composerPrefill}
            composerKey={currentThreadId ?? 'draft'}
            canSubmit={attachments.length > 0}
            running={runningRunId != null}
            locked={conversationLocked}
            lockLabel={
              historyLoading
                ? '正在恢复会话'
                : resumingRunId
                  ? '正在接续上一轮回复'
                  : runningRunId
                    ? '上一轮回复尚未完成'
                    : undefined
            }
            onStop={() => {
              if (runningRunId) void cancelRun(runningRunId);
            }}
            selectionChip={(workspaceRoot || selection || continuationContext || (activeDocumentId && activeDocumentTitle) || (includeProjectContext && hasProjectScope)) ? (
              <>
                {workspaceRoot && (
                  <ComposerWorkspaceChip
                    root={workspaceRoot}
                    onClear={onClearWorkspace}
                  />
                )}
                {selection ? (
                  <ComposerSelectionChip
                    context={selection}
                    lines={selectionLines}
                    onClear={onClearSelection}
                  />
                ) : continuationContext && currentChange ? (
                  <ContinuationChangeChip title={currentChange.preview.title} />
                ) : activeDocumentId && activeDocumentTitle && !includeProjectContext ? (
                  <ComposerDocumentChip title={activeDocumentTitle} />
                ) : includeProjectContext && hasProjectScope ? (
                  <ComposerProjectChip name={projectName} onClear={onClearProjectContext} />
                ) : null}
              </>
            ) : null}
            skillChip={
              activeSkillInfo ? (
                <ComposerSkillChip skill={activeSkillInfo} onClear={() => pickSkill(null)} />
              ) : null
            }
            attachmentChips={attachments.map((attachment) => (
              <ComposerAttachmentChip
                key={attachment.id}
                attachment={attachment}
                onClear={() => setAttachments((items) => items.filter((item) => item.id !== attachment.id))}
              />
            ))}
            error={composerError}
            onPasteImage={(file) => void appendPastedImage(file, file.name || '粘贴图片')}
            leftSlot={
              <>
                <button
                  onClick={() => {
                    setModelMenuOpen(false);
                    setUrlEditorOpen(false);
                    setAttachmentMenuOpen((value) => !value);
                  }}
                  disabled={conversationLocked}
                  title="添加附件"
                  className={`w-7 h-7 rounded-lg flex items-center justify-center transition-colors disabled:opacity-40 disabled:pointer-events-none ${
                    attachmentMenuOpen || attachments.length > 0
                      ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                      : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
                  }`}
                >
                  <Plus size={14} />
                </button>
                <ComposerPermissionControl
                  value={effectivePermissionMode}
                  onChange={changePermissionMode}
                  disabled={conversationLocked}
                />
                <button
                  type="button"
                  onClick={() => void toggleSessionYolo()}
                  disabled={!currentThreadId || conversationLocked}
                  title={
                    !currentThreadId
                      ? '发送第一轮后可开关本轮 YOLO'
                      : sessionSurface?.yolo
                        ? '本轮 YOLO 开：危险命令将自动批准'
                        : '本轮 YOLO 关：危险命令需要审批'
                  }
                  className={`inline-flex h-7 items-center gap-1 rounded-lg px-1.5 text-xs font-medium transition-colors hover:bg-[var(--bg-sunken)] disabled:pointer-events-none disabled:opacity-40 ${
                    sessionSurface?.yolo ? 'text-[var(--warning)]' : 'text-[var(--text-tertiary)]'
                  }`}
                >
                  <Zap size={13} />
                  YOLO
                </button>
              </>
            }
            rightSlot={
              <>
                {sessionSurface?.contextPercent != null && (
                  <ContextOccupancyBar
                    percent={sessionSurface.contextPercent}
                    used={sessionSurface.contextUsed}
                    max={sessionSurface.contextMax}
                  />
                )}
                <button
                type="button"
                onClick={() => {
                  setAttachmentMenuOpen(false);
                  setUrlEditorOpen(false);
                  setModelMenuOpen((value) => !value);
                }}
                disabled={conversationLocked}
                className="h-7 max-w-[168px] inline-flex items-center gap-1 rounded-lg px-2 text-xs text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] disabled:opacity-40"
                title={selectedHermesProvider
                  ? `${selectedHermesProvider} · ${selectedHermesModel}`
                  : hermesModelError ?? '正在读取 Hermes Runtime 模型'}
              >
                <span className="truncate">{selectedHermesModel || '未配置模型'}</span>
                <ChevronDown size={10} className="shrink-0" />
              </button>
              </>
            }
          />
        </div>
      </div>
      </>
      )}
    </aside>
  );
}

/** 时间线条目：消息与工具卡按时间合并（AG-21；AG-26 起消息携带选区上下文） */
type TimelineItem =
  | { kind: 'message'; at: number; key: string; message: AgentMessage; durationMs?: number }
  | { kind: 'tool'; at: number; key: string; card: ToolCard };

/** 选区摘录（chip 展示用）：压缩空白 + 截断 */
function selectionSnippet(markdown: string, max = 40): string {
  const flat = markdown.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
}

/** 消息头范围 chip（AG-26 验收场景②：Chat 显示绑定的文章/选区/版本） */
function SelectionChip({ context }: { context: RunContext }) {
  return (
    <div className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-[var(--accent-subtle)] border border-[var(--accent-border)] text-xs text-[var(--accent)] max-w-full">
      <span className="font-medium truncate max-w-[140px]" title={context.title}>
        《{context.title || '未命名文档'}》
      </span>
      <span className="text-[var(--accent)] shrink-0">v{context.baseVersion}</span>
      <span className="text-[var(--accent)] truncate" title={context.selectedMarkdown}>
        · {selectionSnippet(context.selectedMarkdown, 32)}
      </span>
    </div>
  );
}

/** AG-27 来源展示口径（管理面板/选择器/消息头共用） */
function skillSourceLabel(source: string): string {
  if (source === 'hermes') return 'Hermes';
  if (source === 'bundled') return '内置';
  if (source === 'user') return '用户';
  if (source === 'workspace') return '项目';
  return source;
}

/** 消息头技能 chip（AG-27 验收「Run 可见版本与来源」：name vX · 来源） */
function SkillRunChip({ skill }: { skill: RunSkillRef }) {
  return (
    <div className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-[var(--skill-subtle)] border border-[var(--skill-border)] text-xs text-[var(--skill)]">
      <Sparkles size={10} className="shrink-0" />
      <span className="font-medium truncate max-w-[140px]" title={skill.name}>
        {skill.name}
      </span>
      <span className="text-[color-mix(in_srgb,var(--skill)_70%,transparent)] shrink-0">v{skill.version}</span>
      <span className="text-[color-mix(in_srgb,var(--skill)_70%,transparent)] shrink-0">· {skillSourceLabel(skill.source)}</span>
    </div>
  );
}

/** 时间线：渲染消息 + 富工具结果卡；同 run 过程步进助手气泡过程轨 */
const ChatTimeline = memo(function ChatTimeline({
  items,
  toolCardsByRunId,
  eventsByRunId,
  running = false,
  highlightKey = null,
}: {
  items: TimelineItem[];
  toolCardsByRunId: Record<string, ToolCard[]>;
  eventsByRunId: Record<string, AgentEvent[]>;
  running?: boolean;
  highlightKey?: string | null;
}) {
  return (
    <div className="space-y-3">
      {items.map((item) =>
        item.kind === 'message' ? (
          <div
            key={item.key}
            data-timeline-key={item.key}
            className={highlightKey === item.key ? 'rounded-xl ring-2 ring-[var(--accent)]' : undefined}
          >
          <ChatMessageView
            message={item.message}
            durationMs={item.durationMs}
            streaming={running && item.message.id.endsWith(':streaming')}
            processCards={
              item.message.role === 'assistant'
                ? toolCardsByRunId[item.message.runId] ?? EMPTY_TOOL_CARDS
                : EMPTY_TOOL_CARDS
            }
            processStartedAt={
              item.message.role === 'assistant'
                ? eventsByRunId[item.message.runId]?.find(
                  (event) => event.payload.type === 'run_started'
                )?.timestamp
                : undefined
            }
          />
          </div>
        ) : (
          <div
            key={item.key}
            data-timeline-key={item.key}
            className={`hb-chat-timeline-item flex justify-start ${highlightKey === item.key ? 'rounded-xl ring-2 ring-[var(--accent)]' : ''}`}
          >
            <ToolCardView card={item.card} />
          </div>
        )
      )}
    </div>
  );
});

function formatRunDuration(ms: number): string {
  if (ms < 1000) return '<1 秒';
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const rest = seconds % 60;
  return rest === 0 ? `${minutes} 分钟` : `${minutes} 分 ${rest} 秒`;
}

const ChatMessageView = memo(function ChatMessageView({
  message,
  durationMs,
  streaming = false,
  processCards = EMPTY_TOOL_CARDS,
  processStartedAt,
}: {
  message: AgentMessage;
  durationMs?: number;
  /** 流式未定稿：过程轨默认展开 */
  streaming?: boolean;
  /** 同 runId 工具步骤（进过程轨；富卡仍可另挂时间线） */
  processCards?: ToolCard[];
  processStartedAt?: number;
}) {
  if (message.role === 'user') {
    return (
      <article className="hb-chat-timeline-item w-full">
        {(message.context || message.skill) && (
          <div className="mb-1.5 flex flex-wrap gap-1 items-center">
            {message.context && <SelectionChip context={message.context} />}
            {message.skill && <SkillRunChip skill={message.skill} />}
          </div>
        )}
        <div className="w-fit max-w-[78ch] rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] px-3 py-2.5 text-[13px] leading-relaxed text-[var(--text-primary)] whitespace-pre-wrap shadow-[var(--shadow-sm)]">
          {message.content}
        </div>
      </article>
    );
  }
  const hasAnswer = Boolean(message.content.trim());
  const phase: AssistantPhase =
    message.phase ??
    (streaming ? (hasAnswer ? 'answering' : 'thinking') : 'done');
  const showAreaBContent = shouldRenderAreaBContent({
    phase,
    hasContent: hasAnswer,
  });
  return (
    <article className="hb-chat-timeline-item w-full">
      <div className="w-full px-1 py-1">
        <ProcessRail
          phase={phase}
          durationMs={durationMs}
          processCards={processCards}
          startedAt={processStartedAt}
        />
        <div className="mt-2 w-full text-[13px] leading-relaxed text-[var(--text-secondary)]" role="region" aria-label="回复">
          {showAreaBContent ? (
            <StreamingMarkdownBody content={message.content} streaming={phase === 'answering'} />
          ) : phase === 'answering' && !hasAnswer ? (
            <p className="text-[12px] text-[var(--text-tertiary)]">正在生成回复…</p>
          ) : null}
        </div>
      </div>
    </article>
  );
}, (previous, next) => (
  previous.message.id === next.message.id &&
  previous.message.content === next.message.content &&
  previous.message.phase === next.message.phase &&
  previous.message.thinkingStatus === next.message.thinkingStatus &&
  previous.message.contentStatus === next.message.contentStatus &&
  previous.message.context === next.message.context &&
  previous.message.skill === next.message.skill &&
  previous.durationMs === next.durationMs &&
  previous.streaming === next.streaming &&
  previous.processCards === next.processCards &&
  previous.processStartedAt === next.processStartedAt
));

type ExecutionStep = {
  key: string;
  label: string;
  count: number;
  status: ToolCard['status'];
};

/**
 * 执行区只展示用户能理解的业务动作。原始 reasoning、工具名称、参数和返回值
 * 都不进入 UI；重复动作归并为一行，避免形成「Used N tools」式噪音。
 */
function executionSteps(cards: ToolCard[]): ExecutionStep[] {
  const steps = new Map<string, ExecutionStep>();
  for (const card of cards) {
    const summary = toolStepSummary(card);
    const label = `${toolDisplayName(card.name)}${summary ? ` · ${summary}` : ''}`;
    const key = label.toLocaleLowerCase();
    const existing = steps.get(key);
    if (!existing) {
      steps.set(key, { key: card.callId, label, count: 1, status: card.status });
      continue;
    }
    existing.count += 1;
    if (card.status === 'running' || card.status === 'failed') existing.status = card.status;
  }
  return [...steps.values()];
}

/** 单个 Run 只有一个固定高度、可折叠的执行区。 */
function ProcessRail({
  phase,
  durationMs,
  processCards,
  startedAt,
}: {
  phase: AssistantPhase;
  durationMs?: number;
  processCards: ToolCard[];
  startedAt?: number;
}) {
  const steps = useMemo(() => executionSteps(processCards), [processCards]);
  const running = phase === 'thinking' || phase === 'answering';
  const now = useActivityNow(running);
  const elapsedMs = durationMs ?? (startedAt ? Math.max(0, now - startedAt) : undefined);
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  const canExpand = steps.length > 0;
  const open = canExpand && (userOpen ?? running);

  useEffect(() => {
    // 运行时自动展开，完成后自动收成一行；此后保留用户的手动选择。
    setUserOpen(null);
  }, [running]);

  const currentStep = [...processCards].reverse().find((card) => card.status === 'running');
  const label = phase === 'error'
    ? '执行未完成'
    : phase === 'done'
      ? '已完成'
      : currentStep
        ? `正在${toolDisplayName(currentStep.name)}`
        : phase === 'answering'
          ? '正在生成回复'
          : '正在理解任务';

  return (
    <section className="hb-chat-process" role="region" aria-label="执行进度">
      <button
        type="button"
        onClick={() => canExpand && setUserOpen(!open)}
        className={`hb-chat-process-summary ${canExpand ? 'cursor-pointer' : 'cursor-default'}`}
        aria-expanded={canExpand ? open : undefined}
      >
        {running ? (
          <Loader2 size={12} className="shrink-0 animate-spin text-[var(--text-tertiary)]" />
        ) : phase === 'error' ? (
          <X size={12} className="shrink-0 text-[var(--danger)]" />
        ) : (
          <Check size={12} className="shrink-0 text-[var(--success)]" />
        )}
        <span className="font-medium text-[var(--text-secondary)]">{label}</span>
        {steps.length > 0 ? <span>{steps.length} 项操作</span> : null}
        {elapsedMs != null ? <span className="tabular-nums">{formatRunDuration(elapsedMs)}</span> : null}
        {canExpand ? (
          <ChevronDown size={12} className={`ml-auto shrink-0 transition-transform ${open ? '' : '-rotate-90'}`} />
        ) : null}
      </button>
      {open ? (
        <ol className="hb-chat-process-steps">
          {steps.map((step) => (
            <li key={step.key} className="flex min-w-0 items-start gap-2">
              {step.status === 'running' ? (
                <Loader2 size={11} className="mt-0.5 shrink-0 animate-spin" />
              ) : step.status === 'failed' ? (
                <X size={11} className="mt-0.5 shrink-0 text-[var(--danger)]" />
              ) : (
                <Check size={11} className="mt-0.5 shrink-0 text-[var(--success)]" />
              )}
              <span className="min-w-0 break-words">
                {step.label}{step.count > 1 ? ` ×${step.count}` : ''}
              </span>
            </li>
          ))}
        </ol>
      ) : null}
    </section>
  );
}

function useActivityNow(running: boolean): number {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);
  return now;
}

/** 流式事件已在 store 按视觉帧合并；这里直接渲染最新快照。 */
const StableMarkdownBlock = memo(function StableMarkdownBlock({ content }: { content: string }) {
  return (
    <MarkdownView
      content={content}
      className="hb-chat-agent-markdown"
      copySpecialBlocks={false}
      lite
    />
  );
});

function StreamingMarkdownBody({
  content,
  streaming,
}: {
  content: string;
  streaming: boolean;
}) {
  const parts = useMemo(
    () => streaming ? splitStreamingMarkdown(content) : null,
    [content, streaming]
  );

  if (!streaming || !parts) {
    return (
      <MarkdownView
        content={content}
        className="hb-chat-agent-markdown"
        lite
      />
    );
  }

  return (
    <div className="relative">
      {parts.stableBlocks.map((block, index) => (
        <StableMarkdownBlock key={index} content={block} />
      ))}
      <span className="whitespace-pre-wrap break-words">{parts.tail}</span>
      <span
        className="inline-block w-1.5 h-3.5 ml-0.5 align-text-bottom bg-[var(--text-tertiary)] animate-pulse"
        aria-hidden
      />
    </div>
  );
}

/** AG-21 工具结果卡：状态头 + allowlist 视图（未知 kind → fallbackMarkdown）
 * + 截断标记 + 来源行。不渲染任何 model_text（它不在事件 payload 里） */
const ToolCardView = memo(function ToolCardView({ card }: { card: ToolCard }) {
  const view = pickArtifactView(card);
  const provenance = card.provenance ?? [];
  const statusLabel = card.status === 'running'
    ? '执行中'
    : card.status === 'completed'
      ? '已完成'
      : '未完成';
  // NEXT-042：rename 提案卡是 Chat 内唯一决策入口，改名失败原因同样必须可见——
  // 两者都不得折叠进 <details>（此前改名卡默认折叠，用户根本没看见，标题永远改不掉）。
  const alwaysOpen = view.mode === 'rename'
    || (card.name === 'rename_article' && card.status === 'failed');
  if (alwaysOpen) {
    return (
      <div className="max-w-[85%] rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] overflow-hidden">
        <div
          className="flex items-center gap-1.5 px-2.5 py-1.5 bg-[var(--bg-sunken)] text-xs text-[var(--text-tertiary)] select-none"
          title={card.name}
        >
          <Wrench size={10} className="shrink-0" />
          <span className="truncate">{toolDisplayName(card.name)}</span>
          {card.status === 'running' && <Loader2 size={10} className="animate-spin text-[var(--accent)] shrink-0" />}
          {card.status === 'completed' && <Check size={10} className="text-[var(--success)] shrink-0" />}
          {card.status === 'failed' && <X size={10} className="text-[var(--danger)] shrink-0" />}
          <span className="text-[var(--text-tertiary)]">· {statusLabel}</span>
        </div>
        <div className="px-3 py-2 border-t border-[var(--border-default)] text-xs text-[var(--text-secondary)] space-y-1.5">
          {card.status === 'running' && <p className="text-[var(--text-tertiary)]">执行中…</p>}
          {card.status === 'failed' && <p className="text-[var(--danger)]">{card.error ?? '改名提案未成立'}</p>}
          {card.status === 'completed' && <ArtifactBody view={view} card={card} />}
          {provenance.length > 0 && (
            <div className="flex flex-wrap gap-1 pt-1">
              {provenance.map((p, i) => (
                <span
                  key={i}
                  className="px-1.5 py-0.5 rounded bg-[var(--bg-sunken)] text-[var(--text-tertiary)] text-xs max-w-[180px] truncate"
                  title={`${p.source}${p.sourceId ? ` · ${p.sourceId}` : ''}`}
                >
                  来源：{p.title ?? p.sourceId ?? p.source}
                </span>
              ))}
            </div>
          )}
        </div>
      </div>
    );
  }
  return (
    <details className="group max-w-[85%] rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] overflow-hidden">
      <summary
        className="flex items-center gap-1.5 px-2.5 py-1.5 bg-[var(--bg-sunken)] text-xs text-[var(--text-tertiary)] cursor-pointer select-none list-none hover:bg-[var(--bg-sunken)] transition-colors"
        title={card.name}
      >
        <Wrench size={10} className="shrink-0" />
        <span className="truncate">{toolDisplayName(card.name)}</span>
        {card.status === 'running' && <Loader2 size={10} className="animate-spin text-[var(--accent)] shrink-0" />}
        {card.status === 'completed' && <Check size={10} className="text-[var(--success)] shrink-0" />}
        {card.status === 'failed' && <X size={10} className="text-[var(--danger)] shrink-0" />}
        <span className="text-[var(--text-tertiary)]">· {statusLabel}</span>
        {card.preresolved && (
          <span className="px-1.5 rounded bg-[var(--bg-sunken)] text-[var(--text-tertiary)] border border-[var(--border-default)] shrink-0">未执行</span>
        )}
        {card.truncated && (
          <span className="ml-auto px-1.5 rounded bg-[var(--warning-subtle)] text-[var(--warning)] border border-[var(--gold-border)] shrink-0">
            内容已截断
          </span>
        )}
      </summary>
      <div className="px-3 py-2 border-t border-[var(--border-default)] text-xs text-[var(--text-secondary)] space-y-1.5">
        {card.status === 'running' && <p className="text-[var(--text-tertiary)]">执行中…</p>}
        {card.status === 'failed' && <p className="text-[var(--danger)]">{card.error ?? '执行失败'}</p>}
        {card.status === 'completed' && <ArtifactBody view={view} card={card} />}
        {provenance.length > 0 && (
          <div className="flex flex-wrap gap-1 pt-1">
            {provenance.map((p, i) => (
              <span
                key={i}
                className="px-1.5 py-0.5 rounded bg-[var(--bg-sunken)] text-[var(--text-tertiary)] text-xs max-w-[180px] truncate"
                title={`${p.source}${p.sourceId ? ` · ${p.sourceId}` : ''}`}
              >
                来源：{p.title ?? p.sourceId ?? p.source}
              </span>
            ))}
          </div>
        )}
      </div>
    </details>
  );
}, (previous, next) => (
  previous.card.callId === next.card.callId &&
  previous.card.status === next.card.status &&
  previous.card.error === next.card.error &&
  previous.card.uiArtifact === next.card.uiArtifact &&
  previous.card.structured === next.card.structured
));

/** 按 kind 渲染 artifact 视图；无 envelope 时展示 structured 只读预览 */
function ArtifactBody({ view, card }: {
  view: ArtifactView;
  card: ToolCard;
}) {
  switch (view.mode) {
    case 'diff':
      // 审批唯一入口是原文内 ✓/×；Chat 只留审计状态，不重复决策控件。
      return <DiffResultNotice preview={view.preview} />;
    case 'rename':
      // 标题改名没有"原文"可审，Chat 卡即审批入口：
      // 「应用」走 appStore.updateArticleTitle 完整链路（SQLite + frontmatter + 双链级联 + 索引重建）。
      return <RenameApprovalCard preview={view.preview} />;
    case 'keyValue':
      return (
        <div className="space-y-0.5">
          {view.rows.map(([label, value], i) => (
            <div key={i} className="flex gap-2">
              <span className="text-[var(--text-tertiary)] shrink-0">{label}</span>
              <span className="text-[var(--text-secondary)] break-all">{formatValue(value)}</span>
            </div>
          ))}
        </div>
      );
    case 'table':
      return (
        <div className="overflow-x-auto">
          <table className="text-xs border-collapse">
            <thead>
              <tr>
                {view.columns.map((c) => (
                  <th key={c} className="text-left pr-3 py-0.5 text-[var(--text-tertiary)] font-medium whitespace-nowrap">
                    {c}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {view.rows.map((r, i) => (
                <tr key={i}>
                  {view.columns.map((_, j) => (
                    <td key={j} className="pr-3 py-0.5 text-[var(--text-secondary)] whitespace-nowrap">
                      {formatValue(r[j])}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case 'markdown':
    case 'fallback':
      // 未知 kind / payload 不合约定 → fallbackMarkdown（不空白、不执行）
      return <MarkdownView content={view.markdown} />;
    case 'none':
      // 无 envelope：展示 structured 只读预览；连 structured 也没有则状态行
      return card.structured != null ? (
        <pre className="font-mono text-[13px] text-[var(--text-secondary)] whitespace-pre-wrap break-all max-h-32 overflow-y-auto rounded-[6px] bg-[var(--bg-sunken)] px-2.5 py-2">
          {JSON.stringify(card.structured, null, 2)}
        </pre>
      ) : (
        <p className="text-[var(--text-tertiary)]">执行完成</p>
      );
  }
}

/** rename 卡：标题改名提案审批（Chat 内唯一决策入口）。 */
function RenameApprovalCard({ preview }: { preview: RenamePreviewPayload }) {
  const [phase, setPhase] = useState<'idle' | 'applying' | 'applied' | 'failed'>('idle');
  const [error, setError] = useState<string | null>(null);
  const apply = useCallback(async () => {
    setPhase('applying');
    setError(null);
    try {
      await useAppStore.getState().updateArticleTitle(preview.documentId, preview.newTitle);
      setPhase('applied');
    } catch (e) {
      setPhase('failed');
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [preview.documentId, preview.newTitle]);

  return (
    <div className="py-1 text-xs text-[var(--text-secondary)]">
      <p>
        将《{preview.oldTitle}》重命名为《{preview.newTitle}》
        {preview.wikilinkAffectedCount > 0
          ? `（另 ${preview.wikilinkAffectedCount} 篇文档的双链将同步改写）`
          : ''}
      </p>
      {phase === 'applied' ? (
        <p className="text-[var(--success)] mt-1">✓ 已应用</p>
      ) : (
        <div className="flex items-center gap-2 mt-1.5">
          <button
            type="button"
            onClick={apply}
            disabled={phase === 'applying'}
            className="px-2 py-0.5 rounded bg-[var(--accent)] hover:bg-[var(--accent-strong)] text-white text-xs disabled:opacity-50"
          >
            {phase === 'applying' ? '应用中…' : '应用改名'}
          </button>
          {phase === 'failed' && <span className="text-[var(--danger)]">{error ?? '改名失败'}</span>}
        </div>
      )}
    </div>
  );
}

/** Chat 只记录修改结果；应用/放弃始终由原文中的 ✓/× 完成。 */
function DiffResultNotice({ preview }: { preview: DiffPreviewPayload }) {
  const session = useChangeSessionStore((state) => state.sessions[preview.operationId]);
  const phase = session?.phase ?? changePhaseFromStatus(preview.status);
  const text = phase === 'applied'
    ? '已应用到原文'
    : phase === 'rejected'
      ? '已放弃修改'
      : phase === 'conflict'
        ? '修改发生冲突'
        : phase === 'undone'
          ? '修改已撤销'
          : phase === 'applying'
            ? '正在处理修改'
            : '修改建议已显示在原文中';
  // NEXT-042：同一审批携带标题改时如实标注；部分批准不改标题（后端口径），
  // 因此待批准文案明确「全部批准才改名」，避免用户误以为已经生效。
  const titleNote = (() => {
    if (!preview.proposedTitle) return '';
    if (phase === 'applied') return `，标题已改为《${preview.proposedTitle}》`;
    if (phase === 'proposed' || phase === 'applying' || phase === 'conflict') {
      return `；全部批准变更块时标题将改为《${preview.proposedTitle}》`;
    }
    return '';
  })();
  return (
    <div className="flex items-center gap-2 py-0.5 text-xs text-[var(--text-tertiary)]">
      <span className={`h-1.5 w-1.5 rounded-full shrink-0 ${
        phase === 'applied'
          ? 'bg-[var(--success)]'
          : phase === 'conflict'
            ? 'bg-[var(--danger)]'
            : phase === 'applying'
              ? 'bg-[var(--warning)] animate-pulse'
            : 'bg-[var(--border-strong)]'
      }`} />
      <span className="truncate" title={preview.title}>
        《{preview.title || '未命名文档'}》{text}{titleNote}
      </span>
    </div>
  );
}

function ContinuationChangeChip({ title }: { title: string }) {
  return (
    <div
      className="inline-flex min-w-0 items-center gap-1.5 rounded-md border border-[var(--success)] bg-[var(--success-subtle)] px-2 py-1 text-xs text-[var(--success)]"
      title="本轮会自动沿用当前变更的文档、原选区与锚点；新提案会替换旧建议"
    >
      <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--success)]" />
      <span className="shrink-0 font-medium">继续修改</span>
      <span className="truncate text-[var(--success)]">《{title || '未命名文档'}》</span>
    </div>
  );
}

/** 卡片值展示：字符串原样，其余 JSON 化（null → 占位符） */
function formatValue(v: unknown): string {
  if (v === null || v === undefined) return '—';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  return JSON.stringify(v);
}

/** composer 上方的范围 chip（AG-26 验收场景①：选中段落 → chip 出现 → 下指令）。
 * AG-31：chip 增加源码行号「(起-止)」——best-effort（未命中时只留摘录），
 * 行号仅展示，真实锚定仍走 TextAnchor hash。外层容器由 ChatComposer chip 行负责 */
function ComposerSelectionChip({ context, lines = null, onClear }: {
  context: RunContext;
  lines?: [number, number] | null;
  onClear?: () => void;
}) {
  return (
    <span className="inline-flex items-center gap-1 max-w-full px-2 py-1 rounded-md bg-[var(--accent-subtle)] border border-[var(--accent-border)] text-xs text-[var(--accent)]">
      <span className="font-medium truncate max-w-[160px]" title={context.title}>
        《{context.title || '未命名文档'}》
      </span>
      {lines && (
        <span className="shrink-0 text-[var(--accent)]" title="选区在 Markdown 源码中的行号">
          ({lines[0]}-{lines[1]} 行)
        </span>
      )}
      <span className="text-[var(--accent)] shrink-0">v{context.baseVersion}</span>
      <span className="text-[var(--accent)] truncate" title={context.selectedMarkdown}>
        · {selectionSnippet(context.selectedMarkdown, 32)}
      </span>
      {onClear && (
        <button
          onClick={onClear}
          title="移除选区范围（回到全项目对话）"
          className="ml-0.5 shrink-0 text-[var(--text-tertiary)] hover:text-[var(--accent)] transition-colors"
        >
          <X size={10} />
        </button>
      )}
    </span>
  );
}

/** 绑定的本地目录是会话级持续上下文；每轮由 Hermes 原生 folder attach 读取。 */
function ComposerWorkspaceChip({
  root,
  onClear,
}: {
  root: string;
  onClear?: () => void;
}) {
  const name = attachmentName(root);
  return (
    <span className="inline-flex items-center gap-1 max-w-full px-2 py-1 rounded-md bg-[var(--accent-subtle)] border border-[var(--accent-border)] text-xs text-[var(--accent)]">
      <FolderOpen size={10} className="shrink-0" />
      <span className="font-medium truncate max-w-[190px]" title={root}>{name}</span>
      {onClear && (
        <button
          type="button"
          onClick={onClear}
          title="移除本地项目范围"
          className="ml-0.5 shrink-0 text-[var(--text-tertiary)] hover:text-[var(--accent)] transition-colors"
        >
          <X size={10} />
        </button>
      )}
    </span>
  );
}

/** 权限是会话级控制，固定放在 Composer 底栏加号之后。 */
function ComposerPermissionControl({
  value,
  onChange,
  disabled = false,
}: {
  value: WorkspacePermissionMode;
  onChange: (mode: WorkspacePermissionMode) => void;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const options: Array<{ value: WorkspacePermissionMode; label: string }> = [
    { value: 'ask', label: '询问权限' },
    { value: 'autoEdit', label: '自动编辑' },
    { value: 'plan', label: '计划模式' },
  ];
  const active = options.find((option) => option.value === value) ?? options[0];
  const tone = value === 'autoEdit'
    ? 'text-[var(--accent)]'
    : value === 'plan'
      ? 'text-[var(--text-tertiary)]'
      : 'text-[var(--warning)]';

  return (
    <div className="relative">
      <button
        type="button"
        onClick={() => setOpen((current) => !current)}
        disabled={disabled}
        className={`inline-flex h-7 items-center gap-1.5 rounded-lg px-1.5 text-xs font-medium transition-colors hover:bg-[var(--bg-sunken)] disabled:pointer-events-none disabled:opacity-40 ${tone}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title="当前会话的权限模式"
      >
        <ShieldAlert size={13} />
        <span>{active.label}</span>
        <ChevronDown size={10} />
      </button>
      {open && (
        <>
          <button type="button" className="fixed inset-0 z-30 cursor-default" aria-label="关闭权限菜单" onClick={() => setOpen(false)} />
          <div className="absolute bottom-[calc(100%+8px)] left-0 z-40 w-44 overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] py-1.5 shadow-[var(--shadow-lg)]" role="menu">
            {options.map((option) => (
              <button
                key={option.value}
                type="button"
                onClick={() => { onChange(option.value); setOpen(false); }}
                className={`flex h-9 w-full items-center gap-2.5 px-3 text-left transition-colors hover:bg-[var(--bg-sunken)] ${value === option.value ? 'bg-[var(--accent-subtle)]' : ''}`}
                role="menuitemradio"
                aria-checked={value === option.value}
              >
                <ShieldAlert size={14} className={`shrink-0 ${option.value === 'ask' ? 'text-[var(--warning)]' : option.value === 'autoEdit' ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)]'}`} />
                <span className="min-w-0 flex-1 text-xs font-medium text-[var(--text-secondary)]">{option.label}</span>
                {value === option.value && <Check size={12} className="shrink-0 text-[var(--accent)]" />}
              </button>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

/** 当前文档由 Surface 自动绑定；正文仅在发送时读取，不进入常驻 React state。 */
function ComposerDocumentChip({ title }: { title: string }) {
  return (
    <span className="inline-flex items-center gap-1 max-w-full px-2 py-1 rounded-md bg-[var(--accent-subtle)] border border-[var(--accent-border)] text-xs text-[var(--accent)]">
      <FileText size={10} className="shrink-0" />
      <span className="font-medium truncate max-w-[190px]" title={title}>
        《{title || '未命名文档'}》
      </span>
      <span className="shrink-0 text-[var(--accent)]">· 当前文档</span>
    </span>
  );
}

/** 显式项目范围只传项目元数据和文档清单；具体正文需用户选择对应文档。 */
function ComposerProjectChip({ name, onClear }: { name: string; onClear?: () => void }) {
  return (
    <span className="inline-flex items-center gap-1 max-w-full px-2 py-1 rounded-md bg-[var(--accent-subtle)] border border-[var(--accent-border)] text-xs text-[var(--accent)]">
      <FolderOpen size={10} className="shrink-0" />
      <span className="font-medium truncate max-w-[190px]" title={name}>{name}</span>
      <span className="shrink-0 text-[var(--accent)]">· 项目</span>
      {onClear && (
        <button
          type="button"
          onClick={onClear}
          title="移除项目范围"
          className="ml-0.5 shrink-0 text-[var(--text-tertiary)] hover:text-[var(--accent)] transition-colors"
        >
          <X size={10} />
        </button>
      )}
    </span>
  );
}

/** composer 上方的技能 chip（AG-27：激活态可见 + 可清除；发送后保留由宿主控制） */
function ComposerSkillChip({ skill, onClear }: {
  skill: HermesSkillInfo;
  onClear?: () => void;
}) {
  return (
    <span className="inline-flex items-center gap-1 max-w-full px-2 py-1 rounded-md bg-[var(--skill-subtle)] border border-[var(--skill-border)] text-xs text-[var(--skill)]">
      <Sparkles size={10} className="shrink-0" />
      <span className="font-medium truncate max-w-[160px]" title={skill.description || skill.name}>
        {skill.name}
      </span>
      <span className="text-[color-mix(in_srgb,var(--skill)_70%,transparent)] shrink-0">· Hermes</span>
      {onClear && (
        <button
          onClick={onClear}
          title="取消激活技能（回到普通对话）"
          className="ml-0.5 shrink-0 text-[color-mix(in_srgb,var(--skill)_55%,transparent)] hover:text-[var(--skill)] transition-colors"
        >
          <X size={10} />
        </button>
      )}
    </span>
  );
}

function HermesComposerPalette({ items, activeIndex, onPick }: {
  items: HermesComposerItem[];
  activeIndex: number;
  onPick: (item: HermesComposerItem) => void;
}) {
  let previousGroup = '';
  return (
    <div className="absolute bottom-full left-0 right-0 mb-2 max-h-72 overflow-y-auto rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] py-1 shadow-[var(--shadow-lg)] z-30">
      {items.map((item, index) => {
        const showGroup = item.group !== previousGroup;
        previousGroup = item.group;
        return (
          <div key={`${item.kind}:${item.name}:${index}`}>
            {showGroup && (
              <p className="px-3 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-[var(--text-tertiary)]">
                {item.group}
              </p>
            )}
            <button
              type="button"
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => onPick(item)}
              className={`flex w-full items-center gap-2 px-3 py-2 text-left transition-colors ${
                index === activeIndex ? 'bg-[var(--accent-subtle)]' : 'hover:bg-[var(--bg-sunken)]'
              }`}
            >
              {item.kind === 'skill'
                ? <Sparkles size={12} className="shrink-0 text-[var(--skill)]" />
                : item.kind === 'reference'
                  ? <Link2 size={12} className="shrink-0 text-[var(--accent)]" />
                  : <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded border border-[var(--accent-border)] font-mono text-xs text-[var(--accent)]">›</span>}
              <span className="shrink-0 font-mono text-xs font-medium text-[var(--text-secondary)]">{item.name}</span>
              <span className="min-w-0 truncate text-xs text-[var(--text-tertiary)]">{item.description}</span>
            </button>
          </div>
        );
      })}
    </div>
  );
}

function CapabilitySearchEmpty({ query, label }: { query: string; label: string }) {
  return (
    <p className="rounded-lg border border-dashed border-[var(--border-default)] px-3 py-5 text-center text-xs text-[var(--text-tertiary)]">
      {query ? `未找到匹配“${query}”的 ${label}` : `Hermes Runtime 当前没有可用 ${label}`}
    </p>
  );
}

function CapabilitySwitch({ checked, disabled, onChange }: { checked: boolean; disabled?: boolean; onChange: (checked: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={(event) => { event.stopPropagation(); onChange(!checked); }}
      className={`relative h-4 w-7 shrink-0 rounded-full transition ${checked ? 'bg-[var(--accent)]' : 'bg-[var(--bg-sunken)]'} disabled:opacity-50`}
    >
      <span className={`absolute top-0.5 h-3 w-3 rounded-full bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] transition ${checked ? 'left-3.5' : 'left-0.5'}`} />
    </button>
  );
}

function CapabilityMasterRow({ active, title, subtitle, badges, toggle, onClick }: {
  active: boolean;
  title: string;
  subtitle?: ReactNode;
  badges?: ReactNode;
  toggle?: ReactNode;
  onClick: () => void;
}) {
  return (
    <button type="button" onClick={onClick} className={`flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left transition ${active ? 'bg-[var(--accent-subtle)] text-[var(--text-primary)]' : 'hover:bg-[var(--bg-sunken)] text-[var(--text-secondary)]'}`}>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-1.5"><span className="truncate text-xs font-medium">{title}</span>{badges}</div>
        {subtitle && <div className="mt-0.5 truncate text-xs text-[var(--text-tertiary)]">{subtitle}</div>}
      </div>
      {toggle ?? <ChevronRight size={12} className="mt-0.5 shrink-0 text-[var(--text-tertiary)]" />}
    </button>
  );
}

function CapabilitySearch({ value, onChange, placeholder }: { value: string; onChange: (value: string) => void; placeholder: string }) {
  return (
    <label className="flex items-center gap-2 border-b border-[var(--border-default)] px-3">
      <Search size={12} className="shrink-0 text-[var(--text-tertiary)]" />
      <input value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} className="min-w-0 flex-1 bg-transparent py-2.5 text-xs text-[var(--text-secondary)] outline-none placeholder:text-[var(--text-disabled)]" />
      {value && <button type="button" onClick={() => onChange('')} className="text-[var(--text-tertiary)] hover:text-[var(--text-tertiary)]"><X size={11} /></button>}
    </label>
  );
}

/** Runtime 同源的能力控制面：主从浏览，写操作只调用正式接口。 */
export function HermesCapabilitiesPanel({ snapshot, error, connStatus, tab, onTab, onRefresh, onReconnect, onClose, embedded = false, showTabs = true }: {
  snapshot: HermesCapabilities | null;
  error: string | null;
  connStatus: HermesConnectionStatus;
  tab: CapabilityTab;
  onTab: (tab: CapabilityTab) => void;
  onRefresh: () => void;
  onReconnect: () => void;
  onClose?: () => void;
  embedded?: boolean;
  showTabs?: boolean;
}) {
  const [busyName, setBusyName] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [selectedSkill, setSelectedSkill] = useState('');
  const [selectedToolset, setSelectedToolset] = useState('');
  const [selectedMcp, setSelectedMcp] = useState('');
  const [selectedCatalog, setSelectedCatalog] = useState('');
  const [mcpMode, setMcpMode] = useState<'servers' | 'catalog'>('servers');
  const [mcpAddOpen, setMcpAddOpen] = useState(false);
  const [mcpDraft, setMcpDraft] = useState<HermesMcpServerCreate>({ name: '', transport: 'http', url: '', command: '', args: [], env: {}, auth: 'none', bearerToken: '' });
  const [mcpArgsDraft, setMcpArgsDraft] = useState('');
  const [mcpEnvDraft, setMcpEnvDraft] = useState('');
  const [mcpProbes, setMcpProbes] = useState<Record<string, HermesMcpProbe>>({});
  const [catalog, setCatalog] = useState<HermesMcpCatalog | null>(null);
  const [catalogEnv, setCatalogEnv] = useState<Record<string, string>>({});
  const [hubQuery, setHubQuery] = useState('');
  const [hub, setHub] = useState<HermesHubPage | null>(null);
  const [hubLoading, setHubLoading] = useState(false);
  const [hubPreview, setHubPreview] = useState<HermesHubPreview | null>(null);
  const [skillEditor, setSkillEditor] = useState<{ name: string; content: string } | null>(null);
  const [skillDraft, setSkillDraft] = useState('');
  const tabs = [
    ['skills', `Skill ${snapshot?.skills.length ?? 0}`],
    ['tools', `Tools ${snapshot?.tools.length ?? 0}`],
    ['mcp', `MCP ${snapshot?.mcpServers.length ?? 0}`],
    ['hub', 'Browse Hub'],
  ] as const;
  const activeMenuItem = CAPABILITY_MENU_ITEMS.find((item) => item.key === tab) ?? CAPABILITY_MENU_ITEMS[0];
  const ActiveCapabilityIcon = activeMenuItem.icon;

  useEffect(() => { setQuery(''); }, [tab]);
  useEffect(() => {
    if (tab !== 'hub' || hub) return;
    setHubLoading(true);
    hermesSkillsHub('', 1).then(setHub).catch((reason) => setActionError(String(reason))).finally(() => setHubLoading(false));
  }, [hub, tab]);
  useEffect(() => {
    if (tab !== 'mcp' || mcpMode !== 'catalog' || catalog) return;
    hermesMcpCatalog().then(setCatalog).catch((reason) => setActionError(String(reason)));
  }, [catalog, mcpMode, tab]);

  const filteredSkills = useMemo(() => snapshot?.skills.filter((skill) => capabilityMatches(query, skill.name, skill.description, skill.category, skill.provenance, skill.origin)) ?? [], [query, snapshot]);
  const filteredToolsets = useMemo(() => snapshot?.toolsets.filter((toolset) => capabilityMatches(query, toolset.name, toolset.description, ...toolset.tools)) ?? [], [query, snapshot]);
  const filteredServers = useMemo(() => snapshot?.mcpServers.filter((server) => capabilityMatches(query, server.name, server.transport, server.url, server.command, ...server.tools.flatMap((tool) => [tool.name, tool.description]))) ?? [], [query, snapshot]);
  const filteredCatalog = useMemo(() => catalog?.entries.filter((entry) => capabilityMatches(query, entry.name, entry.description, entry.source, entry.transport, ...entry.requiredEnv.map((item) => item.name))) ?? [], [catalog, query]);
  const activeSkill = filteredSkills.find((item) => item.name === selectedSkill) ?? filteredSkills[0] ?? null;
  const activeToolset = filteredToolsets.find((item) => item.name === selectedToolset) ?? filteredToolsets[0] ?? null;
  const activeMcp = filteredServers.find((item) => item.name === selectedMcp) ?? filteredServers[0] ?? null;
  const activeCatalog = filteredCatalog.find((item) => item.name === selectedCatalog) ?? filteredCatalog[0] ?? null;

  const runAction = async (key: string, action: () => Promise<void>) => {
    setBusyName(key); setActionError(null);
    try { await action(); } catch (reason) { setActionError(reason instanceof Error ? reason.message : String(reason)); } finally { setBusyName(null); }
  };
  const parseMcpEnv = (raw: string): Record<string, string> => {
    const result: Record<string, string> = {};
    for (const sourceLine of raw.split('\n')) {
      const line = sourceLine.trim(); if (!line) continue;
      const separator = line.indexOf('=');
      if (separator <= 0) throw new Error(`环境变量格式无效：${line}`);
      const key = line.slice(0, separator).trim();
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) throw new Error(`环境变量名称无效：${key}`);
      result[key] = line.slice(separator + 1);
    }
    return result;
  };
  const saveMcp = () => runAction('mcp:add', async () => {
    const request: HermesMcpServerCreate = {
      ...mcpDraft,
      url: mcpDraft.transport === 'http' ? mcpDraft.url : '',
      command: mcpDraft.transport === 'stdio' ? mcpDraft.command : '',
      args: mcpDraft.transport === 'stdio' ? mcpArgsDraft.split('\n').map((item) => item.trim()).filter(Boolean) : [],
      env: mcpDraft.transport === 'stdio' ? parseMcpEnv(mcpEnvDraft) : {},
      auth: mcpDraft.transport === 'http' ? mcpDraft.auth : 'none',
      bearerToken: mcpDraft.transport === 'http' ? mcpDraft.bearerToken : '',
    };
    if (request.transport === 'stdio' && !(await confirmDialog('本地 stdio MCP 会由 Hermes 启动本机命令。请只添加你信任的软件包。继续吗？', { title: 'SophoNote', kind: 'warning' }))) return;
    const probe = await hermesMcpAdd(request);
    setMcpProbes((current) => ({ ...current, [request.name.trim()]: probe }));
    setMcpAddOpen(false); setMcpDraft({ name: '', transport: 'http', url: '', command: '', args: [], env: {}, auth: 'none', bearerToken: '' }); setMcpArgsDraft(''); setMcpEnvDraft(''); onRefresh();
  });
  const testMcp = (name: string) => runAction(`mcp:test:${name}`, async () => { const probe = await hermesMcpTest(name); setMcpProbes((current) => ({ ...current, [name]: probe })); });
  const authorizeMcp = (name: string) => runAction(`mcp:auth:${name}`, async () => {
    const started = await hermesMcpOAuthStart(name);
    if (started.status === 'error' || !started.authorization_url) throw new Error(started.error || 'Hermes 未返回 OAuth 授权地址');
    await openUrl(started.authorization_url);
    for (let attempt = 0; attempt < 600; attempt += 1) {
      await new Promise<void>((resolve) => window.setTimeout(resolve, 1000));
      const current = await hermesMcpOAuthStatus(started.flow_id);
      if (current.status === 'approved') { await testMcp(name); onRefresh(); return; }
      if (current.status === 'error') throw new Error(current.error || 'Hermes MCP OAuth 授权失败');
    }
    throw new Error('MCP OAuth 授权等待超时');
  });
  const manageBrowser = (action: 'connect' | 'disconnect') => runAction(`browser:${action}`, async () => { await hermesBrowserManage(action); onRefresh(); });
  const searchHub = () => { setHubLoading(true); setActionError(null); setHubPreview(null); hermesSkillsHub(hubQuery, 1).then(setHub).catch((reason) => setActionError(String(reason))).finally(() => setHubLoading(false)); };
  const openSkillEditor = (name: string) => runAction(`skill:edit:${name}`, async () => {
    const document = await hermesSkillDocument(name);
    setSkillEditor(document);
    setSkillDraft(document.content);
  });
  const archiveSkill = async (name: string) => {
    if (!(await confirmDialog(`归档 Hermes Skill「${name}」？之后可通过 Hermes curator 恢复。`, { title: 'SophoNote', kind: 'warning' }))) return;
    await runAction(`skill:archive:${name}`, async () => {
      await hermesSkillArchive(name);
      setSelectedSkill('');
      onRefresh();
    });
  };
  const removeMcp = async (name: string) => {
    if (!(await confirmDialog(`移除 MCP Server「${name}」？`, { title: 'SophoNote', kind: 'warning' }))) return;
    await runAction(`mcp:remove:${name}`, async () => {
      await hermesMcpRemove(name);
      onRefresh();
    });
  };

  const skillPane = (
    <div className="grid h-full min-h-0 grid-cols-[minmax(150px,0.9fr)_minmax(190px,1.1fr)]">
      <div className="min-h-0 overflow-y-auto border-r border-[var(--border-default)] p-2">
        <p className="px-2 pb-1 text-xs text-[var(--text-tertiary)]">按使用频率与名称浏览</p>
        {filteredSkills.map((skill) => <CapabilityMasterRow key={skill.name} active={activeSkill?.name === skill.name} title={skill.name} subtitle={skill.category || skill.origin || 'General'} badges={skill.provenance && skill.provenance !== 'bundled' ? <span className="rounded bg-[var(--accent-subtle)] px-1 py-0.5 text-xs text-[var(--accent)]">{skill.provenance === 'agent' ? 'learned' : skill.provenance}</span> : undefined} toggle={<div className="flex items-center gap-1"><span className="text-xs text-[var(--text-tertiary)]">×{skill.usage || 0}</span><CapabilitySwitch checked={skill.enabled !== false} disabled={busyName === `skill:${skill.name}`} onChange={(enabled) => void runAction(`skill:${skill.name}`, async () => { await hermesSkillSetEnabled(skill.name, enabled); onRefresh(); })} /></div>} onClick={() => setSelectedSkill(skill.name)} />)}
        {snapshot && filteredSkills.length === 0 && <CapabilitySearchEmpty query={query} label="Skill" />}
      </div>
      <div className="min-h-0 overflow-y-auto p-4">
        {activeSkill ? <><div className="flex flex-wrap items-center gap-1.5"><h3 className="text-sm font-semibold text-[var(--text-primary)]">{activeSkill.name}</h3><span className="rounded bg-[var(--bg-sunken)] px-1.5 py-0.5 text-xs text-[var(--text-tertiary)]">{activeSkill.category || 'General'}</span>{activeSkill.provenance && <span className="rounded bg-[var(--accent-subtle)] px-1.5 py-0.5 text-xs text-[var(--accent)]">{activeSkill.provenance}</span>}</div><p className="mt-2 text-xs leading-relaxed text-[var(--text-tertiary)]">{activeSkill.description || '该 Skill 未提供说明。'}</p><div className="mt-4 flex items-center gap-2 text-xs text-[var(--text-tertiary)]"><span>使用 {activeSkill.usage || 0} 次</span><span>·</span><span>{activeSkill.enabled !== false ? '已启用' : '已停用'}</span></div>{activeSkill.provenance === 'agent' && <div className="mt-4 flex items-center gap-3"><button type="button" disabled={busyName === `skill:edit:${activeSkill.name}`} onClick={() => void openSkillEditor(activeSkill.name)} className="text-xs font-medium text-[var(--text-tertiary)] hover:text-[var(--text-primary)]">编辑</button><button type="button" onClick={() => void archiveSkill(activeSkill.name)} className="text-xs font-medium text-[var(--danger)] hover:text-[var(--danger)]">归档</button></div>}</> : <CapabilitySearchEmpty query={query} label="Skill" />}
      </div>
    </div>
  );

  const toolPane = (
    <div className="grid h-full min-h-0 grid-cols-[minmax(150px,0.9fr)_minmax(190px,1.1fr)]">
      <div className="min-h-0 overflow-y-auto border-r border-[var(--border-default)] p-2">
        <p className="px-2 pb-1 text-xs text-[var(--text-tertiary)]">Toolset 与执行能力</p>
        {filteredToolsets.map((toolset) => <CapabilityMasterRow key={toolset.name} active={activeToolset?.name === toolset.name} title={toolset.name} subtitle={toolset.description} toggle={<div className="flex items-center gap-1"><span className="text-xs text-[var(--text-tertiary)]">×{toolset.usage || 0}</span><CapabilitySwitch checked={toolset.enabled} disabled={busyName === `tool:${toolset.name}`} onChange={(enabled) => void runAction(`tool:${toolset.name}`, async () => { await hermesToolsetSetEnabled(toolset.name, enabled); onRefresh(); })} /></div>} onClick={() => setSelectedToolset(toolset.name)} />)}
        {snapshot && filteredToolsets.length === 0 && <CapabilitySearchEmpty query={query} label="Toolset" />}
      </div>
      <div className="min-h-0 overflow-y-auto p-4">
        {activeToolset ? <><h3 className="text-sm font-semibold text-[var(--text-primary)]">{activeToolset.name}</h3><p className="mt-1 text-xs leading-relaxed text-[var(--text-tertiary)]">{activeToolset.description}</p>{activeToolset.tools.length > 0 && <div className="mt-3 flex flex-wrap gap-1">{activeToolset.tools.map((name) => <span key={name} className="rounded bg-[var(--bg-sunken)] px-1.5 py-1 font-mono text-xs text-[var(--text-tertiary)]">{name}</span>)}</div>}{activeToolset.name === 'terminal' && snapshot && <div className="mt-5"><div className="mb-2 flex items-center justify-between"><h4 className="text-xs font-semibold text-[var(--text-secondary)]">Execution backend</h4><button type="button" onClick={onRefresh} className="text-xs text-[var(--text-tertiary)]">刷新探测</button></div><div className="space-y-1.5">{snapshot.terminalBackends.backends.map((backend) => <button type="button" key={backend.name} disabled={busyName === `terminal:${backend.name}`} onClick={() => void runAction(`terminal:${backend.name}`, async () => { await hermesTerminalBackendSelect(backend.name); onRefresh(); })} className={`w-full rounded-lg border px-2.5 py-2 text-left ${backend.active ? 'border-[var(--accent-border)] bg-[var(--accent-subtle)]' : 'border-transparent bg-[var(--bg-sunken)] hover:border-[var(--border-default)]'}`}><span className="flex flex-wrap items-center gap-1.5"><span className="text-xs font-medium text-[var(--text-secondary)]">{backend.label}</span><span className={`rounded px-1 py-0.5 text-xs ${backend.status === 'ready' ? 'bg-[var(--success-subtle)] text-[var(--success)]' : 'bg-[var(--warning-subtle)] text-[var(--warning)]'}`}>{backend.status === 'ready' ? 'Ready' : 'Needs setup'}</span>{backend.active && <span className="rounded bg-[var(--accent-subtle)] px-1 py-0.5 text-xs text-[var(--accent)]">In use</span>}</span><span className="mt-0.5 block text-xs text-[var(--text-tertiary)]">{backend.description}</span>{backend.detail && <span className="mt-1 flex items-start gap-1 text-xs text-[var(--warning)]"><AlertTriangle size={9} className="mt-0.5 shrink-0" />{backend.detail}</span>}</button>)}</div></div>}</> : <CapabilitySearchEmpty query={query} label="Toolset" />}
      </div>
    </div>
  );

  const mcpPane = (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-3 border-b border-[var(--border-default)] px-3"><button type="button" onClick={() => setMcpMode('servers')} className={`text-xs ${mcpMode === 'servers' ? 'font-semibold text-[var(--text-secondary)]' : 'text-[var(--text-tertiary)]'}`}>Servers</button><button type="button" onClick={() => setMcpMode('catalog')} className={`text-xs ${mcpMode === 'catalog' ? 'font-semibold text-[var(--text-secondary)]' : 'text-[var(--text-tertiary)]'}`}>Catalog</button>{mcpMode === 'servers' && <button type="button" onClick={() => setMcpAddOpen((open) => !open)} className="ml-auto text-xs text-[var(--accent)]"><Plus size={10} className="mr-1 inline" />新增</button>}</div>
      {mcpAddOpen && <div className="shrink-0 border-b border-[var(--border-default)] bg-[var(--bg-sunken)] p-3 space-y-2"><input value={mcpDraft.name} onChange={(event) => setMcpDraft((draft) => ({ ...draft, name: event.target.value }))} placeholder="Server 名称" className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 text-xs outline-none" /><div className="flex gap-1">{(['http', 'stdio'] as const).map((transport) => <button type="button" key={transport} onClick={() => setMcpDraft((draft) => ({ ...draft, transport, auth: transport === 'stdio' ? 'none' : draft.auth }))} className={`rounded border px-2 py-1 text-xs ${mcpDraft.transport === transport ? 'border-[var(--accent-border)] text-[var(--accent)]' : 'border-[var(--border-default)] text-[var(--text-tertiary)]'}`}>{transport}</button>)}</div>{mcpDraft.transport === 'http' ? <><input value={mcpDraft.url} onChange={(event) => setMcpDraft((draft) => ({ ...draft, url: event.target.value }))} placeholder="https://example.com/mcp" className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 text-xs outline-none" /><select value={mcpDraft.auth} onChange={(event) => setMcpDraft((draft) => ({ ...draft, auth: event.target.value as HermesMcpServerCreate['auth'], bearerToken: '' }))} className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 text-xs"><option value="none">无需认证</option><option value="header">Bearer Token</option><option value="oauth">OAuth 2.1</option></select>{mcpDraft.auth === 'header' && <input type="password" autoComplete="off" value={mcpDraft.bearerToken} onChange={(event) => setMcpDraft((draft) => ({ ...draft, bearerToken: event.target.value }))} placeholder="Token 仅提交给 Hermes" className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 text-xs outline-none" />}</> : <><input value={mcpDraft.command} onChange={(event) => setMcpDraft((draft) => ({ ...draft, command: event.target.value }))} placeholder="命令，如 npx" className="w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 text-xs outline-none" /><textarea value={mcpArgsDraft} onChange={(event) => setMcpArgsDraft(event.target.value)} placeholder="参数，每行一个" className="min-h-12 w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 font-mono text-xs outline-none" /><textarea value={mcpEnvDraft} onChange={(event) => setMcpEnvDraft(event.target.value)} placeholder="环境变量，每行 KEY=VALUE" className="min-h-12 w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1.5 font-mono text-xs outline-none" /></>}<div className="flex justify-end gap-1"><button type="button" onClick={() => setMcpAddOpen(false)} className="px-2 py-1 text-xs text-[var(--text-tertiary)]">取消</button><button type="button" onClick={() => void saveMcp()} disabled={busyName === 'mcp:add'} className="rounded bg-[var(--accent)] px-2 py-1 text-xs text-white">保存并连接</button></div></div>}
      <div className="grid min-h-0 flex-1 grid-cols-[minmax(140px,0.85fr)_minmax(200px,1.15fr)]">
        <div className="min-h-0 overflow-y-auto border-r border-[var(--border-default)] p-2">{mcpMode === 'servers' ? filteredServers.map((server) => <CapabilityMasterRow key={server.name} active={activeMcp?.name === server.name} title={server.name} subtitle={`${server.transport} · ${server.tools.length} tools`} badges={<span className={`size-1.5 rounded-full ${server.enabled ? 'bg-[var(--success)]' : 'bg-[var(--border-strong)]'}`} />} toggle={<CapabilitySwitch checked={server.enabled} disabled={server.name === 'sophonote-bridge' || Boolean(busyName?.includes(server.name))} onChange={(enabled) => void runAction(`mcp:toggle:${server.name}`, async () => { await hermesMcpSetEnabled(server.name, enabled); onRefresh(); })} />} onClick={() => setSelectedMcp(server.name)} />) : filteredCatalog.map((entry) => <CapabilityMasterRow key={entry.name} active={activeCatalog?.name === entry.name} title={entry.name} subtitle={entry.description} badges={entry.installed ? <span className="rounded bg-[var(--success-subtle)] px-1 py-0.5 text-xs text-[var(--success)]">installed</span> : undefined} onClick={() => setSelectedCatalog(entry.name)} />)}{mcpMode === 'servers' && snapshot && filteredServers.length === 0 && <CapabilitySearchEmpty query={query} label="MCP Server" />}</div>
        <div className="min-h-0 overflow-y-auto p-4">{mcpMode === 'servers' ? activeMcp ? <><div className="flex items-center gap-2"><Server size={13} className="text-[var(--text-tertiary)]" /><h3 className="text-sm font-semibold text-[var(--text-primary)]">{activeMcp.name}</h3><span className="text-xs text-[var(--text-tertiary)]">{activeMcp.transport}</span></div><div className="mt-3 rounded-[6px] bg-[var(--bg-sunken)] p-3 font-mono text-[13px] leading-relaxed text-[var(--text-secondary)]"><p>{'{'}</p><p className="pl-3">"enabled": {String(activeMcp.enabled)},</p>{activeMcp.url && <p className="pl-3">"url": "{activeMcp.url}",</p>}{activeMcp.command && <p className="pl-3">"command": "{activeMcp.command}",</p>}{activeMcp.args.length > 0 && <p className="pl-3">"args": {JSON.stringify(activeMcp.args)},</p>}{activeMcp.auth && <p className="pl-3">"auth": "configured"</p>}<p>{'}'}</p></div><p className="mt-2 text-xs text-[var(--text-tertiary)]">安全预览已隐藏 headers、Token 与环境变量值。</p><div className="mt-3 space-y-1">{activeMcp.tools.map((tool) => <p key={tool.name} className="text-xs text-[var(--text-tertiary)]"><span className="font-mono text-[var(--text-secondary)]">{tool.name}</span>{tool.description ? ` · ${tool.description}` : ''}</p>)}</div>{mcpProbes[activeMcp.name] && <p className={`mt-3 text-xs ${mcpProbes[activeMcp.name].ok ? 'text-[var(--success)]' : 'text-[var(--danger)]'}`}>{mcpProbes[activeMcp.name].ok ? `连接成功 · ${mcpProbes[activeMcp.name].tools.length} tools · ${mcpProbes[activeMcp.name].prompts} prompts · ${mcpProbes[activeMcp.name].resources} resources` : mcpProbes[activeMcp.name].error}</p>}<div className="mt-4 flex flex-wrap gap-1"><button type="button" onClick={() => void testMcp(activeMcp.name)} className="rounded border border-[var(--border-default)] px-2 py-1 text-xs text-[var(--text-tertiary)]">测试</button>{activeMcp.auth === 'oauth' && <button type="button" onClick={() => void authorizeMcp(activeMcp.name)} className="rounded border border-[var(--gold-border)] px-2 py-1 text-xs text-[var(--warning)]">OAuth 授权</button>}{activeMcp.name !== 'sophonote-bridge' && <button type="button" onClick={() => void removeMcp(activeMcp.name)} className="ml-auto px-2 py-1 text-xs text-[var(--danger)]">移除</button>}</div></> : <CapabilitySearchEmpty query={query} label="MCP Server" /> : activeCatalog ? <><h3 className="text-sm font-semibold text-[var(--text-primary)]">{activeCatalog.name}</h3><p className="mt-1 text-xs leading-relaxed text-[var(--text-tertiary)]">{activeCatalog.description}</p><div className="mt-2 flex flex-wrap gap-1"><span className="rounded bg-[var(--bg-sunken)] px-1.5 py-0.5 text-xs text-[var(--text-tertiary)]">{activeCatalog.source}</span><span className="rounded bg-[var(--bg-sunken)] px-1.5 py-0.5 text-xs text-[var(--text-tertiary)]">{activeCatalog.transport}</span><span className="rounded bg-[var(--bg-sunken)] px-1.5 py-0.5 text-xs text-[var(--text-tertiary)]">{activeCatalog.authType}</span></div>{activeCatalog.requiredEnv.length > 0 && <div className="mt-4 space-y-2"><p className="text-xs font-medium text-[var(--text-secondary)]">安装所需环境变量</p>{activeCatalog.requiredEnv.map((item) => <label key={item.name} className="block"><span className="mb-1 block text-xs text-[var(--text-tertiary)]">{item.name}{item.required ? ' · 必填' : ''}</span><input type="password" autoComplete="off" value={catalogEnv[item.name] ?? ''} onChange={(event) => setCatalogEnv((current) => ({ ...current, [item.name]: event.target.value }))} placeholder={item.prompt || '仅提交给 Hermes'} className="w-full rounded-md border border-[var(--border-default)] px-2 py-1.5 text-xs outline-none" /></label>)}</div>}{activeCatalog.postInstall && <p className="mt-3 rounded-lg bg-[var(--warning-subtle)] px-3 py-2 text-xs text-[var(--warning)]">{activeCatalog.postInstall}</p>}<button type="button" disabled={activeCatalog.installed || busyName === `catalog:${activeCatalog.name}`} onClick={() => void runAction(`catalog:${activeCatalog.name}`, async () => { const env = Object.fromEntries(activeCatalog.requiredEnv.map((item) => [item.name, catalogEnv[item.name] ?? '']).filter(([, value]) => value)); await hermesMcpCatalogInstall(activeCatalog.name, env); setCatalog(null); onRefresh(); })} className="mt-4 rounded bg-[var(--accent)] px-3 py-1.5 text-xs text-white disabled:opacity-40">{activeCatalog.installed ? '已安装' : '安装到 Hermes'}</button></> : <CapabilitySearchEmpty query={query} label="MCP Catalog" />}</div>
      </div>
    </div>
  );

  const hubPane = (
    <div className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 px-4 pt-3 pb-2"><span className="text-xs text-[var(--text-tertiary)]">Connected hubs:</span><div className="mt-1.5 flex flex-wrap gap-1">{snapshot?.hubSources.sources.map((source) => <span key={source.id} className={`rounded px-1.5 py-0.5 text-xs ${(source.available === false || source.rateLimited) ? 'bg-[var(--warning-subtle)] text-[var(--warning)]' : 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]'}`}>{source.label}</span>)}</div></div>
      <div className="shrink-0 border-y border-[var(--border-default)] px-3"><label className="flex items-center gap-2"><Search size={12} className="text-[var(--text-tertiary)]" /><input value={hubQuery} onChange={(event) => setHubQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') searchHub(); }} placeholder="搜索 Skills Hub" className="min-w-0 flex-1 py-2.5 text-xs outline-none" />{hubQuery && <button type="button" onClick={() => setHubQuery('')} className="text-[var(--text-tertiary)]"><X size={11} /></button>}</label></div>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2">{hubLoading ? <p className="py-4 text-center text-xs text-[var(--text-tertiary)]">正在检索连接的 Hub…</p> : hub?.items.length ? <><p className="pb-1 text-xs text-[var(--text-tertiary)]">{hub.total} results</p>{hub.items.map((skill) => <div key={skill.identifier || skill.name} className="flex items-start gap-2 border-b border-[var(--border-default)] py-2"><div className="min-w-0 flex-1"><div className="flex items-center gap-1"><span className="text-xs font-medium text-[var(--text-secondary)]">{skill.name}</span><span className="rounded bg-[var(--warning-subtle)] px-1 py-0.5 text-xs text-[var(--warning)]">{skill.trust || skill.source || 'community'}</span></div><p className="mt-0.5 line-clamp-2 text-xs leading-relaxed text-[var(--text-tertiary)]">{skill.description}</p></div><button type="button" onClick={() => void runAction(`preview:${skill.identifier || skill.name}`, async () => setHubPreview(await hermesSkillHubPreview(skill.identifier || skill.name)))} className="shrink-0 px-1 py-1 text-xs text-[var(--text-tertiary)]">Preview</button><button type="button" disabled={busyName === `install:${skill.identifier || skill.name}`} onClick={() => void runAction(`install:${skill.identifier || skill.name}`, async () => { await hermesSkillInstall(skill.identifier || skill.name); onRefresh(); })} className="shrink-0 px-1 py-1 text-xs font-medium text-[var(--text-secondary)]">Install</button></div>)}</> : <p className="py-8 text-center text-xs text-[var(--text-tertiary)]">输入关键词检索连接的 Skills Hub。</p>}</div>
      {hubPreview && <div className="absolute inset-6 z-30 flex min-h-0 flex-col rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]"><header className="flex items-start gap-2 border-b border-[var(--border-default)] p-3"><div className="min-w-0 flex-1"><h3 className="truncate text-sm font-semibold text-[var(--text-primary)]">{hubPreview.name}</h3><p className="truncate text-xs text-[var(--text-tertiary)]">{hubPreview.identifier}</p></div><button type="button" onClick={() => setHubPreview(null)} className="text-[var(--text-tertiary)]"><X size={13} /></button></header><div className="min-h-0 flex-1 overflow-y-auto p-4"><p className="text-xs text-[var(--text-tertiary)]">{hubPreview.description}</p><pre className="mt-3 whitespace-pre-wrap rounded-[6px] bg-[var(--bg-sunken)] p-3 font-mono text-[13px] leading-relaxed text-[var(--text-secondary)]">{hubPreview.skillMd}</pre></div></div>}
    </div>
  );

  return (
    <div className={embedded
      ? 'relative flex min-h-[680px] flex-col overflow-hidden rounded-2xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]'
      : 'absolute inset-0 z-20 flex flex-col bg-[var(--bg-surface)]'}>
      <header className="flex h-10 shrink-0 items-center gap-1.5 border-b border-[var(--border-default)] px-3"><ActiveCapabilityIcon size={13} className="text-[var(--accent)]" /><span className="text-xs font-semibold text-[var(--text-primary)]">{activeMenuItem.label}</span>
        {connStatus === 'connected' && <span className="ml-1 flex items-center gap-1 text-xs text-[var(--success)]"><span className="size-1.5 rounded-full bg-[var(--success)]" />已连接</span>}
        {connStatus === 'disconnected' && <span className="ml-1 flex items-center gap-1 text-xs text-[var(--danger)]"><span className="size-1.5 rounded-full bg-[var(--danger)]" />未连接</span>}
        {connStatus === 'restarting' && <span className="ml-1 flex items-center gap-1 text-xs text-[var(--warning)]"><span className="size-1.5 animate-pulse rounded-full bg-[var(--warning)]" />重连中…</span>}
        <button type="button" onClick={onRefresh} className="ml-auto px-2 py-1 text-xs text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]">刷新</button>
        {connStatus === 'disconnected' && <button type="button" onClick={onReconnect} className="px-2 py-1 text-xs font-medium text-[var(--accent)] hover:text-[var(--accent-strong)]">重连</button>}
        {onClose && <button type="button" onClick={onClose} title="关闭" className="flex size-6 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]"><X size={13} /></button>}
      </header>
      {showTabs && <nav className="flex h-9 shrink-0 items-center gap-1 overflow-x-auto border-b border-[var(--border-default)] px-2">{tabs.map(([key, label]) => <button type="button" key={key} onClick={() => onTab(key)} className={`shrink-0 rounded-md px-2 py-1 text-xs ${tab === key ? 'bg-[var(--bg-sunken)] font-medium text-[var(--text-primary)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'}`}>{label}</button>)}</nav>}
      {tab !== 'hub' && <CapabilitySearch value={query} onChange={setQuery} placeholder={`检索 ${tab === 'skills' ? 'Skills' : tab === 'tools' ? 'Tools' : mcpMode === 'servers' ? 'MCP Servers' : 'MCP Catalog'}`} />}
      {(error || actionError) && <div className="mx-3 mt-2 shrink-0 rounded-lg border border-[var(--danger)] bg-[var(--danger-subtle)] px-3 py-2 text-xs text-[var(--danger)] break-all">{actionError ?? error}{connStatus === 'disconnected' && <button type="button" onClick={onReconnect} className="ml-2 rounded border border-[var(--danger)] px-1.5 py-0.5 text-xs font-medium text-[var(--danger)] hover:bg-[var(--danger-subtle)]">重连 Hermes</button>}</div>}
      {connStatus === 'restarting' && !error && <div className="mx-3 mt-2 shrink-0 rounded-lg border border-[var(--gold-border)] bg-[var(--warning-subtle)] px-3 py-2 text-xs text-[var(--warning)]">正在重启 Hermes Runtime，请稍候…</div>}
      <div className="relative min-h-0 flex-1">{!snapshot && !error ? <p className="p-4 text-xs text-[var(--text-tertiary)]">正在读取 Hermes Runtime…</p> : connStatus === 'restarting' && !snapshot ? <p className="p-4 text-xs text-[var(--warning)]">Hermes 正在重连，能力面板将在恢复后自动刷新…</p> : tab === 'skills' ? skillPane : tab === 'tools' ? toolPane : tab === 'mcp' ? mcpPane : hubPane}</div>
      {skillEditor && <div className="absolute inset-4 z-40 flex min-h-0 flex-col rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]"><header className="flex items-center gap-2 border-b border-[var(--border-default)] px-3 py-2"><div className="min-w-0 flex-1"><h3 className="truncate text-xs font-medium text-[var(--text-secondary)]">{skillEditor.name}/SKILL.md</h3><p className="text-xs text-[var(--text-tertiary)]">保存将由 Hermes 校验并写入原 Skill</p></div><button type="button" disabled={busyName === `skill:save:${skillEditor.name}`} onClick={() => void runAction(`skill:save:${skillEditor.name}`, async () => { await hermesSkillDocumentSave(skillEditor.name, skillDraft); setSkillEditor(null); onRefresh(); })} className="rounded bg-[var(--accent)] px-2.5 py-1 text-xs text-white disabled:opacity-40">保存</button><button type="button" onClick={() => setSkillEditor(null)} className="text-[var(--text-tertiary)]"><X size={13} /></button></header><textarea value={skillDraft} onChange={(event) => setSkillDraft(event.target.value)} spellCheck={false} className="min-h-0 flex-1 resize-none bg-[var(--bg-sunken)] p-4 font-mono text-[13px] leading-relaxed text-[var(--text-primary)] outline-none" /></div>}
      {snapshot && <footer className="flex shrink-0 items-center gap-2 border-t border-[var(--border-default)] px-3 py-2 text-xs text-[var(--text-tertiary)]"><span className="min-w-0 flex-1 truncate">Browser {snapshot.browserConnected ? `已连接 · ${snapshot.browserUrl}` : '未连接'} · 状态来自当前 Hermes Runtime</span><button type="button" disabled={Boolean(busyName?.startsWith('browser:'))} onClick={() => void manageBrowser(snapshot.browserConnected ? 'disconnect' : 'connect')} className="shrink-0 rounded border border-[var(--border-default)] px-2 py-1 text-xs text-[var(--text-tertiary)]">{snapshot.browserConnected ? '断开' : '连接'}</button></footer>}
    </div>
  );
}

function AttachmentKindIcon({ kind }: { kind: AgentAttachmentKind }) {
  if (kind === 'image') return <Image size={11} />;
  if (kind === 'folder') return <FolderOpen size={11} />;
  if (kind === 'url') return <Link2 size={11} />;
  return <FileText size={11} />;
}

function ComposerAttachmentChip({
  attachment,
  onClear,
}: {
  attachment: AgentAttachmentInput;
  onClear: () => void;
}) {
  return (
    <span
      className="inline-flex max-w-[190px] items-center gap-1 rounded-md border border-[var(--accent-border)] bg-[var(--accent-subtle)] px-2 py-1 text-xs text-[var(--accent)]"
      title={attachment.path ?? attachment.url ?? attachment.name}
    >
      <AttachmentKindIcon kind={attachment.kind} />
      <span className="truncate">{attachment.name}</span>
      <button type="button" onClick={onClear} className="ml-0.5 text-[var(--accent)] hover:text-[var(--accent)]" title="移除附件">
        <X size={10} />
      </button>
    </span>
  );
}

function AttachmentPickerPopup({
  onPickFile,
  onPickFolder,
  onPickImage,
  onPasteImage,
  onPickUrl,
  skills,
  activeSkill,
  onPickSkill,
  onClose,
}: {
  onPickFile: () => void;
  onPickFolder: () => void;
  onPickImage: () => void;
  onPasteImage: () => void;
  onPickUrl: () => void;
  skills: HermesSkillInfo[];
  activeSkill: string | null;
  onPickSkill: (name: string | null) => void;
  onClose: () => void;
}) {
  const [view, setView] = useState<'add' | 'skills'>('add');
  const items = [
    { label: '文件…', icon: FileText, action: onPickFile },
    { label: '文件夹…', icon: FolderOpen, action: onPickFolder },
    { label: '图片…', icon: Image, action: onPickImage },
    { label: '粘贴图片', icon: Clipboard, action: onPasteImage },
    { label: 'URL…', icon: Link2, action: onPickUrl },
  ];
  return (
    <>
      <div className="fixed inset-0 z-30" onClick={onClose} />
      <div className="absolute bottom-[calc(100%+8px)] left-0 z-40 w-72 overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] py-1.5 shadow-[var(--shadow-lg)]">
        {view === 'add' ? (
          <>
            <p className="px-3 pb-1 pt-0.5 text-xs font-semibold uppercase tracking-[0.14em] text-[var(--text-tertiary)]">添加到本轮</p>
            {items.map(({ label, icon: Icon, action }) => (
              <button
                key={label}
                type="button"
                onClick={action}
                className="flex w-full items-center gap-2.5 px-3 py-2 text-left text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] hover:text-[var(--text-primary)]"
              >
                <Icon size={14} className="text-[var(--text-tertiary)]" />
                <span>{label}</span>
              </button>
            ))}
            <div className="mx-2 my-1 border-t border-[var(--border-default)]" />
            <button
              type="button"
              onClick={() => setView('skills')}
              className="flex w-full items-center gap-2.5 px-3 py-2 text-left text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] hover:text-[var(--text-primary)]"
            >
              <Sparkles size={14} className="text-[var(--skill)]" />
              <span className="font-medium">技能</span>
              <span className="ml-auto max-w-32 truncate text-[var(--text-tertiary)]">{activeSkill ?? `${skills.length} 个可用`}</span>
              <ChevronRight size={12} className="text-[var(--text-tertiary)]" />
            </button>
            <div className="mx-2 mt-1 border-t border-[var(--border-default)] px-1 pt-2 text-xs leading-relaxed text-[var(--text-tertiary)]">
              也可以在输入框直接按 ⌘V 粘贴图片
            </div>
          </>
        ) : (
          <>
            <div className="flex items-center gap-1 border-b border-[var(--border-default)] px-2 pb-1.5">
              <button type="button" onClick={() => setView('add')} className="flex h-7 w-7 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="返回添加菜单"><ChevronLeft size={14} /></button>
              <span className="text-xs font-medium text-[var(--text-secondary)]">选择技能</span>
            </div>
            <div className="max-h-64 overflow-y-auto py-1">
              <button
                type="button"
                onClick={() => { onPickSkill(null); onClose(); }}
                className={`flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition-colors hover:bg-[var(--bg-sunken)] ${activeSkill == null ? 'bg-[var(--accent-subtle)]' : ''}`}
              >
                <MessageSquareText size={13} className="text-[var(--text-tertiary)]" />
                <span className="font-medium text-[var(--text-secondary)]">普通对话</span>
                <span className="text-[var(--text-tertiary)]">不使用技能</span>
                {activeSkill == null && <Check size={11} className="ml-auto text-[var(--accent)]" />}
              </button>
              {skills.length === 0 ? (
                <p className="px-3 py-3 text-xs leading-relaxed text-[var(--text-tertiary)]">Hermes Runtime 当前没有可用技能。</p>
              ) : skills.map((skill) => (
                <button
                  key={skill.name}
                  type="button"
                  onClick={() => { onPickSkill(skill.name); onClose(); }}
                  className={`flex w-full items-center gap-2 px-3 py-2 text-left text-xs transition-colors hover:bg-[var(--bg-sunken)] ${activeSkill === skill.name ? 'bg-[var(--skill-subtle)]' : ''}`}
                  title={skill.description}
                >
                  <Sparkles size={13} className="shrink-0 text-[var(--skill)]" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium text-[var(--text-secondary)]">{skill.name}</span>
                    {skill.description && <span className="mt-0.5 block truncate text-[var(--text-tertiary)]">{skill.description}</span>}
                  </span>
                  {activeSkill === skill.name && <Check size={11} className="shrink-0 text-[var(--skill)]" />}
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </>
  );
}

function UrlAttachmentEditor({
  value,
  onChange,
  onSubmit,
  onClose,
}: {
  value: string;
  onChange: (value: string) => void;
  onSubmit: () => void;
  onClose: () => void;
}) {
  return (
    <>
      <div className="fixed inset-0 z-30" onClick={onClose} />
      <div className="absolute bottom-[calc(100%+8px)] left-0 z-40 w-80 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-3 shadow-[var(--shadow-lg)]">
        <label className="mb-2 block text-[13px] font-medium text-[var(--text-secondary)]">添加 URL</label>
        <div className="flex gap-2">
          <input
            autoFocus
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                onSubmit();
              }
            }}
            placeholder="https://example.com"
            className="min-w-0 flex-1 rounded-lg border border-[var(--border-strong)] px-2.5 py-1.5 text-xs text-[var(--text-primary)] placeholder:text-[var(--text-disabled)] outline-none transition-all focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
          />
          <button type="button" onClick={onSubmit} className="rounded-lg bg-[var(--accent)] px-3 text-xs text-white hover:bg-[var(--accent-strong)]">添加</button>
        </div>
      </div>
    </>
  );
}

function HermesModelPicker({
  providers,
  activeProvider,
  activeModel,
  error,
  onPick,
  onRetry,
  onClose,
}: {
  providers: tauri.HermesModelProvider[];
  activeProvider: string;
  activeModel: string;
  error: string | null;
  onPick: (provider: string, model: string) => void;
  onRetry: () => void;
  onClose: () => void;
}) {
  const available = providers.filter(
    (provider) => provider.authenticated === true && provider.models.length > 0
  );
  return (
    <>
      <div className="fixed inset-0 z-30" onClick={onClose} />
      <div className="absolute bottom-[calc(100%+8px)] right-0 z-40 max-h-64 w-64 overflow-y-auto rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] py-1.5 shadow-[var(--shadow-lg)]">
        <div className="border-b border-[var(--border-default)] px-3 pb-2 pt-1">
          <p className="text-xs font-semibold uppercase tracking-[0.14em] text-[var(--text-tertiary)]">本轮模型</p>
          <p className="mt-0.5 truncate text-xs text-[var(--text-tertiary)]">来自 Hermes Runtime</p>
        </div>
        {available.map((provider) => (
          <div key={provider.slug} className="border-b border-[var(--border-default)] last:border-b-0">
            <p className="px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wide text-[var(--text-tertiary)]">
              {provider.name}
            </p>
            {provider.models.map((model) => (
              <button
                key={`${provider.slug}:${model}`}
                type="button"
                onClick={() => onPick(provider.slug, model)}
                className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left hover:bg-[var(--bg-sunken)]"
              >
                <span className="min-w-0 truncate text-xs text-[var(--text-secondary)]">{model}</span>
                {activeProvider === provider.slug && activeModel === model && (
                  <Check size={12} className="shrink-0 text-[var(--accent)]" />
                )}
              </button>
            ))}
          </div>
        ))}
        {available.length === 0 && (
          <div className="px-3 py-3 text-xs leading-relaxed text-[var(--text-tertiary)]">
            <p className={error ? 'text-[var(--danger)]' : undefined}>
              {error ?? 'Hermes Runtime 没有已认证的可选模型，请先运行 `hermes model` 完成配置。'}
            </p>
            <button type="button" onClick={onRetry} className="mt-2 rounded-md border border-[var(--border-default)] px-2 py-1 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]">重新读取</button>
          </div>
        )}
      </div>
    </>
  );
}

function ContextOccupancyBar({
  percent,
  used,
  max,
}: {
  percent: number;
  used: number | null;
  max: number | null;
}) {
  const title = used != null && max != null
    ? `上下文 ${used.toLocaleString()} / ${max.toLocaleString()}（${percent}%）`
    : `上下文占用 ${percent}%`;
  return (
    <span title={title} className="inline-flex h-7 items-center gap-1 rounded-lg px-1.5 text-[10px] tabular-nums text-[var(--text-tertiary)]">
      <span className="h-1 w-10 overflow-hidden rounded-full bg-[var(--bg-sunken)]">
        <span className="block h-full bg-[var(--accent)]" style={{ width: `${Math.max(0, Math.min(100, percent))}%` }} />
      </span>
      {percent}%
    </span>
  );
}

function ConversationFindBar({
  query,
  matchCount,
  activeIndex,
  onQueryChange,
  onNext,
  onPrev,
  onClose,
}: {
  query: string;
  matchCount: number;
  activeIndex: number;
  onQueryChange: (value: string) => void;
  onNext: () => void;
  onPrev: () => void;
  onClose: () => void;
}) {
  return (
    <div className="absolute top-2 right-2 z-20 flex items-center gap-1 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] px-2 py-1 shadow-[var(--shadow-md)]">
      <Search size={12} className="text-[var(--text-tertiary)]" />
      <input
        autoFocus
        value={query}
        onChange={(event) => onQueryChange(event.target.value)}
        placeholder="查找会话"
        className="h-7 w-40 bg-transparent text-xs text-[var(--text-secondary)] outline-none placeholder:text-[var(--text-disabled)]"
      />
      <span className="min-w-[3.5rem] text-center text-[10px] tabular-nums text-[var(--text-tertiary)]">
        {query.trim() ? `${matchCount === 0 ? 0 : activeIndex + 1}/${matchCount}` : ''}
      </span>
      <button type="button" onClick={onPrev} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="上一个">
        <ChevronLeft size={12} />
      </button>
      <button type="button" onClick={onNext} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="下一个">
        <ChevronRight size={12} />
      </button>
      <button type="button" onClick={onClose} className="flex h-6 w-6 items-center justify-center rounded text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="关闭查找">
        <X size={12} />
      </button>
    </div>
  );
}

/** 对话输入框：Enter 发送、Shift+Enter 换行；running 时发送键变停止键（AG-18）。
 * AG-31：移除 focus-capture（隐式捕获改为编辑器浮动条 / ⌘L 显式 Add to Chat） */
function ChatComposer({
  placeholder,
  onSend,
  commands,
  skills,
  references,
  onPickSkill,
  onPickReference,
  onCommand,
  canSubmit,
  running,
  locked,
  lockLabel,
  onStop,
  selectionChip,
  skillChip,
  attachmentChips,
  error,
  onPasteImage,
  leftSlot,
  rightSlot,
  prefill = null,
  composerKey = 'draft',
}: {
  placeholder: string;
  onSend: (text: string) => Promise<boolean>;
  commands?: HermesCommandInfo[];
  skills?: HermesSkillInfo[];
  references?: HermesReferenceInfo[];
  onPickSkill?: (name: string | null) => void;
  onPickReference?: (reference: string) => string;
  onCommand?: (command: string) => boolean;
  /** 附件可形成无文本 Run。 */
  canSubmit?: boolean;
  running?: boolean;
  /** 历史恢复或当前轮非终态时禁止编辑/发送；状态层另有同口径硬门禁。 */
  locked?: boolean;
  lockLabel?: string;
  onStop?: () => void;
  /** AG-26：范围 chip 插槽（textarea 上方） */
  selectionChip?: ReactNode;
  /** AG-27：技能 chip 插槽（与范围 chip 同行） */
  skillChip?: ReactNode;
  attachmentChips?: ReactNode;
  error?: string | null;
  onPasteImage?: (file: File) => void;
  /** AG-27：底部左侧扩展槽（技能选择按钮） */
  leftSlot?: ReactNode;
  rightSlot?: ReactNode;
  prefill?: { nonce: number; text: string } | null;
  composerKey?: string;
}) {
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [caret, setCaret] = useState(0);
  const [activeItem, setActiveItem] = useState(0);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const historyRef = useRef<string[]>([]);
  const draftRef = useRef('');
  const trigger = useMemo(() => detectComposerTrigger(text, caret), [text, caret]);
  const triggerItems = useMemo(
    () => composerItems(trigger, commands ?? [], skills ?? [], references ?? []),
    [commands, references, skills, trigger],
  );
  useEffect(() => { setActiveItem(0); }, [trigger?.kind, trigger?.query]);
  useEffect(() => {
    historyRef.current = [];
    draftRef.current = '';
    setHistoryIndex(-1);
    setText('');
    setCaret(0);
  }, [composerKey]);
  useEffect(() => {
    if (!prefill) return;
    setText(prefill.text);
    setCaret(prefill.text.length);
    setHistoryIndex(-1);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(prefill.text.length, prefill.text.length);
    });
  }, [prefill]);

  const selectComposerItem = (item: HermesComposerItem) => {
    if (!trigger) return;
    let replacement = `${item.name} `;
    if (item.kind === 'skill') {
      onPickSkill?.(item.name.replace(/^\//, ''));
      replacement = '';
    } else if (item.kind === 'reference') {
      replacement = onPickReference?.(item.name) ?? `${item.name} `;
    }
    const next = replaceComposerTrigger(text, trigger, replacement);
    const nextCaret = trigger.start + replacement.length;
    setText(next);
    setCaret(nextCaret);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(nextCaret, nextCaret);
    });
  };

  const submit = async () => {
    const t = text.trim();
    if ((!t && !canSubmit) || sending || running || locked) return;
    if (t.startsWith('/') && onCommand?.(t)) {
      setText('');
      setCaret(0);
      setHistoryIndex(-1);
      return;
    }
    setHistoryIndex(-1);
    draftRef.current = '';
    // 提交即消费草稿，不等待启动 Run/刷新 Thread 列表。否则事件流中的用户
    // 消息已经出现，textarea 仍会保留同一段文字，造成“似乎没有发送”的错觉。
    setText('');
    setSending(true);
    try {
      const sent = await onSend(t);
      if (sent) {
        historyRef.current = rememberComposerHistory(historyRef.current, t);
      } else {
        // 启动失败时恢复原草稿；若用户已开始输入下一条，保留两段内容。
        setText((current) => current.trim() ? `${t}\n${current}` : t);
      }
    } finally {
      setSending(false);
    }
  };

  return (
    <div className="relative rounded-2xl border border-[var(--border-strong)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)] focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_var(--accent-subtle)] transition-all">
      {trigger && triggerItems.length > 0 && !locked && (
        <HermesComposerPalette
          items={triggerItems}
          activeIndex={activeItem}
          onPick={selectComposerItem}
        />
      )}
      {(selectionChip || skillChip || attachmentChips) && (
        <div className="px-3 pt-2.5 flex flex-wrap gap-1.5 items-center">
          {selectionChip}
          {skillChip}
          {attachmentChips}
        </div>
      )}
      <textarea
        ref={textareaRef}
        value={text}
        disabled={locked}
        onChange={(e) => {
          setText(e.target.value);
          setCaret(e.target.selectionStart);
        }}
        onClick={(e) => setCaret(e.currentTarget.selectionStart)}
        onKeyUp={(e) => setCaret(e.currentTarget.selectionStart)}
        onPaste={(event) => {
          const images = Array.from(event.clipboardData.files).filter((file) => file.type.startsWith('image/'));
          if (images.length === 0) return;
          event.preventDefault();
          images.forEach((file) => onPasteImage?.(file));
        }}
        onKeyDown={(e) => {
          if (triggerItems.length > 0) {
            if (e.key === 'ArrowDown') {
              e.preventDefault();
              setActiveItem((current) => (current + 1) % triggerItems.length);
              return;
            }
            if (e.key === 'ArrowUp') {
              e.preventDefault();
              setActiveItem((current) => (current - 1 + triggerItems.length) % triggerItems.length);
              return;
            }
            if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
              e.preventDefault();
              selectComposerItem(triggerItems[activeItem] ?? triggerItems[0]);
              return;
            }
          }
          if (canUseComposerHistory(text, caret, triggerItems.length > 0)) {
            if (e.key === 'ArrowUp' && !e.shiftKey) {
              e.preventDefault();
              if (historyIndex < 0) draftRef.current = text;
              const next = composerHistoryStep('up', historyRef.current, historyIndex, draftRef.current);
              setHistoryIndex(next.index);
              setText(next.text);
              setCaret(next.text.length);
              return;
            }
            if (e.key === 'ArrowDown' && !e.shiftKey) {
              e.preventDefault();
              const next = composerHistoryStep('down', historyRef.current, historyIndex, draftRef.current);
              setHistoryIndex(next.index);
              setText(next.text);
              setCaret(next.text.length);
              return;
            }
          }
          if (e.key === 'Enter' && !e.shiftKey && !running && !locked) { e.preventDefault(); void submit(); }
        }}
        placeholder={placeholder}
        rows={1}
        className="w-full resize-none bg-transparent px-4 pt-3.5 text-sm text-[var(--text-secondary)] placeholder:text-[var(--text-disabled)] focus:outline-none disabled:text-[var(--text-disabled)] disabled:cursor-not-allowed"
      />
      {error && <p className="px-4 pb-1 text-xs text-[var(--danger)]">{error}</p>}
      <div className="flex items-center justify-between px-3 pb-2.5">
        <div className="flex items-center gap-1">
          {(sending || locked) && <Loader2 size={12} className="text-[var(--accent)] animate-spin" />}
          {locked && lockLabel && (
            <span className="text-xs text-[var(--text-tertiary)]">{lockLabel}</span>
          )}
          {leftSlot}
        </div>
        <div className="flex items-center gap-1">
          {rightSlot}
          {running ? (
            <button
              onClick={onStop}
              title="停止运行"
              className="w-7 h-7 rounded-lg border border-[var(--danger)] text-[var(--danger)] flex items-center justify-center hover:bg-[var(--danger-subtle)] transition-colors"
            >
              <Square size={12} />
            </button>
          ) : (
            <button
              onClick={() => void submit()}
              disabled={(!text.trim() && !canSubmit) || sending || locked}
              title="发送"
              className="w-7 h-7 rounded-lg bg-[var(--accent)] text-white flex items-center justify-center hover:bg-[var(--accent-strong)] transition-colors disabled:opacity-40 disabled:hover:bg-[var(--accent)]"
            >
              {sending ? <Send size={14} /> : <ArrowUp size={14} />}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
