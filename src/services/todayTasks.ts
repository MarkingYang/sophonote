import type { Task } from '../types';

/**
 * 今日视图分类（DEC-034 / NEXT-063）。
 * 全部按本地时区判断「同一天」，与驾驶舱展示的「今天」保持一致。
 * dueDate 非法或为空的任务不参与到期/逾期分类。
 */

export interface TodayView {
  /** 到期日早于今天且未完成 */
  overdue: Task[];
  /** 到期日为今天且未完成 */
  dueToday: Task[];
  /** 今天完成（completedAt 为今天） */
  completedToday: Task[];
}

export function startOfLocalDay(d: Date): Date {
  const copy = new Date(d);
  copy.setHours(0, 0, 0, 0);
  return copy;
}

export function isSameLocalDay(iso: string | undefined, now: Date): boolean {
  if (!iso) return false;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return false;
  return startOfLocalDay(d).getTime() === startOfLocalDay(now).getTime();
}

function isActive(task: Task): boolean {
  return task.status !== 'done' && task.status !== 'cancelled';
}

/** 按优先级（1 高 → 3 低）再按到期日升序；无到期日沉底。 */
function byPriorityThenDue(a: Task, b: Task): number {
  if (a.priority !== b.priority) return a.priority - b.priority;
  const ad = a.dueDate ?? '';
  const bd = b.dueDate ?? '';
  if (ad === bd) return 0;
  if (ad === '') return 1;
  if (bd === '') return -1;
  return ad < bd ? -1 : 1;
}

export function classifyTodayTasks(tasks: Task[], now: Date): TodayView {
  const dayStart = startOfLocalDay(now);

  const overdue: Task[] = [];
  const dueToday: Task[] = [];
  const completedToday: Task[] = [];

  for (const task of tasks) {
    if (task.status === 'done') {
      if (isSameLocalDay(task.completedAt, now)) completedToday.push(task);
      continue;
    }
    if (!isActive(task) || !task.dueDate) continue;

    const due = new Date(task.dueDate);
    if (Number.isNaN(due.getTime())) continue;

    if (startOfLocalDay(due).getTime() < dayStart.getTime()) {
      overdue.push(task);
    } else if (startOfLocalDay(due).getTime() === dayStart.getTime()) {
      dueToday.push(task);
    }
  }

  overdue.sort(byPriorityThenDue);
  dueToday.sort(byPriorityThenDue);
  return { overdue, dueToday, completedToday };
}
