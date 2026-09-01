// ============================================================
// Track B · 智能体演进（AG-21 追加）：工具结果卡纯函数层
// 实施基线：docs/architecture.md UiArtifact（allowlist + 回退）+
// ToolOutput 五件套呈现契约。
//
// 铁律：
// ① 卡片只消费 structured/uiArtifact——model_text 根本不进事件 payload，
//    本层更不解析它（两条通道结构性解耦，「结果卡不解析 model_text」）；
// ② UiArtifact kind 前端按 allowlist 识别（Rust 侧已过第一道闸，这里是
//    纵深防御）：不识别的 kind 一律回退 fallbackMarkdown，仍可读不空白
//    （禁令 15：不承载任意 HTML/JS/CSS）；
// ③ 纯函数无副作用：upsert/reduce/视图选择全部确定性可单测。
// ============================================================

/** 来源引用（与 Rust ProvenanceRef 对齐，camelCase） */
export interface ProvenanceRef {
  source: string;
  sourceId?: string;
  title?: string;
  retrievedAt?: number;
}

/** 生成式 UI 安全包络（与 Rust UiArtifact 对齐，camelCase） */
export interface UiArtifact {
  kind: string;
  schemaVersion: number;
  payload: unknown;
  fallbackMarkdown: string;
  provenance?: ProvenanceRef[];
}

/** AG-21 kind allowlist（与 Rust UiArtifact::ALLOWED_KINDS 同集；
 * 两侧独立维护——前端不信任任何未识别 kind，即便它绕过 Rust 闸）。
 * AG-26：'diff' = 文档修改提案审批卡（propose_document_patch 的 UiArtifact）；
 * 'rename' = 标题改名提案审批卡（rename_article 的 UiArtifact） */
export const ALLOWED_ARTIFACT_KINDS = ['table', 'key-value', 'markdown', 'diff', 'rename'] as const;

/** diff 卡内的行级变更块（与 Rust PatchHunk camelCase 对齐） */
export interface DiffHunk {
  startLine: number;
  contextBefore: string[];
  removed: string[];
  added: string[];
  contextAfter: string[];
}

/** diff 卡载荷（= PatchPreview 的 JSON 投影；审批交互只需此子集 + operationId） */
export interface DiffPreviewPayload {
  operationId: string;
  documentId: string;
  title: string;
  baseVersion: number;
  targetVersion: number;
  /** 连续微调时恢复原锚点；旧事件可能没有，解析层回落空串。 */
  oldText: string;
  /** 当前提案正文，Chat 只审计不重复展示。 */
  newText: string;
  hunks: DiffHunk[];
  /** pending_approval / committed / rejected / failed / rolled_back */
  status: string;
  scope: 'selection' | 'current-block' | 'section' | null;
  rebased: boolean;
  /** NEXT-042：同一审批内的标题改提案（null = 仅正文）。整块批准时随正文生效。 */
  proposedTitle: string | null;
}

/** rename 卡载荷（= RenameArticleTool 提案的 JSON 投影；审批交互只需此子集） */
export interface RenamePreviewPayload {
  operationId: string;
  documentId: string;
  oldTitle: string;
  newTitle: string;
  /** 其它文档正文里引用旧标题双链的篇数（前缀粗匹配，仅展示影响范围） */
  wikilinkAffectedCount: number;
  /** pending_approval */
  status: string;
}

export type ToolCardStatus = 'running' | 'completed' | 'failed';

  /** 工具结果卡（前端渲染视图模型；由 tool_started/tool_completed 事件归约） */
export interface ToolCard {
  callId: string;
  runId: string;
  threadId: string;
  name: string;
  status: ToolCardStatus;
  startedAt: number;
  completedAt?: number;
  /** tool_started 原始参数（过程轨短摘要；不入答案正文） */
  argumentsJson?: string;
  /** 失败时的错误文本（与回填模型的文本同源） */
  error?: string | null;
  /** preresolved 标记（跳过执行的兄弟调用，从未真正执行） */
  preresolved?: boolean;
  /** 结构化结果（UI 渲染主通道） */
  structured?: unknown;
  /** 可选 UI 安全包络（kind 已过 Rust 侧 allowlist） */
  uiArtifact?: UiArtifact | null;
  /** 大结果截断标记 */
  truncated?: boolean;
  /** 来源引用（来源行展示） */
  provenance?: ProvenanceRef[];
}

/** 按 kind 选择出的视图（未知 kind → fallback；无 envelope → none） */
export type ArtifactView =
  | { mode: 'none' }
  | { mode: 'keyValue'; rows: [string, unknown][] }
  | { mode: 'table'; columns: string[]; rows: unknown[][] }
  | { mode: 'markdown'; markdown: string }
  | { mode: 'diff'; preview: DiffPreviewPayload }
  | { mode: 'rename'; preview: RenamePreviewPayload }
  | { mode: 'fallback'; markdown: string };

