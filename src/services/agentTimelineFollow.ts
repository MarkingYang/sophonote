import type { AgentEvent } from '../stores/agentStore';

export const TIMELINE_BOTTOM_THRESHOLD_PX = 96;
export const TIMELINE_INITIAL_WINDOW = 16;
export const TIMELINE_PREPEND_PAGE = 60;

export interface TimelineScrollMetrics {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

/** 用户仍在底部附近时才继续跟随流式输出。 */
export function isTimelineNearBottom(
  metrics: TimelineScrollMetrics,
  threshold = TIMELINE_BOTTOM_THRESHOLD_PX,
): boolean {
  return metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight <= threshold;
}

/** 长会话初始只挂载最近一窗。过大时会话页二次挂载会把 MutationObserver settle 拉过 300ms。 */
export function latestTimelineStart(
  length: number,
  windowSize = TIMELINE_INITIAL_WINDOW,
): number {
  return Math.max(0, length - Math.max(1, windowSize));
}

/** 触顶后向前扩一页；返回稳定且不小于 0 的新起点。 */
export function previousTimelineStart(
  currentStart: number,
  pageSize = TIMELINE_PREPEND_PAGE,
): number {
  return Math.max(0, currentStart - Math.max(1, pageSize));
}

/**
 * 只为当前 Thread 的 Run 生成变更指纹。后台会话事件增加时，该值保持不变，
 * 因而不会触发当前时间线贴底。
 */
export function threadEventRevision(
  runIds: readonly string[],
  eventsByRunId: Record<string, AgentEvent[]>,
): string {
  return runIds.map((runId) => {
    const events = eventsByRunId[runId] ?? [];
    const last = events[events.length - 1];
    return `${runId}:${events.length}:${last?.seq ?? -1}`;
  }).join('|');
}
