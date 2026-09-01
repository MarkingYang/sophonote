// ============================================================
// Agent Chat 过程轨（思维链 P0）：推理 + 工具步骤归并纯函数
// 对齐 docs/architecture.md 过程轨：推理与工具步骤归并
// ============================================================
import { pickArtifactView, type ToolCard } from './agentToolCards';
import type { AgentEvent } from '../stores/agentStore';

export type RunPhase = 'waiting' | 'tooling' | 'answering';

/** 时间线仍独立展示的富工具卡（需用户操作 / 大块结果）；其余进过程轨紧凑步骤 */
export function isRichTimelineToolCard(card: ToolCard): boolean {
  const view = pickArtifactView(card);
  return (
    view.mode === 'diff' ||
    view.mode === 'rename' ||
    view.mode === 'table' ||
    view.mode === 'keyValue'
  );
}

export function groupToolCardsByRunId(cards: ToolCard[]): Record<string, ToolCard[]> {
  const map: Record<string, ToolCard[]> = {};
  for (const card of cards) {
    const list = map[card.runId] ?? (map[card.runId] = []);
    list.push(card);
  }
  for (const runId of Object.keys(map)) {
    map[runId].sort((a, b) => a.startedAt - b.startedAt);
  }
  return map;
}

/** 时间线只挂富卡；只读过程步（list/read 等）不单独占行 */
export function timelineToolCards(cards: ToolCard[]): ToolCard[] {
  return cards.filter(isRichTimelineToolCard);
}

export function deriveRunPhase(input: {
  streaming: boolean;
  hasRunningTool: boolean;
  hasAssistantContent: boolean;
}): RunPhase {
  if (!input.streaming) return 'answering';
  if (input.hasRunningTool) return 'tooling';
  if (input.hasAssistantContent) return 'answering';
  return 'waiting';
}

export function runPhaseLabel(phase: RunPhase): string {
  switch (phase) {
    case 'tooling':
      return '调用工具中';
    case 'answering':
      return '生成回复中';
    default:
      return '等待模型…';
  }
}

/**
 * Hermes/MCP 会按客户端版本产出多种命名空间格式，例如：
 * `mcp__sophonote_bridge__read_document`、`mcp_sophonote-bridge_read_document`。
 * 过程轨只基于去命名空间后的稳定工具名做分类和展示。
 */
function bareToolName(name: string): string {
  return name
    .replace(/^mcp(?:__|_)sophonote(?:-|_)bridge(?:__|_)+/i, '')
    .replace(/^sophonote(?:-|_)bridge(?:__|_)+/i, '');
}

/** 面向用户的工具名（与 Chat 面板口径一致） */
export function toolDisplayName(name: string): string {
  const bare = bareToolName(name);
  switch (bare) {
    case 'list_project_documents':
      return '查看项目文档';
    case 'read_document':
      return '读取文档';
    case 'propose_document_patch':
    case 'sophonote_document_patch':
      return '起草文档修改';
    case 'rename_article':
      return '重命名文档';
    case 'create_document':
      return '创建文档';
    case 'move_document':
      return '移动文档';
    case 'set_document_parent':
    case 'sophonote_project_tree':
      return '整理项目目录';
    case 'calculator':
      return '计算';
    case 'weather':
      return '查询天气';
    case 'web_search':
      return '搜索网页';
    case 'web_extract':
      return '读取网页';
    case 'browser_navigate':
      return '打开网页';
    case 'browser_snapshot':
      return '读取页面';
    case 'browser_click':
      return '点击页面';
    case 'browser_type':
      return '输入内容';
    case 'browser_scroll':
      return '滚动页面';
    case 'browser_console':
      return '检查控制台';
    case 'browser_vision':
      return '截图验证';
    case 'read_file':
      return '读取文件';
    case 'write_file':
    case 'patch':
      return '修改文件';
    case 'search_files':
      return '搜索代码';
    case 'terminal':
      return '执行命令';
    case 'process':
      return '管理后台进程';
    default:
      return bare || name;
  }
}

function looksLikeInternal(s: string): boolean {
  const t = s.toLowerCase();
  return (
    t.includes('localhost') ||
    t.includes('127.0.0.1') ||
    t.includes('/mcp') ||
    t.includes('bearer') ||
    t.includes('sophonote-bridge')
  );
}

