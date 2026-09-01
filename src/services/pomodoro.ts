import type { PomodoroSession } from '../types';
import { isSameLocalDay } from './todayTasks';

/**
 * 番茄钟纯逻辑（DEC-034）：计时参数、会话构造、专注统计。
 * 组件只持有 UI 状态，所有可测判断都在这里。
 */

export const FOCUS_MINUTES = 25;
export const BREAK_MINUTES = 5;

export function createPomodoroSession(opts: {
  id: string;
  taskId?: string;
  plannedMinutes: number;
  startedAt: Date;
  endedAt: Date;
  completed: boolean;
}): PomodoroSession {
  return {
    id: opts.id,
    taskId: opts.taskId,
    plannedMinutes: opts.plannedMinutes,
    startedAt: opts.startedAt.toISOString(),
    endedAt: opts.endedAt.toISOString(),
    completed: opts.completed,
  };
}

/**
 * 单条会话贡献的专注分钟数：
 * 自然完成 = 计划时长；中途放弃 = 实际经过的整分钟（不超过计划时长）。
 */
export function focusMinutesOf(session: PomodoroSession): number {
  if (session.completed) return session.plannedMinutes;
  if (!session.endedAt) return 0;
  const start = new Date(session.startedAt).getTime();
  const end = new Date(session.endedAt).getTime();
  if (Number.isNaN(start) || Number.isNaN(end) || end <= start) return 0;
  const actual = Math.floor((end - start) / 60000);
  return Math.max(0, Math.min(actual, session.plannedMinutes));
}

export interface FocusSummary {
  /** 今日累计专注分钟 */
  minutes: number;
  /** 今日完成的完整番茄数 */
  completedCount: number;
}

/** 统计某一天（默认今天）开始的所有专注会话。 */
export function summarizeFocus(sessions: PomodoroSession[], now: Date): FocusSummary {
  let minutes = 0;
  let completedCount = 0;
  for (const session of sessions) {
    if (!isSameLocalDay(session.startedAt, now)) continue;
    minutes += focusMinutesOf(session);
    if (session.completed) completedCount += 1;
  }
  return { minutes, completedCount };
}

/** mm:ss 展示，负数/非有限值归零。 */
export function formatClock(totalSeconds: number): string {
  const safe = Number.isFinite(totalSeconds) && totalSeconds > 0 ? Math.floor(totalSeconds) : 0;
  const mm = Math.floor(safe / 60);
  const ss = safe % 60;
  return `${String(mm).padStart(2, '0')}:${String(ss).padStart(2, '0')}`;
}
