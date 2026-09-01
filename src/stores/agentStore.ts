// ============================================================
// Track B · 智能体演进（AG-15 · Phase 2 Chat · agentStore slice）
// 实施基线：docs/architecture.md 事件协议 + §六 RunStore
//
// 职责：
// - 管理 threads / runs / events 状态
// - 按 AgentEvent payload type 归约状态（reducer 模式）
// - 与后端 agent_run_start / agent_run_events_replay 命令交互
//
// 数据来源 = SQLite（Rust agent 模块），不 persist 到 localStorage。
// §3.9 规则⑤：新 slice 独立文件，零改动 appStore.ts。
// ============================================================
import { create } from 'zustand';
import { invoke, Channel } from '@tauri-apps/api/core';
import remend from 'remend';
import {
  isToolEvent,
  reduceToolCards,
  isToolCardTerminalEvent,
  type ProvenanceRef,
  type ToolCard,
  type UiArtifact,
} from '../services/agentToolCards';
import {
  deriveContentStatus,
  deriveThinkingStatus,
  reduceAssistantPhase,
  type AssistantPhase,
  type ContentStatus,
  type ThinkingStatus,
} from '../services/agentMessagePhase';
import {
  streamFlushDelay,
} from '../services/agentStreamBatching';
import { recordAgentStoreBatch } from '../services/agentStreamPerf';
import type { WorkspacePermissionMode } from '../services/workspaceBinding';

// ------------------- 类型定义（与 Rust 侧 AgentEvent 对齐）-------------------

/** 事件协议版本（与 Rust AGENT_EVENT_SCHEMA_VERSION 一致；Gateway Surface = 4） */
export const AGENT_EVENT_SCHEMA_VERSION = 4;
/** 可归约的最低 schema（含历史 v1 事件重放） */
export const AGENT_EVENT_SCHEMA_MIN = 1;

/** Thread 状态 */
export type ThreadStatus = 'running' | 'completed' | 'cancelled' | 'failed';

/** Run 状态 */
export type RunStatus =
  | 'queued'
  | 'running'
  | 'waiting_approval'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'interrupted';

/** Thread（话题容器） */
export interface AgentThread {
  id: string;
  title: string;
  status: ThreadStatus;
  projectId: string | null;
  latestRunId: string | null;
  createdAt: number;
  updatedAt: number;
  /** 关闭进历史；null = 活跃 tab */
  closedAt?: number | null;
  /** 归档后不可见；逾 TTL 硬删 */
  archivedAt?: number | null;
  /** 置顶时间；null/缺省 = 未置顶（侧栏「置顶」段优先展示） */
  pinnedAt?: number | null;
  /** 收藏夹分类 ID；null/缺省 = 未收藏（一个会话至多归一个分类） */
  collectionId?: string | null;
}

/** 收藏夹分类（后端 thread_collections 表，camelCase） */
export interface ThreadCollection {
  id: string;
  name: string;
  createdAt: number;
}

export type ThreadListScope = 'active' | 'history';

/** 归档会话可选 TTL 设置键（天）；0 / 缺省 = 永久保留，不自动清理 */
export const THREAD_HISTORY_TTL_DAYS_KEY = 'agent.thread_history_ttl_days';
/** 0 = 永久；不对普通/历史会话做定时删除 */
export const DEFAULT_THREAD_HISTORY_TTL_DAYS = 0;

/** 可见的当前文档范围。markdown 是发送时编辑器草稿，只用于 Hermes 原生
 * 只读附件；有显式选区时调用方不应同时上传全文。 */
export interface FocusDocumentInput {
  articleId: string;
  title: string;
  baseVersion: number;
  markdown: string;
}

/**
 * 用某个项目最新拉取的 Thread 替换该项目缓存，同时保留其它项目的缓存。
 * ProjectChatPanel 在项目间快速切换时会并发触发 loadThreads；若直接覆盖全局
 * threads，较晚返回的旧项目请求会污染当前项目的会话窗口。
 */
export function mergeThreadsForProject(
  existing: AgentThread[],
  incoming: AgentThread[],
  projectId?: string
): AgentThread[] {
  const scope = projectId ?? null;
  return [
    ...existing.filter((thread) => thread.projectId !== scope),
    ...incoming.filter((thread) => thread.projectId === scope),
  ];
}

/**
 * 只在目标项目范围内解析首选 Thread。即使 React 项目切换后的首帧还持有旧
 * activeThreadId，也绝不能把旧项目会话带进新项目 Run。
 */
export function resolveProjectThreadId(
  threads: AgentThread[],
  projectId: string | null,
  preferredThreadId?: string | null
): string | null {
  const active = threads.filter(
    (thread) => thread.projectId === projectId && thread.closedAt == null && thread.archivedAt == null
  );
  if (preferredThreadId) {
    const preferred = active.find((thread) => thread.id === preferredThreadId);
    if (preferred) return preferred.id;
  }
  return active[0]?.id ?? null;
}

export function isPlaceholderThreadTitle(title: string | null | undefined): boolean {
  const t = (title ?? '').trim();
  return t.length === 0 || t === '新会话' || t === '新对话' || t === '未命名会话';
}

/** 由首条用户 Query 与可选有效助手回复生成会话标题（与 Rust derive 对齐） */
export function deriveThreadTitleFromMessages(
  messages: { role: string; content: string }[]
): string {
  const collapse = (s: string) => s.split(/\s+/).filter(Boolean).join(' ');
  const clip = (s: string, max: number) => {
    const chars = [...s];
    return chars.length <= max ? s : `${chars.slice(0, max).join('')}…`;
  };
  const isRealAssistant = (m: { role: string; content: string }) =>
    m.role === 'assistant' &&
    !!m.content.trim() &&
    !m.content.startsWith('运行失败：') &&
    !m.content.startsWith('运行已取消');
  const user = messages.find((m) => m.role === 'user' && m.content.trim());
  const assistant = messages.find(isRealAssistant);
  const u = clip(collapse(user?.content ?? ''), 36);
  if (!u) return '对话';
  if (assistant) {
    const line =
      assistant.content
        .split('\n')
        .map((l) => l.trim())
        .find((l) => l.length > 0) ?? '';
    const a = clip(collapse(line), 20);
    if (a) return `${u} · ${a}`;
  }
  return u;
}

/** Run（单次运行） */
export interface AgentRun {
  id: string;
  threadId: string;
  projectId: string | null;
  status: RunStatus;
  provider: string;
  model: string;
  promptVersion: string | null;
  maxModelCalls: number;
  currentModelCalls: number;
  engine: string;
  engineVersion: string;
  createdAt: number;
  updatedAt: number;
}

/** 事件信封（与 Rust AgentEvent 对齐，camelCase） */
export interface AgentEvent {
  eventId: string;
  threadId: string;
  runId: string;
  seq: number;
  timestamp: number;
  schemaVersion: number;
  payload: AgentEventPayload;
}

/** AG-26：Run 选区上下文（与 Rust RunContext 对齐，camelCase）。
 * run_started 的增量字段——旧事件无 context 键 → undefined，按无选区处理 */
export interface RunContext {
  articleId: string;
  title: string;
  baseVersion: number;
  selectedMarkdown: string;
  selectedTextHash: string;
  beforeContext: string;
  afterContext: string;
}

/** AG-27：激活 Skill 引用（与 Rust RunSkillRef 对齐，camelCase）。
 * run_started 的增量字段——旧事件无 skill 键 → undefined，按未激活处理。
 * Worklog「Run 可见版本与来源」的数据源 */
export interface RunSkillRef {
  name: string;
  version: number;
  /** bundled / user / workspace */
  source: string;
}

/** DEC-014：Composer 选择后交给 Rust 校验/有界化的 Hermes 附件。 */
export type AgentAttachmentKind = 'image' | 'file' | 'folder' | 'url';

export interface AgentAttachmentInput {
  id: string;
  kind: AgentAttachmentKind;
  name: string;
  path?: string | null;
  url?: string | null;
  dataUrl?: string | null;
}