/** 过程轨短参数摘要：只取安全字段，不展示 URL/密钥 */
export function toolStepSummary(card: ToolCard): string | null {
  const fromProvenance = card.provenance?.find((p) => p.title)?.title;
  if (fromProvenance && !looksLikeInternal(fromProvenance)) {
    return `《${fromProvenance}》`;
  }
  const structured = card.structured;
  if (structured && typeof structured === 'object' && structured !== null) {
    const obj = structured as Record<string, unknown>;
    const title = typeof obj.title === 'string' ? obj.title : null;
    if (title && !looksLikeInternal(title)) return `《${title}》`;
    const articleId = typeof obj.articleId === 'string' ? obj.articleId : null;
    if (articleId && articleId.length <= 40 && !looksLikeInternal(articleId)) {
      return articleId;
    }
  }
  if (card.argumentsJson) {
    try {
      const args = JSON.parse(card.argumentsJson) as Record<string, unknown>;
      const title = typeof args.title === 'string' ? args.title : null;
      if (title && !looksLikeInternal(title)) return `《${title}》`;
      const articleId =
        typeof args.articleId === 'string'
          ? args.articleId
          : typeof args.article_id === 'string'
            ? args.article_id
            : null;
      if (articleId && articleId.length <= 40 && !looksLikeInternal(articleId)) {
        return articleId;
      }
      const instruction = typeof args.instruction === 'string' ? args.instruction.trim() : null;
      if (instruction && !looksLikeInternal(instruction)) {
        const flat = instruction.replace(/\s+/g, ' ');
        return flat.length > 36 ? `${flat.slice(0, 36)}…` : flat;
      }
      const bare = bareToolName(card.name);
      if (bare === 'web_search') {
        const query = typeof args.query === 'string' ? args.query.trim() : null;
        if (query && !looksLikeInternal(query)) return query.length > 36 ? `${query.slice(0, 36)}…` : query;
      }
      if (bare === 'web_extract' || bare.startsWith('browser_')) {
        const rawUrl = typeof args.url === 'string' ? args.url.trim() : null;
        if (rawUrl) {
          try {
            const url = new URL(rawUrl);
            if ((url.protocol === 'http:' || url.protocol === 'https:') && !url.username && !url.password) {
              const path = url.pathname === '/' ? '' : url.pathname;
              const summary = `${url.host}${path}`;
              return summary.length > 44 ? `${summary.slice(0, 44)}…` : summary;
            }
          } catch {
            /* ignore invalid URL */
          }
        }
      }
    } catch {
      /* ignore */
    }
  }
  return null;
}

export type ProcessActivity =
  | {
      kind: 'reasoning';
      id: string;
      text: string;
      startedAt: number;
      endedAt?: number;
      running: boolean;
    }
  | {
      kind: 'tools';
      id: string;
      cards: ToolCard[];
      startedAt: number;
      endedAt?: number;
      running: boolean;
    };

/**
 * 按 RunStore seq 重建 Hermes Desktop 式过程块：连续 reasoning delta 合为
 * 一个 Thought，连续工具合为一个 Explored；二者交替时保留真实先后顺序。
 */