/** 工具事件的最小结构（与 AgentEvent 的工具子集结构兼容） */
export interface ToolEventLike {
  runId: string;
  threadId: string;
  timestamp: number;
  payload:
    | { type: 'tool_started'; callId: string; name: string; argumentsJson?: string }
    | {
        type: 'tool_completed';
        callId: string;
        name: string;
        ok: boolean;
        error: string | null;
        preresolved: boolean;
        /** AG-21 新字段：旧事件（AG-21 前落库）无这些字段 → 按缺省处理 */
        structured?: unknown;
        uiArtifact?: UiArtifact | null;
        truncated?: boolean;
        provenance?: ProvenanceRef[];
      };
}

export interface RunTerminalEventLike {
  runId: string;
  threadId: string;
  timestamp: number;
  payload:
    | { type: 'run_completed'; outcome: string; finalAnswer: string; modelCalls: number }
    | { type: 'run_failed'; outcome: string; error: string }
    | { type: 'run_cancelled'; reason: string };
}

/** 判别：事件是否为工具事件（tool_started / tool_completed）。
 * 泛型交叉类型谓词：对 AgentEvent[] 使用 filter(isToolEvent) 时
 * 能正确收窄为 (AgentEvent & ToolEventLike)[] */
export function isToolEvent<
  T extends { runId: string; threadId: string; timestamp: number; payload: { type: string } },
>(e: T): e is T & ToolEventLike {
  return e.payload.type === 'tool_started' || e.payload.type === 'tool_completed';
}

/** 工具卡只需要识别 Run 的三种终态，作为悬空工具的上层收口边界。 */
export function isToolCardTerminalEvent<
  T extends { runId: string; threadId: string; timestamp: number; payload: { type: string } },
>(e: T): e is T & RunTerminalEventLike {
  return e.payload.type === 'run_completed' ||
    e.payload.type === 'run_failed' ||
    e.payload.type === 'run_cancelled';
}

/** 按 callId 幂等 upsert：started 建卡，completed 补全
 * （重放场景可能只有 completed 无 started → 直接建完成卡） */
export function upsertToolCard(cards: ToolCard[], event: ToolEventLike): ToolCard[] {
  const p = event.payload;
  const idx = cards.findIndex((c) => c.callId === p.callId);

  if (p.type === 'tool_started') {
    if (idx >= 0) return cards; // 幂等：重复 started 不重复建卡
    return [
      ...cards,
      {
        callId: p.callId,
        runId: event.runId,
        threadId: event.threadId,
        name: p.name,
        status: 'running',
        startedAt: event.timestamp,
        argumentsJson: p.argumentsJson,
      },
    ];
  }

  // tool_completed：有起始卡则补全，无则整卡直建（重放/历史路径）
  const base: ToolCard =
    idx >= 0
      ? cards[idx]
      : {
          callId: p.callId,
          runId: event.runId,
          threadId: event.threadId,
          name: p.name,
          status: 'running',
          startedAt: event.timestamp,
        };
  const filled: ToolCard = {
    ...base,
    status: p.ok ? 'completed' : 'failed',
    completedAt: event.timestamp,
    error: p.error,
    preresolved: p.preresolved,
    structured: p.structured,
    uiArtifact: p.uiArtifact ?? null,
    truncated: p.truncated ?? false,
    provenance: p.provenance ?? [],
  };
  if (idx >= 0) {
    const next = cards.slice();
    next[idx] = filled;
    return next;
  }
  return [...cards, filled];
}

/** 从事件流归约工具卡列表（与 reduceMessages 同口径：全量、确定性、按事件序） */
export function reduceToolCards(events: Array<ToolEventLike | RunTerminalEventLike>): ToolCard[] {
  let cards: ToolCard[] = [];
  for (const e of events) {
    if (isToolEvent(e)) {
      cards = upsertToolCard(cards, e);
      continue;
    }
    const terminalError = e.payload.type === 'run_completed'
      ? null
      : e.payload.type === 'run_failed'
        ? e.payload.error
        : e.payload.reason;
    cards = cards.map((card) => card.runId === e.runId && card.status === 'running'
      ? {
          ...card,
          status: terminalError == null ? 'completed' : 'failed',
          completedAt: e.timestamp,
          error: terminalError,
        }
      : card);
  }
  return cards;
}

/** 按 kind 选择渲染视图：allowlist 内 → 专用视图；
 * 未识别 kind / payload 形状不合约定 → fallbackMarkdown（不空白、不执行） */
export function pickArtifactView(card: ToolCard): ArtifactView {
  const artifact = card.uiArtifact;
  if (!artifact) return { mode: 'none' };

  switch (artifact.kind) {
    case 'key-value': {
      const rows = parseKeyValueRows(artifact.payload);
      return rows.length > 0
        ? { mode: 'keyValue', rows }
        : { mode: 'fallback', markdown: artifact.fallbackMarkdown };
    }
    case 'table': {
      const parsed = parseTable(artifact.payload);
      return parsed
        ? { mode: 'table', columns: parsed.columns, rows: parsed.rows }
        : { mode: 'fallback', markdown: artifact.fallbackMarkdown };
    }
    case 'markdown': {
      const md = (artifact.payload as { markdown?: unknown } | null | undefined)?.markdown;
      return typeof md === 'string' && md.length > 0
        ? { mode: 'markdown', markdown: md }
        : { mode: 'fallback', markdown: artifact.fallbackMarkdown };
    }
    case 'diff': {
      const parsed = parseDiff(artifact.payload);
      return parsed
        ? { mode: 'diff', preview: parsed }
        : { mode: 'fallback', markdown: artifact.fallbackMarkdown };
    }
    case 'rename': {
      const parsed = parseRename(artifact.payload);
      return parsed
        ? { mode: 'rename', preview: parsed }
        : { mode: 'fallback', markdown: artifact.fallbackMarkdown };
    }
    default:
      // 未知 kind（allowlist 外的未来 kind）→ Markdown 回退
      return { mode: 'fallback', markdown: artifact.fallbackMarkdown };
  }
}