/** 事件 payload（与 Rust AgentEventPayload 对齐，snake_case tag + camelCase 字段） */
export type AgentEventPayload =
  | {
      type: 'run_started';
      userMessage: string;
      maxTurns: number;
      /** AG-26 增量字段（AG-21 先例：增量 optional 不升 schemaVersion） */
      context?: RunContext | null;
      /** AG-27 增量字段（同款口径：optional + 旧事件无键 → undefined） */
      skill?: RunSkillRef | null;
    }
  | { type: 'model_started'; turn: number }
  | { type: 'tool_started'; callId: string; name: string; argumentsJson: string }
  | {
      type: 'tool_completed';
      callId: string;
      name: string;
      ok: boolean;
      error: string | null;
      preresolved: boolean;
      /** AG-21：UI 渲染通道（卡片只消费这些字段，不解析 model_text——
       * model_text 根本不进事件 payload）。旧事件无这些字段 → optional */
      structured?: unknown;
      uiArtifact?: UiArtifact | null;
      truncated?: boolean;
      provenance?: ProvenanceRef[];
    }
  | { type: 'message_delta'; text: string; index?: number | null }
  | { type: 'message_completed'; text: string }
  | { type: 'message_interim'; text: string; alreadyStreamed: boolean }
  | { type: 'reasoning_delta'; text: string }
  /** 显式 thinking_end（Hermes reasoning.end；也可由首条 message_delta 合成） */
  | { type: 'reasoning_completed' }
  | {
      type: 'approval_required';
      approvalId: string;
      toolName: string;
      argumentsJson: string;
      choices: string[];
    }
  | {
      type: 'clarify_required';
      requestId: string;
      question: string;
      choices: string[];
    }
  | { type: 'engine_degraded'; reason: string; reconnecting: boolean }
  | { type: 'run_completed'; outcome: string; finalAnswer: string; modelCalls: number }
  | { type: 'run_failed'; outcome: string; error: string }
  | { type: 'run_cancelled'; reason: string };

/** 消息（前端渲染用，从事件流归约而来） */
export interface AgentMessage {
  id: string;
  threadId: string;
  runId: string;
  role: 'user' | 'assistant';
  content: string;
  createdAt: number;
  /** Bridge：推理/思考文本（不进正文；可折叠展示） */
  reasoning?: string | null;
  /**
   * 助手消息阶段（派生，不入事件 schema）：
   * thinking → answering（首条 message_delta / reasoning_completed）→ done | error
   */
  phase?: AssistantPhase;
  thinkingStatus?: ThinkingStatus;
  contentStatus?: ContentStatus;
  /** AG-26：该轮 Run 绑定的选区上下文（仅 user 消息；无选区 = undefined）。
   * Chat 头部渲染「绑定文章/选区/版本」chip 的数据源 */
  context?: RunContext | null;
  /** AG-27：该轮 Run 激活的 Skill（仅 user 消息；未激活 = undefined）。
   * 消息头部渲染「技能名 vX · 来源」chip 的数据源 */
  skill?: RunSkillRef | null;
}

/** AG-20：Run 状态快照（Rust RunSnapshot 对齐，agent_run_snapshot 返回）——
 * 真正的 state_snapshot：每一路都取自真相源表，可完整重建 UI 状态 */
export interface RunSnapshot {
  runId: string;
  threadId: string;
  /** Run 状态（来自 agent_runs 表，不经事件推断） */
  runStatus: RunStatus;
  /** 已持久化事件的最新 seq */
  latestSeq: number;
  /** Run 全量事件 JSON（seq 升序、含 seq=0），经同一 handleEvent 链路回灌 */
  events: string[];
  messages: AgentMessage[];
  toolCalls: unknown[];
  pendingApprovals: unknown[];
}

// ------------------- AG-20：缺口检测与 schema 降级工具 -------------------

/** 已知 payload 类型集合（与 AgentEventPayload 联合类型一一对应）。
 * 未知类型 = 更高协议版本的新事件 → 显式降级，不猜测归约 */
const KNOWN_PAYLOAD_TYPES = new Set<string>([
  'run_started',
  'model_started',
  'tool_started',
  'tool_completed',
  'message_delta',
  'message_completed',
  'message_interim',
  'reasoning_delta',
  'reasoning_completed',
  'approval_required',
  'clarify_required',
  'engine_degraded',
  'run_completed',
  'run_failed',
  'run_cancelled',
]);

function isTerminalEvent(event: AgentEvent): boolean {
  return event.payload.type === 'run_completed' ||
    event.payload.type === 'run_failed' ||
    event.payload.type === 'run_cancelled';
}

/** 事件是否可归约（AG-20：未知 schema/类型 → 显式降级；H4 接受 schema 1|2） */
export function isKnownEvent(event: AgentEvent): boolean {
  return (
    event.schemaVersion >= AGENT_EVENT_SCHEMA_MIN &&
    event.schemaVersion <= AGENT_EVENT_SCHEMA_VERSION &&
    KNOWN_PAYLOAD_TYPES.has(event.payload?.type)
  );
}

/** 找出事件列表中的第一个 seq 空洞（从 0 起严格连续；无空洞返回 null）。
 * 乱序/重复先归一成升序唯一序列再扫描 */
export function firstGapSeq(events: AgentEvent[]): number | null {
  if (events.length === 0) return null;
  const seqs = Array.from(new Set(events.map((e) => e.seq))).sort((a, b) => a - b);
  for (let i = 0; i < seqs.length; i++) {
    if (seqs[i] !== i) return i; // 期望 i（0 起连续），实际 seqs[i] > i → 空洞在 i
  }
  return null;
}

/** seq 缺口补齐阶梯的重试上限（replay 次数；用尽后升级 Snapshot） */
const REPLAY_ATTEMPTS = 2;

/** 重挂载后的 DB 追平频率。Channel 绑定旧 WebView，重新打开会话后只能从
 * RunStore 追平；终态到达前保持 Thread 发送门禁。 */
const RECOVERED_RUN_POLL_MS = 750;

function isTerminalRunStatus(status: RunStatus): boolean {
  return status === 'completed' || status === 'failed' || status === 'cancelled' || status === 'interrupted';
}

// ------------------- Store 状态 -------------------

interface AgentState {
  /** 活跃 Thread 列表（tab） */
  threads: AgentThread[];
  /** 历史 Thread（已关闭未归档；按项目 merge） */
  historyThreads: AgentThread[];
  /** 当前选中的 Thread */
  selectedThreadId: string | null;
  /** 当前运行中的 Run（按 threadId 索引） */
  activeRuns: Record<string, AgentRun>;
  /** 事件流（按 runId 索引，每个 Run 的事件列表） */
  eventsByRunId: Record<string, AgentEvent[]>;
  /** Thread 的 Run 到达顺序（跨 Run 消息归约用；实时与重放均按创建顺序到达） */
  runIdsByThreadId: Record<string, string[]>;
  /** 消息列表（按 threadId 索引，从事件流归约） */
  messagesByThreadId: Record<string, AgentMessage[]>;
  /** AG-21：工具结果卡（按 threadId 索引，从 tool_started/tool_completed 归约） */
  toolCardsByThreadId: Record<string, ToolCard[]>;
  /** 进行中的 Run（threadId → runId；AG-18：驱动 Chat 停止按钮） */
  runningRunByThreadId: Record<string, string>;
  /** Thread 历史正在装载（计数防并发 load 提前解锁）；装载完成前禁止启动下一轮。 */
  historyLoadingByThreadId: Record<string, number>;
  /** 从已关闭/卸载视图恢复的非终态 Run；终态前持续从 RunStore 追平。 */
  resumingRunByThreadId: Record<string, string>;
  /** 恢复监视器在途标记（runId → true；防重复轮询）。 */
  resumeInFlight: Record<string, boolean>;
  /** AG-20：seq 缺口恢复在途标记（runId → true；防重入触发） */
  recoveryInFlight: Record<string, boolean>;
  /** AG-20：显式降级登记（key = runId 或 `thread:<threadId>`；value = 原因）。
   * 未知协议版本/类型、无法解析的事件、补不齐的缺口都会登记——
   * 审计要求「显式降级，不静默跳过」，UI 可消费本字段提示用户 */
  degraded: Record<string, string>;
  /** 加载状态 */
  loading: boolean;

