// ============================================================
// 助手消息阶段状态机（对齐 thinking-chain-ui-requirement）
// 需求 thinking_delta/thinking_end/content_delta/done
// ↔ SophoNote reasoning_delta / 首条 message_delta / message_delta / run_completed
// ============================================================

export type AssistantPhase = 'thinking' | 'answering' | 'done' | 'error';

export type ThinkingStatus = 'none' | 'streaming' | 'done';
export type ContentStatus = 'pending' | 'streaming' | 'done';

/** 事件 payload.type 的最小集合（供纯函数归约，不依赖完整 AgentEvent） */
export type PhaseEventType =
  | 'run_started'
  | 'model_started'
  | 'reasoning_delta'
  | 'reasoning_completed'
  | 'tool_started'
  | 'tool_completed'
  | 'message_delta'
  | 'message_completed'
  | 'message_interim'
  | 'run_completed'
  | 'run_failed'
  | 'run_cancelled'
  | 'engine_degraded'
  | 'approval_required'
  | 'clarify_required'
  | string;

/** 单事件推进阶段（thinking_end = 首条 message_delta 或显式 reasoning_completed） */
export function reduceAssistantPhase(
  current: AssistantPhase | null,
  eventType: PhaseEventType
): AssistantPhase {
  if (current === 'done' || current === 'error') {
    return current;
  }
  if (eventType === 'run_failed' || eventType === 'run_cancelled') {
    return 'error';
  }
  if (eventType === 'run_completed') {
    return 'done';
  }
  if (eventType === 'run_started' || eventType === 'model_started') {
    return current ?? 'thinking';
  }
  if (
    eventType === 'reasoning_delta' ||
    eventType === 'tool_started' ||
    eventType === 'tool_completed' ||
    eventType === 'approval_required' ||
    eventType === 'clarify_required'
  ) {
    return current === 'answering' ? 'answering' : 'thinking';
  }
  // 显式推理结束，或首条/后续正文增量 → 答案轨
  if (
    eventType === 'reasoning_completed' ||
    eventType === 'message_delta' ||
    eventType === 'message_completed' ||
    eventType === 'message_interim'
  ) {
    return 'answering';
  }
  return current ?? 'thinking';
}

/** 从有序事件类型列表归约最终阶段 */
export function deriveAssistantPhase(eventTypes: PhaseEventType[]): AssistantPhase {
  let phase: AssistantPhase | null = null;
  for (const t of eventTypes) {
    phase = reduceAssistantPhase(phase, t);
  }
  return phase ?? 'thinking';
}

export function deriveThinkingStatus(input: {
  phase: AssistantPhase;
  hasReasoning: boolean;
  hasTools: boolean;
}): ThinkingStatus {
  if (input.phase === 'thinking') return 'streaming';
  if (!input.hasReasoning && !input.hasTools) return 'none';
  return 'done';
}

export function deriveContentStatus(input: {
  phase: AssistantPhase;
  hasContent: boolean;
}): ContentStatus {
  switch (input.phase) {
    case 'thinking':
      return 'pending';
    case 'answering':
      return input.hasContent ? 'streaming' : 'pending';
    case 'done':
    case 'error':
      return 'done';
    default:
      return 'pending';
  }
}

/** Area A 是否展示：思考中始终占位；其后仅有真实过程内容时展示 */
export function shouldShowAreaA(input: {
  phase: AssistantPhase;
  hasReasoning: boolean;
  hasTools: boolean;
}): boolean {
  if (input.phase === 'thinking') return true;
  return input.hasReasoning || input.hasTools;
}

/** Area A 默认展开：仅 thinking 强制展开；其后默认收起（用户 sticky 另计） */
export function areaADefaultOpen(phase: AssistantPhase): boolean {
  return phase === 'thinking';
}

/** Area B：thinking 阶段不渲染正文（骨架）；其后可渲染 */
export function shouldRenderAreaBContent(input: {
  phase: AssistantPhase;
  hasContent: boolean;
}): boolean {
  if (input.phase === 'thinking') return false;
  return input.hasContent;
}

export function areaASummaryLabel(input: {
  phase: AssistantPhase;
  hasTools: boolean;
  durationMs?: number;
  formatDuration: (ms: number) => string;
}): string {
  if (input.phase === 'thinking') {
    return input.hasTools ? '执行过程 · 进行中' : '正在思考…';
  }
  if (input.phase === 'answering' && input.hasTools) {
    return '执行过程';
  }
  if (input.durationMs != null) {
    return `已思考 · ${input.formatDuration(input.durationMs)}`;
  }
  return input.hasTools ? '执行过程' : '思考过程';
}