/** rename payload 解析（RenameArticleTool 提案 camelCase 投影）：
 * 关键字段齐全 → 视图；畸形 → null（回退 fallbackMarkdown，不渲染半残卡） */
export function parseRename(payload: unknown): RenamePreviewPayload | null {
  const p = payload as Partial<RenamePreviewPayload> | null | undefined;
  if (!p || typeof p !== 'object') return null;
  if (typeof p.operationId !== 'string' || p.operationId.length === 0) return null;
  if (typeof p.documentId !== 'string' || p.documentId.length === 0) return null;
  if (typeof p.oldTitle !== 'string' || typeof p.newTitle !== 'string') return null;
  if (p.newTitle.length === 0) return null;
  return {
    operationId: p.operationId,
    documentId: p.documentId,
    oldTitle: p.oldTitle,
    newTitle: p.newTitle,
    wikilinkAffectedCount:
      typeof p.wikilinkAffectedCount === 'number' ? p.wikilinkAffectedCount : 0,
    status: typeof p.status === 'string' ? p.status : 'pending_approval',
  };
}

/** diff payload 解析（PatchPreview camelCase 投影）：关键字段齐全 → 视图；畸形 → null。
 *  审批交互依赖 operationId/documentId/hunks——缺一即回退 fallbackMarkdown，不渲染半残卡 */
export function parseDiff(payload: unknown): DiffPreviewPayload | null {
  const p = payload as Partial<DiffPreviewPayload> | null | undefined;
  if (!p || typeof p !== 'object') return null;
  if (typeof p.operationId !== 'string' || p.operationId.length === 0) return null;
  if (typeof p.documentId !== 'string' || p.documentId.length === 0) return null;
  if (!Array.isArray(p.hunks)) return null;
  const hunks: DiffHunk[] = [];
  for (const h of p.hunks) {
    const hh = h as Partial<DiffHunk> | null | undefined;
    if (!hh || typeof hh !== 'object' || typeof hh.startLine !== 'number') return null;
    hunks.push({
      startLine: hh.startLine,
      contextBefore: Array.isArray(hh.contextBefore) ? hh.contextBefore.map(String) : [],
      removed: Array.isArray(hh.removed) ? hh.removed.map(String) : [],
      added: Array.isArray(hh.added) ? hh.added.map(String) : [],
      contextAfter: Array.isArray(hh.contextAfter) ? hh.contextAfter.map(String) : [],
    });
  }
  return {
    operationId: p.operationId,
    documentId: p.documentId,
    title: typeof p.title === 'string' ? p.title : '',
    baseVersion: typeof p.baseVersion === 'number' ? p.baseVersion : 0,
    targetVersion: typeof p.targetVersion === 'number' ? p.targetVersion : 0,
    oldText: typeof p.oldText === 'string' ? p.oldText : '',
    newText: typeof p.newText === 'string' ? p.newText : '',
    hunks,
    status: typeof p.status === 'string' ? p.status : 'pending_approval',
    scope: p.scope === 'selection' || p.scope === 'current-block' || p.scope === 'section' ? p.scope : null,
    rebased: p.rebased === true,
    proposedTitle: typeof p.proposedTitle === 'string' && p.proposedTitle.length > 0 ? p.proposedTitle : null,
  };
}

/** key-value payload 解析：rows = [[label, value], ...]；畸形 → 空数组 */
function parseKeyValueRows(payload: unknown): [string, unknown][] {
  const rows = (payload as { rows?: unknown } | null | undefined)?.rows;
  if (!Array.isArray(rows)) return [];
  const out: [string, unknown][] = [];
  for (const row of rows) {
    if (Array.isArray(row) && row.length >= 2 && typeof row[0] === 'string') {
      out.push([row[0], row[1]]);
    }
  }
  return out;
}

/** table payload 解析：columns = string[]、rows = unknown[][]；畸形 → null */
function parseTable(payload: unknown): { columns: string[]; rows: unknown[][] } | null {
  const p = payload as { columns?: unknown; rows?: unknown } | null | undefined;
  if (!p || !Array.isArray(p.columns) || !Array.isArray(p.rows)) return null;
  const columns = p.columns.filter((c): c is string => typeof c === 'string');
  if (columns.length === 0) return null;
  const rows = p.rows.filter((r): r is unknown[] => Array.isArray(r));
  return { columns, rows };
}