  /** 加载 Thread 列表（默认活跃；scope=history 拉历史抽屉） */
  loadThreads: (projectId?: string, scope?: ThreadListScope) => Promise<void>;
  /** 选中 Thread */
  selectThread: (id: string | null) => void;
  /** 显式新建空会话（+ 按钮） */
  createThread: (projectId?: string, title?: string) => Promise<string | null>;
  /** 关闭 → 历史 */
  closeThread: (threadId: string, projectId?: string) => Promise<boolean>;
  /** 历史恢复为活跃 */
  reopenThread: (threadId: string, projectId?: string) => Promise<boolean>;
  /** 归档（不可见） */
  archiveThread: (threadId: string, projectId?: string) => Promise<boolean>;
  /** 收藏夹分类列表（创建时间升序） */
  collections: ThreadCollection[];
  loadCollections: () => Promise<void>;
  createCollection: (name: string) => Promise<ThreadCollection | null>;
  /** 置顶/取消置顶（组织性操作，不扰动最近时序） */
  setThreadPinned: (threadId: string, pinned: boolean) => Promise<boolean>;
  /** 会话加入/移动/移出收藏夹分类（null = 移出） */
  setThreadCollection: (threadId: string, collectionId: string | null) => Promise<boolean>;
  /** 按可选 TTL 清理逾归档会话（ttlDays≤0 跳过；普通/历史会话永不因 TTL 删除） */
  gcThreads: (ttlDays?: number) => Promise<void>;
  /** 启动新 Run。AG-26：selection = 编辑器选区上下文（Chat 范围 chip 捕获），
   * 有值时只随 run_started 事件回传，供 Surface 展示和审计。
   * skill 经 Hermes command.dispatch 激活原生 Skill；SophoNote 不注入正文。
   * focusDocument 是当前 Surface 明示的文档范围，Rust 将其转为原生附件。 */
  startRun: (
    threadId: string | null,
    message: string,
    projectId?: string,
    selection?: RunContext | null,
    skill?: string | null,
    focusDocument?: FocusDocumentInput | null,
    attachments?: AgentAttachmentInput[],
    hermesModel?: string | null,
    hermesProvider?: string | null,
    hermesCommand?: string | null,
    includeProjectContext?: boolean,
    workspaceRoot?: string | null,
    workspacePermissionMode?: WorkspacePermissionMode,
  ) => Promise<{ threadId: string; runId: string } | null>;
  /** 处理事件（reducer 模式） */
  handleEvent: (event: AgentEvent) => void;
  /** 批量处理同一流式窗口的事件；只归约一次消息/Markdown。 */
  handleEvents: (events: AgentEvent[]) => void;
  /** 补全缺失事件（断线恢复） */
  replayEvents: (runId: string, afterSeq: number) => Promise<void>;
  /** AG-20：seq 缺口补齐阶梯——replay（≤REPLAY_ATTEMPTS 次）→ Snapshot 升级
   * → 仍缺口则显式降级。由 handleEvent 的缺口检测自动触发，也可手工调用 */
  recoverRun: (runId: string) => Promise<void>;
  /** AG-20：拉取 Run 状态快照并回灌事件（agent_run_snapshot） */
  loadRunSnapshot: (runId: string, reconcileOrphan?: boolean) => Promise<RunSnapshot | null>;
  /** 重挂载后持续追平失去 Channel 的 Run；只在终态事件或权威 Snapshot 终态时解锁。 */
  reconcileRecoveredRun: (threadId: string, runId: string) => Promise<void>;
  /** 恢复 Thread 全量历史（窗口重挂载恢复，AG-17：含 seq=0，跨 Run 串联） */
  loadThreadHistory: (threadId: string) => Promise<void>;
  /** /undo 后丢掉已裁掉 Run 的内存视图，再 loadThreadHistory 重建。 */
  forgetThreadView: (threadId: string) => void;
  /** 取消 Run（AG-18）：true = 取消信号已发出；终态可见性由 run_cancelled 事件归约承担 */
  cancelRun: (runId: string) => Promise<boolean>;
  /** 回传 Hermes 原生审批结果。 */
  respondApproval: (runId: string, choice: string, all?: boolean) => Promise<boolean>;
  /** 回传 Hermes 原生澄清回答。 */
  respondClarify: (runId: string, requestId: string, answer: string) => Promise<boolean>;
  /** 派生：Thread 的所有消息 */
  messagesOfThread: (threadId: string) => AgentMessage[];
  /** 派生：AG-21 Thread 的工具结果卡列表 */
  toolCardsOfThread: (threadId: string) => ToolCard[];
  /** 派生：Run 的事件列表 */
  eventsOfRun: (runId: string) => AgentEvent[];
  /** 派生：AG-20 当前 Thread 的降级原因（Thread 级或名下任一 Run 级；无则 null） */
  degradedOfThread: (threadId: string) => string | null;
}

// ------------------- 工具函数 -------------------

/** 定稿优先用 finalAnswer（权威终态）；流式仅用于进行中预览，避免丢换行挤成一堵墙 */
function pickCompletedAssistantContent(
  streamed: string | undefined,
  finalAnswer: string
): string {
  if (finalAnswer.trim()) return finalAnswer;
  return streamed ?? '';
}

/** Hermes 有时在回答结束后发 `reasoning.available`，其内容却是答案前缀。 */
function isAnswerEcho(content: string, candidate: string): boolean {
  const compact = (value: string) => value.replace(/\s+/g, '');
  const answer = compact(content);
  const reasoning = compact(candidate);
  return reasoning.length >= 16 && answer.startsWith(reasoning);
}

/** Hermes 会先把中间思考作为 message_delta 发出，再用 reasoning.available 回标。 */
function detachTrailingReasoning(content: string, candidate: string): string | null {
  const reasoning = candidate.trim();
  if (reasoning.replace(/\s+/g, '').length < 16) return null;
  const answer = content.trimEnd();
  if (!answer.endsWith(reasoning)) return null;
  return answer.slice(0, answer.length - reasoning.length).trimEnd();
}

type MarkdownNormalizeOptions = {
  /** 流式预览去掉无意义前导空白、保留尾部换行；终态去掉首尾空白。 */
  trim?: boolean;
};

/** 只改写围栏代码之外的行；图示/源码中的 Markdown 字符必须保持字面量。 */
function mapOutsideMarkdownFences(
  text: string,
  transform: (line: string) => string
): string {
  let fence: { char: '`' | '~'; length: number } | null = null;
  return text
    .split('\n')
    .map((line) => {
      const match = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
      if (match) {
        const marker = match[1];
        const char = marker[0] as '`' | '~';
        if (!fence) {
          fence = { char, length: marker.length };
        } else if (
          fence.char === char &&
          marker.length >= fence.length &&
          match[2].trim().length === 0
        ) {
          fence = null;
        }
        return line;
      }
      return fence ? line : transform(line);
    })
    .join('\n');
}

/** CommonMark 中 `**结论。**因为` 的收口不是 right-flanking，会原样显示星号。 */
function repairTightStrongBoundaries(line: string): string {
  const repairPlain = (value: string) => value.replace(
    /\*\*([^*\n]*?[。！？；：，、.!?;:])\*\*(?=[\p{L}\p{N}])/gu,
    '**$1** '
  );
  let cursor = 0;
  let result = '';
  while (cursor < line.length) {
    const tick = line.indexOf('`', cursor);
    if (tick < 0) return result + repairPlain(line.slice(cursor));
    result += repairPlain(line.slice(cursor, tick));
    let runEnd = tick + 1;
    while (line[runEnd] === '`') runEnd += 1;
    const marker = line.slice(tick, runEnd);
    const close = line.indexOf(marker, runEnd);
    if (close < 0) return result + line.slice(tick);
    result += line.slice(tick, close + marker.length);
    cursor = close + marker.length;
  }
  return result;
}

/**
 * 仅在「标题粘在同一行」时拆开；绝不改写表格行（| … |），
 * 并修复模型常见的紧邻 CJK 加粗收口。围栏代码内容始终原样保留。
 */
