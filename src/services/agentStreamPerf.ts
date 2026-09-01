import type { AgentEvent } from '../stores/agentStore';
import { perfMark } from './notePerf';

export interface AgentStreamPerfSummary {
  runId: string;
  threadId: string;
  eventCount: number;
  batchCount: number;
  textChars: number;
  firstReasoningMs: number | null;
  firstAnswerMs: number | null;
  maxArrivalGapMs: number;
  averageStoreReduceMs: number;
  maxStoreReduceMs: number;
  averageRenderCommitMs: number;
  maxRenderCommitMs: number;
  terminal: boolean;
}

interface MutableRunPerf {
  runId: string;
  threadId: string;
  runStartedTimestamp: number | null;
  firstReasoningTimestamp: number | null;
  firstAnswerTimestamp: number | null;
  eventCount: number;
  batchCount: number;
  textChars: number;
  lastArrivalAt: number | null;
  maxArrivalGapMs: number;
  storeReduceTotalMs: number;
  maxStoreReduceMs: number;
  renderCommitTotalMs: number;
  renderCommitCount: number;
  maxRenderCommitMs: number;
  pendingRenderAt: number | null;
  lastPerfMarkAt: number;
  terminal: boolean;
}

const runs = new Map<string, MutableRunPerf>();
const PERF_MARK_INTERVAL_MS = 1_000;
const MAX_TRACKED_RUNS = 200;

function eventTextLength(event: AgentEvent): number {
  const payload = event.payload;
  if (
    payload.type === 'message_delta' ||
    payload.type === 'reasoning_delta' ||
    payload.type === 'message_completed' ||
    payload.type === 'message_interim'
  ) return payload.text.length;
  return 0;
}

function isTerminal(event: AgentEvent): boolean {
  return event.payload.type === 'run_completed' ||
    event.payload.type === 'run_failed' ||
    event.payload.type === 'run_cancelled';
}

function ensureRun(event: AgentEvent): MutableRunPerf {
  const existing = runs.get(event.runId);
  if (existing) return existing;
  if (runs.size >= MAX_TRACKED_RUNS) {
    const oldestRunId = runs.keys().next().value as string | undefined;
    if (oldestRunId) runs.delete(oldestRunId);
  }
  const created: MutableRunPerf = {
    runId: event.runId,
    threadId: event.threadId,
    runStartedTimestamp: null,
    firstReasoningTimestamp: null,
    firstAnswerTimestamp: null,
    eventCount: 0,
    batchCount: 0,
    textChars: 0,
    lastArrivalAt: null,
    maxArrivalGapMs: 0,
    storeReduceTotalMs: 0,
    maxStoreReduceMs: 0,
    renderCommitTotalMs: 0,
    renderCommitCount: 0,
    maxRenderCommitMs: 0,
    pendingRenderAt: null,
    lastPerfMarkAt: 0,
    terminal: false,
  };
  runs.set(event.runId, created);
  return created;
}

/**
 * 记录一次 Zustand 同步归约。只按批次打点，且每个 Run 至多每秒写一次性能环，
 * 避免诊断本身重新制造逐 token 抖动。
 */
export function recordAgentStoreBatch(
  events: readonly AgentEvent[],
  reduceMs: number,
  now = performance.now(),
): void {
  if (events.length === 0) return;
  const touched = new Map<string, MutableRunPerf>();
  for (const event of events) {
    const run = ensureRun(event);
    touched.set(run.runId, run);
    run.eventCount += 1;
    run.textChars += eventTextLength(event);
    if (event.payload.type === 'run_started') run.runStartedTimestamp = event.timestamp;
    if (event.payload.type === 'reasoning_delta' && run.firstReasoningTimestamp == null) {
      run.firstReasoningTimestamp = event.timestamp;
      if (run.runStartedTimestamp != null) {
        perfMark('Chat · 首推理', Math.max(0, event.timestamp - run.runStartedTimestamp));
      }
    }
    if (
      (event.payload.type === 'message_delta' || event.payload.type === 'message_completed') &&
      run.firstAnswerTimestamp == null
    ) {
      run.firstAnswerTimestamp = event.timestamp;
      if (run.runStartedTimestamp != null) {
        perfMark('Chat · 首答案', Math.max(0, event.timestamp - run.runStartedTimestamp));
      }
    }
    if (isTerminal(event)) run.terminal = true;
  }

  for (const run of touched.values()) {
    run.batchCount += 1;
    run.storeReduceTotalMs += reduceMs;
    run.maxStoreReduceMs = Math.max(run.maxStoreReduceMs, reduceMs);
    if (run.lastArrivalAt != null) {
      run.maxArrivalGapMs = Math.max(run.maxArrivalGapMs, now - run.lastArrivalAt);
    }
    run.lastArrivalAt = now;
    run.pendingRenderAt = now;

    if (run.terminal || now - run.lastPerfMarkAt >= PERF_MARK_INTERVAL_MS) {
      perfMark('Chat · Store 批归约', reduceMs);
      run.lastPerfMarkAt = now;
    }
  }
}

/** 当前可见 Thread 完成一次 React layout commit 后记录 Store→UI 延迟。 */
export function recordAgentRenderCommit(
  threadId: string,
  now = performance.now(),
): void {
  for (const run of runs.values()) {
    if (run.threadId !== threadId || run.pendingRenderAt == null) continue;
    const commitMs = Math.max(0, now - run.pendingRenderAt);
    run.pendingRenderAt = null;
    run.renderCommitTotalMs += commitMs;
    run.renderCommitCount += 1;
    run.maxRenderCommitMs = Math.max(run.maxRenderCommitMs, commitMs);
    if (run.terminal || now - run.lastPerfMarkAt >= PERF_MARK_INTERVAL_MS) {
      perfMark('Chat · React 提交', commitMs);
      run.lastPerfMarkAt = now;
    }
  }
}

export function agentStreamPerfSummary(runId: string): AgentStreamPerfSummary | null {
  const run = runs.get(runId);
  if (!run) return null;
  const sinceStart = (timestamp: number | null) =>
    timestamp != null && run.runStartedTimestamp != null
      ? Math.max(0, timestamp - run.runStartedTimestamp)
      : null;
  return {
    runId: run.runId,
    threadId: run.threadId,
    eventCount: run.eventCount,
    batchCount: run.batchCount,
    textChars: run.textChars,
    firstReasoningMs: sinceStart(run.firstReasoningTimestamp),
    firstAnswerMs: sinceStart(run.firstAnswerTimestamp),
    maxArrivalGapMs: run.maxArrivalGapMs,
    averageStoreReduceMs: run.batchCount > 0 ? run.storeReduceTotalMs / run.batchCount : 0,
    maxStoreReduceMs: run.maxStoreReduceMs,
    averageRenderCommitMs: run.renderCommitCount > 0
      ? run.renderCommitTotalMs / run.renderCommitCount
      : 0,
    maxRenderCommitMs: run.maxRenderCommitMs,
    terminal: run.terminal,
  };
}

export function resetAgentStreamPerfForTests(): void {
  runs.clear();
}