export function buildProcessActivities(
  events: readonly AgentEvent[],
  cards: readonly ToolCard[],
  phase: 'thinking' | 'answering' | 'done' | 'error'
): ProcessActivity[] {
  let alreadyOrdered = true;
  for (let index = 1; index < events.length; index += 1) {
    if (events[index - 1].seq > events[index].seq) {
      alreadyOrdered = false;
      break;
    }
  }
  const ordered = alreadyOrdered ? events : [...events].sort((left, right) => left.seq - right.seq);
  const cardsByCallId = new Map(cards.map((card) => [card.callId, card]));
  const activities: ProcessActivity[] = [];

  for (const event of ordered) {
    if (event.payload.type === 'reasoning_delta' && event.payload.text) {
      const previous = activities[activities.length - 1];
      if (previous?.kind === 'reasoning') {
        previous.text += event.payload.text;
      } else {
        activities.push({
          kind: 'reasoning',
          id: `reasoning-${event.seq}`,
          text: event.payload.text,
          startedAt: event.timestamp,
          running: false,
        });
      }
      continue;
    }
    if (event.payload.type === 'tool_started') {
      const card = cardsByCallId.get(event.payload.callId);
      if (!card) continue;
      const previous = activities[activities.length - 1];
      if (previous?.kind === 'tools') {
        if (!previous.cards.some((item) => item.callId === card.callId)) {
          previous.cards.push(card);
        }
      } else {
        activities.push({
          kind: 'tools',
          id: `tools-${event.seq}`,
          cards: [card],
          startedAt: event.timestamp,
          running: false,
        });
      }
    }
  }

  const terminalAt = [...ordered]
    .reverse()
    .find((event) =>
      event.payload.type === 'run_completed' ||
      event.payload.type === 'run_failed' ||
      event.payload.type === 'run_cancelled'
    )?.timestamp;

  activities.forEach((activity, index) => {
    const nextStartedAt = activities[index + 1]?.startedAt;
    if (activity.kind === 'tools') {
      // 断连/取消窗口可能错过 tool.complete，但 Run 终态是更高层的硬边界。
      // 终态之后不得继续把历史工具画成执行中，也不得让耗时持续增长。
      activity.running = terminalAt == null && activity.cards.some((card) => card.status === 'running');
      activity.endedAt = nextStartedAt ?? terminalAt ?? Math.max(
        ...activity.cards.map((card) => card.completedAt ?? card.startedAt)
      );
      return;
    }
    activity.running = phase === 'thinking' && index === activities.length - 1;
    activity.endedAt = nextStartedAt ?? (activity.running ? undefined : terminalAt);
  });

  return activities;
}

const EXPLORE_TOOL_NAMES = new Set([
  'list_files',
  'read_file',
  'search_files',
  'list_project_documents',
  'read_document',
  'session_search_recall',
  'vision_analyze',
  'web_extract',
  'web_search',
]);

function isExploreTool(name: string): boolean {
  const bare = bareToolName(name);
  return EXPLORE_TOOL_NAMES.has(bare) || bare.startsWith('browser_');
}

/** Hermes Desktop 风格工具组摘要。 */
export function toolRunActivityLabel(cards: readonly ToolCard[], live: boolean): string {
  const exploreCount = cards.filter((card) => isExploreTool(card.name)).length;
  const otherCount = cards.length - exploreCount;
  const parts: string[] = [];
  if (exploreCount > 0) {
    if (exploreCount === 1) {
      const target = toolStepSummary(cards.find((card) => isExploreTool(card.name))!);
      parts.push(`${live ? 'Exploring' : 'Explored'}${target ? ` ${target}` : ' 1 file'}`);
    } else {
      parts.push(`${live ? 'Exploring' : 'Explored'} ${exploreCount} files`);
    }
  }
  if (otherCount > 0) {
    parts.push(`${live ? 'Using' : 'Used'} ${otherCount} ${otherCount === 1 ? 'tool' : 'tools'}`);
  }
  return parts.join(', ') || (live ? 'Working…' : 'Completed');
}

export function activityDurationMs(activity: ProcessActivity, now = Date.now()): number {
  return Math.max(0, (activity.endedAt ?? now) - activity.startedAt);
}

export function processRailSummaryLabel(input: {
  streaming: boolean;
  hasTools: boolean;
  durationMs?: number;
  formatDuration: (ms: number) => string;
}): string {
  if (input.streaming) {
    return input.hasTools ? '执行过程 · 进行中' : '思考中…';
  }
  if (input.durationMs != null) {
    return `已思考 · ${input.formatDuration(input.durationMs)}`;
  }
  return input.hasTools ? '执行过程' : '思考过程';
}

export function shouldShowProcessRail(input: {
  streaming: boolean;
  hasReasoning: boolean;
  hasTools: boolean;
  /** 答案轨已有正文：无真实过程内容时不再占过程轨 */
  hasAnswer?: boolean;
}): boolean {
  if (input.hasReasoning || input.hasTools) return true;
  // 仅等待阶段占位；开始出答案后让答案轨独占，避免结果看起来「在思维链里」
  if (input.streaming && !input.hasAnswer) return true;
  return false;
}