export function normalizeAssistantMarkdown(
  text: string,
  options: MarkdownNormalizeOptions = {}
): string {
  if (!text) return text;
  let out = text.replace(/\r\n?/g, '\n');
  out = mapOutsideMarkdownFences(out, (line) => {
      let normalized = repairTightStrongBoundaries(line);
      const trimmed = normalized.trimStart();
      if (trimmed.startsWith('|') || trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
        return normalized;
      }
      // 句中突然出现 ATX 标题
      normalized = normalized.replace(/([^\n#|])[ \t]*(#{1,6}[ \t]+\S)/g, '$1\n\n$2');
      return normalized;
    });
  // 单行整包倾泻：无换行时再尝试按标题切开
  if (!out.includes('\n') && /#{1,6}\s/.test(out)) {
    out = out.replace(/([^\n#|])[ \t]*(#{1,6}[ \t]+\S)/g, '$1\n\n$2');
  }
  out = mapOutsideMarkdownFences(out, (line) =>
    line.replace(/([。！？；])\s+(\d+\.\s+\S)/g, '$1\n$2')
  );
  return options.trim === false ? out.trimStart() : out.trim();
}

/**
 * 流式预览（对齐 Vercel Streamdown / remend）：
 * 轻量换行修复 → remend 闭合未完成加粗/链接等 → 闭合未完成代码围栏。
 */
export function stabilizeStreamingMarkdown(text: string): string {
  if (!text) return text;
  let out = normalizeAssistantMarkdown(text, { trim: false });
  try {
    out = remend(out, {
      links: true,
      images: false,
      katex: false,
      inlineKatex: false,
    });
  } catch {
    // remend 失败时仍返回规范化文本
  }
  const fenceStarts = out.match(/^ {0,3}(`{3,}|~{3,})/gm);
  if (fenceStarts && fenceStarts.length % 2 === 1) {
    out += '\n```';
  }
  return out;
}

/**
 * 从事件流归约消息列表。
 *
 * `message_interim` 是 Run 内的执行进度，不是面向用户的最终回复。
 * 它保留在事件流供执行区使用，但不再封存为多张 Agent 消息卡。
 */
function reduceMessages(events: AgentEvent[]): AgentMessage[] {
  const messages: AgentMessage[] = [];
  /** runId → 流式正文（终态前展示；定稿后并入 assistant） */
  const streaming = new Map<
    string,
    {
      threadId: string;
      content: string;
      reasoning: string;
      timestamp: number;
      phase: AssistantPhase;
      hasTools: boolean;
      /** 仅在 Hermes 缺失最终答案时作为兜底，不直接投影到对话。 */
      lastInterim?: string;
    }
  >();

  const flushStreaming = (runId: string) => {
    streaming.delete(runId);
  };

  const ensureStream = (event: AgentEvent) => {
    const cur = streaming.get(event.runId) ?? {
      threadId: event.threadId,
      content: '',
      reasoning: '',
      timestamp: event.timestamp,
      phase: 'thinking' as AssistantPhase,
      hasTools: false,
    };
    return cur;
  };

  const attachPhaseFields = (
    base: Omit<AgentMessage, 'phase' | 'thinkingStatus' | 'contentStatus'> & {
      phase: AssistantPhase;
      hasTools?: boolean;
    }
  ): AgentMessage => {
    const hasReasoning = Boolean(base.reasoning?.trim());
    const hasTools = Boolean(base.hasTools);
    const hasContent = Boolean(base.content.trim());
    const { hasTools: _drop, ...msg } = base as typeof base & { hasTools?: boolean };
    return {
      ...msg,
      phase: base.phase,
      thinkingStatus: deriveThinkingStatus({
        phase: base.phase,
        hasReasoning,
        hasTools,
      }),
      contentStatus: deriveContentStatus({
        phase: base.phase,
        hasContent,
      }),
    };
  };

  for (const event of events) {
    if (event.payload.type === 'run_started') {
      messages.push({
        id: `${event.runId}:user:${event.seq}`,
        threadId: event.threadId,
        runId: event.runId,
        role: 'user',
        content: event.payload.userMessage,
        createdAt: event.timestamp,
        context: event.payload.context ?? null,
        skill: event.payload.skill ?? null,
      });
      // 立刻挂上助手流式气泡，边推理边可见，避免长时间只见「正在处理」
      streaming.set(event.runId, {
        threadId: event.threadId,
        content: '',
        reasoning: '',
        timestamp: event.timestamp,
        phase: 'thinking',
        hasTools: false,
      });
    } else if (event.payload.type === 'message_delta') {
      const cur = ensureStream(event);
      cur.content += event.payload.text;
      cur.timestamp = event.timestamp;
      cur.phase = reduceAssistantPhase(cur.phase, 'message_delta');
      streaming.set(event.runId, cur);
    } else if (event.payload.type === 'reasoning_delta') {
      const cur = ensureStream(event);
      const detached = event.payload.text
        ? detachTrailingReasoning(cur.content, event.payload.text)
        : null;
      if (detached != null) {
        cur.content = detached;
        cur.reasoning += event.payload.text;
      } else if (event.payload.text && !isAnswerEcho(cur.content, event.payload.text)) {
        cur.reasoning += event.payload.text;
      } else if (!cur.reasoning) {
        // 空 reasoning 仅在真正的 thinking 阶段建立占位。回答后到达的
        // reasoning.available 若是答案回声，不再创建「正在梳理」假进度。
        if (cur.phase === 'thinking') cur.reasoning = '正在梳理思路…';
      }
      cur.timestamp = event.timestamp;
      cur.phase = detached != null && !cur.content.trim()
        ? 'thinking'
        : reduceAssistantPhase(cur.phase, 'reasoning_delta');
      streaming.set(event.runId, cur);
    } else if (event.payload.type === 'reasoning_completed') {
      const cur = ensureStream(event);
      cur.timestamp = event.timestamp;
      cur.phase = reduceAssistantPhase(cur.phase, 'reasoning_completed');
      streaming.set(event.runId, cur);
    } else if (
      event.payload.type === 'tool_started' ||
      event.payload.type === 'tool_completed'
    ) {
      const cur = ensureStream(event);
      cur.hasTools = true;
      cur.timestamp = event.timestamp;
      cur.phase = reduceAssistantPhase(cur.phase, event.payload.type);
      streaming.set(event.runId, cur);
    } else if (event.payload.type === 'message_completed') {
      const prev = streaming.get(event.runId);
      streaming.set(event.runId, {
        threadId: event.threadId,
        content: event.payload.text,
        reasoning: prev?.reasoning ?? '',
        timestamp: event.timestamp,
        phase: reduceAssistantPhase(prev?.phase ?? 'thinking', 'message_completed'),
        hasTools: prev?.hasTools ?? false,
        lastInterim: prev?.lastInterim,
      });
    } else if (event.payload.type === 'message_interim') {
      const stream = ensureStream(event);
      // Hermes 会把「我先读取…」这类过程说明作为 interim 封口。
      // 清空流式正文以接收后续最终答案，但不把它投影成对话消息。
      streaming.set(event.runId, {
        threadId: event.threadId,
        content: '',
        reasoning: '',
        timestamp: event.timestamp,
        phase: 'thinking',
        hasTools: stream.hasTools,
        lastInterim: event.payload.text || stream.content || stream.lastInterim,
      });
    } else if (event.payload.type === 'run_completed') {
      const stream = streaming.get(event.runId);
      const raw = pickCompletedAssistantContent(
        stream?.content || stream?.lastInterim,
        event.payload.finalAnswer
      );
      // 定稿同样只做代码围栏外的安全修复：既保留模型原文结构，也修复
      // `**结论。**因为` 这类 CommonMark 无法识别的紧邻强调边界。
      const content = normalizeAssistantMarkdown(raw);
      const reasoning = stream?.reasoning?.trim() && !isAnswerEcho(raw, stream.reasoning)
        ? stream.reasoning
        : null;
      const phase = reduceAssistantPhase(stream?.phase ?? 'answering', 'run_completed');
      flushStreaming(event.runId);
      messages.push(
        attachPhaseFields({
          id: `${event.runId}:assistant:${event.seq}`,
          threadId: event.threadId,
          runId: event.runId,
          role: 'assistant',
          content,
          reasoning,
          createdAt: event.timestamp,
          phase,
          hasTools: stream?.hasTools,
        })
      );
    } else if (event.payload.type === 'run_failed') {
      const stream = streaming.get(event.runId);
      flushStreaming(event.runId);
      messages.push(
        attachPhaseFields({
          id: `${event.runId}:assistant:${event.seq}`,
          threadId: event.threadId,
          runId: event.runId,
          role: 'assistant',
          content: `运行失败：${event.payload.error}`,
          reasoning: stream?.reasoning?.trim() ? stream.reasoning : null,
          createdAt: event.timestamp,
          phase: 'error',
          hasTools: stream?.hasTools,
        })
      );
    } else if (event.payload.type === 'run_cancelled') {
      const stream = streaming.get(event.runId);
      flushStreaming(event.runId);
      messages.push(
        attachPhaseFields({
          id: `${event.runId}:assistant:${event.seq}`,
          threadId: event.threadId,
          runId: event.runId,
          role: 'assistant',
          content: event.payload.reason ? `运行已取消：${event.payload.reason}` : '运行已取消',
          createdAt: event.timestamp,
          phase: 'error',
          hasTools: stream?.hasTools,
        })
      );
    }
  }

  // 仍在流式中的 Run：展示未定稿助手消息（engine_degraded 不终结）
  for (const [runId, s] of streaming) {
    messages.push(
      attachPhaseFields({
        id: `${runId}:assistant:streaming`,
        threadId: s.threadId,
        runId,
        role: 'assistant',
        content: s.content ? stabilizeStreamingMarkdown(s.content) : s.content,
        reasoning: s.reasoning.trim() ? s.reasoning : null,
        createdAt: s.timestamp,
        phase: s.phase,
        hasTools: s.hasTools,
      })
    );
  }
  return messages;
}

// Channel 热路径缓存以事件数组 identity 为边界：历史 Run 的数组不再变化，因而
// 不必在当前 Run 每个视觉帧重新建 eventId Set、重放消息和工具卡。replay/乱序会
// 生成新数组并自然失效，不把派生状态写入 localStorage 或跨会话共享。
const eventIdIndexCache = new WeakMap<AgentEvent[], Set<string>>();
const messageProjectionCache = new WeakMap<AgentEvent[], AgentMessage[]>();
const toolProjectionCache = new WeakMap<AgentEvent[], ToolCard[]>();

function eventIdIndex(events: AgentEvent[]): Set<string> {
  const cached = eventIdIndexCache.get(events);
  if (cached) return cached;
  const index = new Set(events.map((event) => event.eventId));
  eventIdIndexCache.set(events, index);
  return index;
}

function projectRunMessages(events: AgentEvent[]): AgentMessage[] {
  const cached = messageProjectionCache.get(events);
  if (cached) return cached;
  const projected = reduceMessages(events);
  messageProjectionCache.set(events, projected);
  return projected;
}

function projectRunToolCards(events: AgentEvent[]): ToolCard[] {
  const cached = toolProjectionCache.get(events);
  if (cached) return cached;
  const projected = reduceToolCards(events.filter(
    (event) => isToolEvent(event) || isToolCardTerminalEvent(event)
  ));
  toolProjectionCache.set(events, projected);
  return projected;
}

function appendOrderedEvents(
  existing: AgentEvent[],
  incoming: AgentEvent[],
): AgentEvent[] {
  if (incoming.length === 0) return existing;
  const tailSeq = existing.length > 0 ? existing[existing.length - 1].seq : -1;
  let previousSeq = tailSeq;
  let orderedTail = true;
  for (const event of incoming) {
    if (event.seq <= previousSeq) {
      orderedTail = false;
      break;
    }
    previousSeq = event.seq;
  }
  const combined = [...existing, ...incoming];
  if (!orderedTail) combined.sort((left, right) => left.seq - right.seq);
  // 当前 state 中 existing 会被 combined 替换；复用同一索引并只追加本批 ID，
  // 避免长回答每个视觉帧都 O(n) 复制数千个 eventId。
  const eventIds = eventIdIndex(existing);
  for (const event of incoming) eventIds.add(event.eventId);
  eventIdIndexCache.set(combined, eventIds);
  return combined;
}

// ------------------- Store 实现 -------------------

export const useAgentStore = create<AgentState>()((set, get) => ({
  threads: [],
  historyThreads: [],
  selectedThreadId: null,
  activeRuns: {},
  eventsByRunId: {},
  runIdsByThreadId: {},
  messagesByThreadId: {},
  toolCardsByThreadId: {},
  runningRunByThreadId: {},
  historyLoadingByThreadId: {},
  resumingRunByThreadId: {},
  resumeInFlight: {},
  recoveryInFlight: {},
  degraded: {},
  loading: false,

  loadThreads: async (projectId, scope = 'active') => {
    const existing = scope === 'history' ? get().historyThreads : get().threads;
    if (existing.length === 0) set({ loading: true });
    try {
      const result = await invoke<{ success: boolean; data: AgentThread[]; error: string | null }>(
        'agent_thread_list',
        { projectId: projectId ?? null, scope }
      );
      if (result.success) {
        set((state) =>
          scope === 'history'
            ? {
                historyThreads: mergeThreadsForProject(state.historyThreads, result.data, projectId),
                loading: false,
              }
            : {
                threads: mergeThreadsForProject(state.threads, result.data, projectId),
                loading: false,
              }
        );
      } else {
        console.error('Failed to load threads:', result.error);
        set({ loading: false });
      }
    } catch (e) {
      console.error('Failed to load threads:', e);
      set({ loading: false });
    }
  },

  selectThread: (id) => set({ selectedThreadId: id }),

  createThread: async (projectId, title = '新会话') => {
    try {
      const result = await invoke<{ success: boolean; data: string; error: string | null }>(
        'agent_thread_create',
        { title, projectId: projectId ?? null }
      );
      if (!result.success) {
        console.error('Failed to create thread:', result.error);
        return null;
      }
      await get().loadThreads(projectId, 'active');
      set({ selectedThreadId: result.data });
      return result.data;
    } catch (e) {
      console.error('Failed to create thread:', e);
      return null;
    }
  },

  closeThread: async (threadId, projectId) => {
    try {
      const result = await invoke<{ success: boolean; data: boolean; error: string | null }>(
        'agent_thread_close',
        { threadId }
      );
      if (!result.success) {
        console.error('Failed to close thread:', result.error);
        return false;
      }
      await Promise.all([
        get().loadThreads(projectId, 'active'),
        get().loadThreads(projectId, 'history'),
      ]);
      const state = get();
      if (state.selectedThreadId === threadId) {
        const next = resolveProjectThreadId(state.threads, projectId ?? null, null);
        set({ selectedThreadId: next });
      }
      return true;
    } catch (e) {
      console.error('Failed to close thread:', e);
      return false;
    }
  },

  reopenThread: async (threadId, projectId) => {
    try {
      const result = await invoke<{ success: boolean; error: string | null }>('agent_thread_reopen', {
        threadId,
      });
      if (!result.success) {
        console.error('Failed to reopen thread:', result.error);
        return false;
      }
      await Promise.all([
        get().loadThreads(projectId, 'active'),
        get().loadThreads(projectId, 'history'),
      ]);
      set({ selectedThreadId: threadId });
      return true;
    } catch (e) {
      console.error('Failed to reopen thread:', e);
      return false;
    }
  },

  archiveThread: async (threadId, projectId) => {
    try {
      const result = await invoke<{ success: boolean; error: string | null }>('agent_thread_archive', {
        threadId,
      });
      if (!result.success) {
        console.error('Failed to archive thread:', result.error);
        return false;
      }
      await Promise.all([
        get().loadThreads(projectId, 'active'),
        get().loadThreads(projectId, 'history'),
      ]);
      return true;
    } catch (e) {
      console.error('Failed to archive thread:', e);
      return false;
    }
  },

  collections: [],

  loadCollections: async () => {
    try {
      const result = await invoke<{
        success: boolean;
        data: ThreadCollection[];
        error: string | null;
      }>('agent_collection_list', {});
      if (result.success) {
        set({ collections: result.data });
      } else {
        console.error('Failed to load collections:', result.error);
      }
    } catch (e) {
      console.error('Failed to load collections:', e);
    }
  },

  createCollection: async (name) => {
    try {
      const result = await invoke<{
        success: boolean;
        data: ThreadCollection | null;
        error: string | null;
      }>('agent_collection_create', { name });
      if (!result.success) {
        console.error('Failed to create collection:', result.error);
        return null;
      }
      await get().loadCollections();
      return result.data;
    } catch (e) {
      console.error('Failed to create collection:', e);
      return null;
    }
  },

  setThreadPinned: async (threadId, pinned) => {
    try {
      const result = await invoke<{ success: boolean; error: string | null }>('agent_thread_pin', {
        threadId,
        pinned,
      });
      if (!result.success) {
        console.error('Failed to pin thread:', result.error);
        return false;
      }
      const patch = (list: AgentThread[]) =>
        list.map((t) => (t.id === threadId ? { ...t, pinnedAt: pinned ? Date.now() : null } : t));
      set((state) => ({ threads: patch(state.threads), historyThreads: patch(state.historyThreads) }));
      return true;
    } catch (e) {
      console.error('Failed to pin thread:', e);
      return false;
    }
  },

  setThreadCollection: async (threadId, collectionId) => {
    try {
      const result = await invoke<{ success: boolean; error: string | null }>(
        'agent_thread_set_collection',
        { threadId, collectionId }
      );
      if (!result.success) {
        console.error('Failed to set thread collection:', result.error);
        return false;
      }
      const patch = (list: AgentThread[]) =>
        list.map((t) => (t.id === threadId ? { ...t, collectionId } : t));
      set((state) => ({ threads: patch(state.threads), historyThreads: patch(state.historyThreads) }));
      return true;
    } catch (e) {
      console.error('Failed to set thread collection:', e);
      return false;
    }
  },

  gcThreads: async (ttlDays) => {
    const days = ttlDays ?? DEFAULT_THREAD_HISTORY_TTL_DAYS;
    if (days <= 0) return;
    try {
      await invoke('agent_thread_gc', { ttlDays: days });
    } catch (e) {
      console.error('Failed to gc threads:', e);
    }
  },

  startRun: async (
    threadId,
    message,
    projectId,
    selection,
    skill,
    focusDocument,
    attachments = [],
    hermesModel,
    hermesProvider,
    hermesCommand,
    includeProjectContext = false,
    workspaceRoot,
    workspacePermissionMode = 'ask',
  ) => {
    // ISSUE-019：同一 Thread 的历史仍在恢复或上一轮尚未终态时，状态层硬拒
    // 下一轮。不能只依赖 Composer disabled，否则快捷键/异步竞态仍可绕过。
    if (
      threadId &&
      ((get().historyLoadingByThreadId[threadId] ?? 0) > 0 ||
        get().runningRunByThreadId[threadId] != null ||
        get().resumingRunByThreadId[threadId] != null)
    ) {
      return null;
    }
    // 创建 Tauri Channel 接收事件流
    const channel = new Channel<AgentEvent>();
    const pendingEvents: AgentEvent[] = [];
    let flushTimer: ReturnType<typeof setTimeout> | null = null;
    let flushDueAt = 0;
    let pendingStreamChars = 0;
    let pendingStreamText = '';
    let lastFlushAt = Date.now();
    const flushPendingEvents = () => {
      if (flushTimer != null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      flushDueAt = 0;
      if (pendingEvents.length === 0) return;
      get().handleEvents(pendingEvents.splice(0, pendingEvents.length));
      pendingStreamChars = 0;
      pendingStreamText = '';
      lastFlushAt = Date.now();
    };
    const scheduleFlush = (delayMs: number) => {
      const dueAt = Date.now() + delayMs;
      if (flushTimer != null && flushDueAt <= dueAt) return;
      if (flushTimer != null) clearTimeout(flushTimer);
      flushDueAt = dueAt;
      flushTimer = setTimeout(flushPendingEvents, delayMs);
    };
    channel.onmessage = (event: AgentEvent) => {
      pendingEvents.push(event);
      if (isTerminalEvent(event)) {
        flushPendingEvents();
        return;
      }
      if (
        event.payload.type !== 'message_delta' &&
        event.payload.type !== 'reasoning_delta'
      ) {
        flushPendingEvents();
        return;
      }
      pendingStreamChars += event.payload.text.length;
      pendingStreamText += event.payload.text;
      scheduleFlush(streamFlushDelay({
        elapsedMs: Date.now() - lastFlushAt,
        pendingChars: pendingStreamChars,
        pendingEvents: pendingEvents.length,
        pendingText: pendingStreamText,
      }));
    };

    try {
      const result = await invoke<{
        success: boolean;
        data: { threadId: string; runId: string };
        error: string | null;
      }>(
        'agent_run_start',
        {
          request: {
            message,
            threadId,
            projectId: projectId ?? null,
            provider: null,
            system: null,
            maxTurns: null,
            // AG-26：选区上下文（无选区 = null → Rust serde(default) None）
            selection: selection ?? null,
            // AG-27：激活 Skill 名（未激活 = null → Rust serde(default) None）
            skill: skill ?? null,
            focusDocument: focusDocument ?? null,
            attachments: attachments.map(({ id: _id, ...attachment }) => attachment),
            hermesModel: hermesModel ?? null,
            hermesProvider: hermesProvider ?? null,
            hermesCommand: hermesCommand ?? null,
            includeProjectContext,
            workspaceRoot: workspaceRoot ?? null,
            workspacePermissionMode,
          },
          onEvent: channel,
        }
      );

      if (!result.success) {
        flushPendingEvents();
        console.error('Failed to start run:', result.error);
        return null;
      }

      // invoke 可在首批 Channel 事件之后才返回；先提交已到达的批次，
      // 使用户消息/思考骨架不必再等一个定时窗口。
      flushPendingEvents();

      // 立即登记 Thread/Run，让新会话不必等待二次列表查询即可被当前项目解析。
      // Channel 可能在 invoke 返回前已送达终态；这种极快完成场景不能重新挂上
      // running 标记，否则停止按钮会永久残留。
      set((state) => {
        const runEvents = state.eventsByRunId[result.data.runId] ?? [];
        const terminalEvent = runEvents.find((event) =>
          event.payload.type === 'run_completed' ||
          event.payload.type === 'run_failed' ||
          event.payload.type === 'run_cancelled'
        );
        const alreadyTerminal = terminalEvent != null;
        const threadStatus: ThreadStatus = terminalEvent?.payload.type === 'run_failed'
          ? 'failed'
          : terminalEvent?.payload.type === 'run_cancelled'
            ? 'cancelled'
            : terminalEvent?.payload.type === 'run_completed'
              ? 'completed'
              : 'running';
        const now = Date.now();
        const existingThread = state.threads.find((item) => item.id === result.data.threadId);
        const threads = existingThread
          ? state.threads.map((item) => item.id === result.data.threadId
            ? {
                ...item,
                status: threadStatus,
                latestRunId: result.data.runId,
                updatedAt: now,
              }
            : item)
          : [
              ...state.threads,
              {
                id: result.data.threadId,
                title: '新会话',
                status: threadStatus,
                projectId: projectId ?? null,
                latestRunId: result.data.runId,
                createdAt: now,
                updatedAt: now,
                closedAt: null,
                archivedAt: null,
              },
            ];
        return {
          threads,
          runningRunByThreadId: alreadyTerminal
            ? state.runningRunByThreadId
            : {
                ...state.runningRunByThreadId,
                [result.data.threadId]: result.data.runId,
              },
        };
      });

      // 列表刷新是后台校准，不属于“消息已发送”的完成条件。
      void get().loadThreads(projectId);

      return result.data;
    } catch (e) {
      flushPendingEvents();
      console.error('Failed to start run:', e);
      return null;
    }
  },

  handleEvent: (event) => get().handleEvents([event]),

  handleEvents: (incomingEvents) => {
    if (incomingEvents.length === 0) return;
    const touchedRunIds = new Set<string>();
    const reduceStartedAt = performance.now();

    set((state) => {
      let eventsByRunId = { ...state.eventsByRunId };
      let runIdsByThreadId = { ...state.runIdsByThreadId };
      let messagesByThreadId = state.messagesByThreadId;
      let toolCardsByThreadId = state.toolCardsByThreadId;
      let runningRunByThreadId = state.runningRunByThreadId;
      let resumingRunByThreadId = state.resumingRunByThreadId;
      let degraded = state.degraded;
      let threads = state.threads;

      const eventIdsByRun = new Map<string, Set<string>>();
      const acceptedEventsByRun = new Map<string, AgentEvent[]>();
      const terminalRunIds = new Set<string>();
      const changedMessageThreads = new Set<string>();
      const changedToolThreads = new Set<string>();
      const completedThreads = new Set<string>();
      let accepted = 0;

      for (const event of incomingEvents) {
        // AG-20：未知协议显式降级，不进入归约流。
        if (!isKnownEvent(event)) {
          const type = (event.payload as { type?: string } | undefined)?.type ?? 'undefined';
          degraded = {
            ...degraded,
            [event.runId]: `未知事件协议（schemaVersion=${event.schemaVersion}, type=${type}），该事件未归约`,
          };
          continue;
        }

        const { runId, threadId } = event;
        let eventIds = eventIdsByRun.get(runId);
        if (!eventIds) {
          eventIds = eventIdIndex(eventsByRunId[runId] ?? []);
          eventIdsByRun.set(runId, eventIds);
        }
        if (eventIds.has(event.eventId)) continue;
        eventIds.add(event.eventId);
        accepted += 1;
        touchedRunIds.add(runId);

        const acceptedForRun = acceptedEventsByRun.get(runId) ?? [];
        acceptedForRun.push(event);
        acceptedEventsByRun.set(runId, acceptedForRun);
        const runIds = runIdsByThreadId[threadId] ?? [];
        if (!runIds.includes(runId)) {
          runIdsByThreadId[threadId] = [...runIds, runId];
        }

        const payloadType = event.payload.type;
        if (
          payloadType === 'run_started' ||
          payloadType === 'message_delta' ||
          payloadType === 'message_completed' ||
          payloadType === 'message_interim' ||
          payloadType === 'reasoning_delta' ||
          payloadType === 'reasoning_completed' ||
          payloadType === 'tool_started' ||
          payloadType === 'tool_completed' ||
          payloadType === 'run_completed' ||
          payloadType === 'run_failed' ||
          payloadType === 'run_cancelled'
        ) {
          changedMessageThreads.add(threadId);
        }
        if (isToolEvent(event) || isTerminalEvent(event)) changedToolThreads.add(threadId);

        if (payloadType === 'engine_degraded') {
          degraded = {
            ...degraded,
            [runId]: event.payload.reconnecting
              ? `引擎降级（重连中）：${event.payload.reason}`
              : `引擎降级：${event.payload.reason}`,
          };
        }

        if (payloadType === 'run_started') {
          const alreadyTerminal = terminalRunIds.has(runId) ||
            (eventsByRunId[runId] ?? []).some(isTerminalEvent);
          if (!alreadyTerminal && runningRunByThreadId[threadId] == null) {
            runningRunByThreadId = {
              ...runningRunByThreadId,
              [threadId]: runId,
            };
          }
        }

        if (isTerminalEvent(event)) {
          terminalRunIds.add(runId);
          // 旧 Run 的迟到终态不得误清理同 Thread 更新一轮的运行标记。
          if (runningRunByThreadId[threadId] === runId) {
            runningRunByThreadId = { ...runningRunByThreadId };
            delete runningRunByThreadId[threadId];
          }
          if (resumingRunByThreadId[threadId] === runId) {
            resumingRunByThreadId = { ...resumingRunByThreadId };
            delete resumingRunByThreadId[threadId];
          }
          if (degraded[runId] != null) {
            degraded = { ...degraded };
            delete degraded[runId];
          }
          if (payloadType === 'run_completed') completedThreads.add(threadId);
        }
      }

      // 每个 Run 每批只扩容/排序一次；禁止按 1～3 字符 token 反复复制
      // 不断增长的事件数组，否则长回答会退化成 O(n²) 并堵住 WebView 渲染。
      for (const [runId, acceptedEvents] of acceptedEventsByRun) {
        eventsByRunId[runId] = appendOrderedEvents(
          eventsByRunId[runId] ?? [],
          acceptedEvents,
        );
      }

      if (accepted > 0 && changedMessageThreads.size > 0) {
        messagesByThreadId = { ...state.messagesByThreadId };
        for (const threadId of changedMessageThreads) {
          messagesByThreadId[threadId] = (runIdsByThreadId[threadId] ?? []).flatMap(
            (runId) => projectRunMessages(eventsByRunId[runId] ?? [])
          );
        }
      }

      if (accepted > 0 && changedToolThreads.size > 0) {
        toolCardsByThreadId = { ...state.toolCardsByThreadId };
        for (const threadId of changedToolThreads) {
          toolCardsByThreadId[threadId] = (runIdsByThreadId[threadId] ?? []).flatMap(
            (runId) => projectRunToolCards(eventsByRunId[runId] ?? [])
          );
        }
      }

      // 仅在成功回复后生成标题；批次内先归约消息，再取定稿内容。
      for (const threadId of completedThreads) {
        const threadMessages = messagesByThreadId[threadId] ?? [];
        const current = threads.find((thread) => thread.id === threadId);
        const realAssistant = threadMessages.find(
          (message) =>
            message.role === 'assistant' &&
            message.content.trim() &&
            !message.content.startsWith('运行失败：') &&
            !message.content.startsWith('运行已取消')
        );
        const hasUser = threadMessages.some(
          (message) => message.role === 'user' && message.content.trim()
        );
        if (!current || !hasUser || !realAssistant) continue;
        const nextTitle = deriveThreadTitleFromMessages(threadMessages);
        if (
          nextTitle &&
          nextTitle !== current.title &&
          (isPlaceholderThreadTitle(current.title) || !current.title.includes('·'))
        ) {
          threads = threads.map((thread) =>
            thread.id === threadId ? { ...thread, title: nextTitle, updatedAt: Date.now() } : thread
          );
        }
      }

      return {
        eventsByRunId,
        runIdsByThreadId,
        messagesByThreadId,
        toolCardsByThreadId,
        runningRunByThreadId,
        resumingRunByThreadId,
        degraded,
        threads,
      };
    });
    recordAgentStoreBatch(incomingEvents, performance.now() - reduceStartedAt);

    // 批次入库后再逐 Run 做一次缺口检测，避免把同批后续 seq 误判为丢失。
    for (const runId of touchedRunIds) {
      const eventsOfRun = get().eventsByRunId[runId] || [];
      if (
        firstGapSeq(eventsOfRun) != null &&
        !get().recoveryInFlight[runId] &&
        get().degraded[runId] == null
      ) {
        void get().recoverRun(runId);
      }
    }
  },

  recoverRun: async (runId) => {
    if (get().recoveryInFlight[runId]) return;
    set((state) => ({
      recoveryInFlight: { ...state.recoveryInFlight, [runId]: true },
    }));
    try {
      // 第一梯队：after_seq 重放（≤REPLAY_ATTEMPTS 次）。RunStore-first 顺序下
      // Channel 丢的事件 DB 一定已提交，重放通常一次即补齐
      for (let attempt = 0; attempt < REPLAY_ATTEMPTS; attempt++) {
        const events = get().eventsByRunId[runId] || [];
        const gap = firstGapSeq(events);
        if (gap == null) return; // 已连续，无需恢复
        if (gap === 0) break; // replay 是排他语义取不到 seq=0 → 直接升级 Snapshot
        await get().replayEvents(runId, gap - 1);
      }
      // 第二梯队：Snapshot 全量重同步（replay 填不上 / seq=0 缺失）
      if (firstGapSeq(get().eventsByRunId[runId] || []) != null) {
        await get().loadRunSnapshot(runId);
      }
      // 最终裁决：仍有空洞 = 事件在 DB 侧真实丢失（写库失败未提交）→
      // 显式降级（UI 可提示），不再循环触发（handleEvent 见 degraded 即停）
      const stillGap = firstGapSeq(get().eventsByRunId[runId] || []) != null;
      set((state) => {
        const degraded = { ...state.degraded };
        if (stillGap) {
          degraded[runId] = '事件缺口无法补齐（数据库可能存在事件丢失），本次运行展示可能不完整';
        } else {
          delete degraded[runId];
        }
        return { degraded };
      });
    } finally {
      set((state) => {
        const recoveryInFlight = { ...state.recoveryInFlight };
        delete recoveryInFlight[runId];
        return { recoveryInFlight };
      });
    }
  },

  loadRunSnapshot: async (runId, reconcileOrphan = false) => {
    try {
      const result = await invoke<{
        success: boolean;
        data: RunSnapshot;
        error: string | null;
      }>(reconcileOrphan ? 'agent_run_reconcile' : 'agent_run_snapshot', { runId });
      if (!result.success) {
        console.error('Failed to load run snapshot:', result.error);
        set((state) => ({
          degraded: {
            ...state.degraded,
            [runId]: `快照获取失败：${result.error ?? '未知错误'}`,
          },
        }));
        return null;
      }
      // 快照事件经同一批量归约链路回灌（eventId 幂等去重）。
      const parsedEvents: AgentEvent[] = [];
      let hasParseError = false;
      for (const json of result.data.events) {
        try {
          parsedEvents.push(JSON.parse(json) as AgentEvent);
        } catch {
          hasParseError = true;
        }
      }
      get().handleEvents(parsedEvents);
      if (hasParseError) {
        set((state) => ({
          degraded: {
            ...state.degraded,
            [runId]: '快照含无法解析的事件，已跳过该条',
          },
        }));
      }
      return result.data;
    } catch (e) {
      console.error('Failed to load run snapshot:', e);
      set((state) => ({
        degraded: {
          ...state.degraded,
          [runId]: `快照获取失败：${String(e)}`,
        },
      }));
      return null;
    }
  },

  reconcileRecoveredRun: async (threadId, runId) => {
    if (get().resumeInFlight[runId]) return;
    set((state) => ({
      resumeInFlight: { ...state.resumeInFlight, [runId]: true },
    }));

    try {
      while (get().resumingRunByThreadId[threadId] === runId) {
        const before = get().eventsByRunId[runId] ?? [];
        if (before.some(isTerminalEvent)) break;

        const afterSeq = before.reduce((max, event) => Math.max(max, event.seq), -1);
        if (afterSeq >= 0) await get().replayEvents(runId, afterSeq);
        if ((get().eventsByRunId[runId] ?? []).some(isTerminalEvent)) break;

        // replay 只回答“有没有新事件”；Snapshot 额外用 agent_runs 真相状态
        // 裁决已终态但终态事件异常缺失的情况，避免输入框永久锁死。
        const snapshot = await get().loadRunSnapshot(runId, true);
        if ((get().eventsByRunId[runId] ?? []).some(isTerminalEvent)) break;
        if (snapshot && isTerminalRunStatus(snapshot.runStatus)) {
          set((state) => {
            const runningRunByThreadId = { ...state.runningRunByThreadId };
            const resumingRunByThreadId = { ...state.resumingRunByThreadId };
            if (runningRunByThreadId[threadId] === runId) delete runningRunByThreadId[threadId];
            if (resumingRunByThreadId[threadId] === runId) delete resumingRunByThreadId[threadId];
            return {
              runningRunByThreadId,
              resumingRunByThreadId,
              degraded: {
                ...state.degraded,
                [runId]: `运行已进入终态（${snapshot.runStatus}），但缺少终态事件；已按状态快照结束恢复`,
              },
            };
          });
          break;
        }

        await new Promise((resolve) => setTimeout(resolve, RECOVERED_RUN_POLL_MS));
      }
    } finally {
      set((state) => {
        const resumeInFlight = { ...state.resumeInFlight };
        delete resumeInFlight[runId];
        return { resumeInFlight };
      });
    }
  },

  replayEvents: async (runId, afterSeq) => {
    try {
      const result = await invoke<{ success: boolean; data: string[]; error: string | null }>(
        'agent_run_events_replay',
        { runId, afterSeq }
      );
      if (!result.success) {
        console.error('Failed to replay events:', result.error);
        return;
      }
      const parsedEvents: AgentEvent[] = [];
      let hasParseError = false;
      for (const json of result.data) {
        try {
          parsedEvents.push(JSON.parse(json) as AgentEvent);
        } catch {
          hasParseError = true;
        }
      }
      get().handleEvents(parsedEvents);
      if (hasParseError) {
        set((state) => ({
          degraded: {
            ...state.degraded,
            [runId]: '重放流含无法解析的事件，已跳过该条',
          },
        }));
      }
    } catch (e) {
      console.error('Failed to replay events:', e);
    }
  },

  loadThreadHistory: async (threadId) => {
    set((state) => ({
      historyLoadingByThreadId: {
        ...state.historyLoadingByThreadId,
        [threadId]: (state.historyLoadingByThreadId[threadId] ?? 0) + 1,
      },
    }));
    try {
      const result = await invoke<{ success: boolean; data: string[]; error: string | null }>(
        'agent_thread_history',
        { threadId }
      );
      if (!result.success) {
        console.error('Failed to load thread history:', result.error);
        return;
      }
      // 历史事件一次批量回灌，避免打开长对话时按 token 反复重算。
      const parsedEvents: AgentEvent[] = [];
      let hasParseError = false;
      for (const json of result.data) {
        try {
          parsedEvents.push(JSON.parse(json) as AgentEvent);
        } catch {
          hasParseError = true;
        }
      }
      get().handleEvents(parsedEvents);
      if (hasParseError) {
        set((state) => ({
          degraded: {
            ...state.degraded,
            [`thread:${threadId}`]: '历史恢复含无法解析的事件，已跳过该条',
          },
        }));
      }

      // AG-20 重挂载/Channel 失效追平：历史里有事件但无终态事件的 Run =
      // 重挂载时仍在进行的运行——它的 Channel 已随旧窗口失效，后续事件
      // 只存在于 DB。对每个这样的 Run 立即 replay 一次（afterSeq=已见最大
      // seq，排他语义正好取「恢复时刻之后已提交」的事件），把视图追平到
      // 当前 DB 状态；追平后若仍缺口，handleEvent 的缺口检测会接管阶梯
      for (const runId of get().runIdsByThreadId[threadId] || []) {
        const events = get().eventsByRunId[runId] || [];
        const hasTerminal = events.some(isTerminalEvent);
        if (hasTerminal || events.length === 0) continue;
        set((state) => ({
          runningRunByThreadId: {
            ...state.runningRunByThreadId,
            [threadId]: runId,
          },
          resumingRunByThreadId: {
            ...state.resumingRunByThreadId,
            [threadId]: runId,
          },
        }));
        const afterSeq = events.reduce((m, e) => Math.max(m, e.seq), -1);
        if (afterSeq >= 0) {
          await get().replayEvents(runId, afterSeq);
        }
        if (!(get().eventsByRunId[runId] ?? []).some(isTerminalEvent)) {
          void get().reconcileRecoveredRun(threadId, runId);
        }
      }
    } catch (e) {
      console.error('Failed to load thread history:', e);
    } finally {
      set((state) => {
        const historyLoadingByThreadId = { ...state.historyLoadingByThreadId };
        const remaining = (historyLoadingByThreadId[threadId] ?? 1) - 1;
        if (remaining > 0) historyLoadingByThreadId[threadId] = remaining;
        else delete historyLoadingByThreadId[threadId];
        return { historyLoadingByThreadId };
      });
    }
  },

  forgetThreadView: (threadId) => {
    set((state) => {
      const runIds = state.runIdsByThreadId[threadId] ?? [];
      const eventsByRunId = { ...state.eventsByRunId };
      const degraded = { ...state.degraded };
      for (const runId of runIds) {
        delete eventsByRunId[runId];
        delete degraded[runId];
      }
      delete degraded[`thread:${threadId}`];
      const runIdsByThreadId = { ...state.runIdsByThreadId };
      delete runIdsByThreadId[threadId];
      const messagesByThreadId = { ...state.messagesByThreadId };
      delete messagesByThreadId[threadId];
      const toolCardsByThreadId = { ...state.toolCardsByThreadId };
      delete toolCardsByThreadId[threadId];
      const runningRunByThreadId = { ...state.runningRunByThreadId };
      delete runningRunByThreadId[threadId];
      const resumingRunByThreadId = { ...state.resumingRunByThreadId };
      delete resumingRunByThreadId[threadId];
      return {
        eventsByRunId,
        runIdsByThreadId,
        messagesByThreadId,
        toolCardsByThreadId,
        runningRunByThreadId,
        resumingRunByThreadId,
        degraded,
      };
    });
  },

  cancelRun: async (runId) => {
    try {
      const result = await invoke<{ success: boolean; data: boolean; error: string | null }>(
        'agent_run_cancel',
        { runId }
      );
      if (!result.success) {
        console.error('Failed to cancel run:', result.error);
        return false;
      }
      // data=true = 取消信号已派发（令牌已 cancel）；UI 停止态由 run_cancelled
      // 终态事件统一清理，此处不改 runningRunByThreadId
      return result.data;
    } catch (e) {
      console.error('Failed to cancel run:', e);
      return false;
    }
  },

  respondApproval: async (runId, choice, all = false) => {
    try {
      const result = await invoke<{ success: boolean; data: boolean; error: string | null }>(
        'agent_run_approval_respond',
        { runId, choice, all }
      );
      return result.success && result.data;
    } catch (error) {
      console.error('Failed to respond to Hermes approval:', error);
      return false;
    }
  },

  respondClarify: async (runId, requestId, answer) => {
    try {
      const result = await invoke<{ success: boolean; data: boolean; error: string | null }>(
        'agent_run_clarify_respond',
        { runId, requestId, answer }
      );
      return result.success && result.data;
    } catch (error) {
      console.error('Failed to respond to Hermes clarify request:', error);
      return false;
    }
  },

  messagesOfThread: (threadId) => {
    return get().messagesByThreadId[threadId] || [];
  },

  toolCardsOfThread: (threadId) => {
    return get().toolCardsByThreadId[threadId] || [];
  },

  eventsOfRun: (runId) => {
    return get().eventsByRunId[runId] || [];
  },

  degradedOfThread: (threadId) => {
    const { degraded, runIdsByThreadId } = get();
    const threadLevel = degraded[`thread:${threadId}`];
    if (threadLevel) return threadLevel;
    for (const runId of runIdsByThreadId[threadId] || []) {
      if (degraded[runId]) return degraded[runId];
    }
    return null;
  },
}));
