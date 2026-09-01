import { describe, expect, it } from 'vitest';
import { classifyTodayTasks, isSameLocalDay, startOfLocalDay } from '../todayTasks';
import type { Task } from '../../types';

function task(patch: Partial<Task> & Pick<Task, 'id' | 'title'>): Task {
  return {
    status: 'todo',
    priority: 2,
    createdAt: '2026-08-01T00:00:00.000Z',
    ...patch,
  };
}

// 固定「现在」= 本地 2026-08-20 14:00
const NOW = new Date(2026, 7, 20, 14, 0, 0);

function localIso(y: number, monthIdx: number, day: number, h = 10): string {
  return new Date(y, monthIdx, day, h, 0, 0).toISOString();
}

describe('startOfLocalDay / isSameLocalDay', () => {
  it('strips the time part in local timezone', () => {
    const d = startOfLocalDay(new Date(2026, 7, 20, 23, 59, 59));
    expect(d.getHours()).toBe(0);
    expect(d.getDate()).toBe(20);
  });

  it('matches same local day regardless of time', () => {
    expect(isSameLocalDay(localIso(2026, 7, 20, 1), NOW)).toBe(true);
    expect(isSameLocalDay(localIso(2026, 7, 20, 23), NOW)).toBe(true);
    expect(isSameLocalDay(localIso(2026, 7, 19), NOW)).toBe(false);
    expect(isSameLocalDay(localIso(2026, 7, 21), NOW)).toBe(false);
  });

  it('rejects empty or invalid values', () => {
    expect(isSameLocalDay(undefined, NOW)).toBe(false);
    expect(isSameLocalDay('not-a-date', NOW)).toBe(false);
  });
});

describe('classifyTodayTasks', () => {
  it('splits overdue / dueToday / completedToday', () => {
    const tasks = [
      task({ id: 't1', title: '逾期', dueDate: localIso(2026, 7, 18), priority: 2 }),
      task({ id: 't2', title: '今天高优先', dueDate: localIso(2026, 7, 20), priority: 1 }),
      task({ id: 't3', title: '今天低优先', dueDate: localIso(2026, 7, 20), priority: 3 }),
      task({ id: 't4', title: '明天', dueDate: localIso(2026, 7, 21) }),
      task({ id: 't5', title: '今天完成', status: 'done', completedAt: localIso(2026, 7, 20, 9) }),
      task({ id: 't6', title: '昨天完成', status: 'done', completedAt: localIso(2026, 7, 19) }),
      task({ id: 't7', title: '无日期' }),
      task({ id: 't8', title: '已取消今天', status: 'cancelled', dueDate: localIso(2026, 7, 20) }),
    ];

    const view = classifyTodayTasks(tasks, NOW);
    expect(view.overdue.map((t) => t.id)).toEqual(['t1']);
    expect(view.dueToday.map((t) => t.id)).toEqual(['t2', 't3']);
    expect(view.completedToday.map((t) => t.id)).toEqual(['t5']);
  });

  it('sorts dueToday by priority then due date', () => {
    const tasks = [
      task({ id: 'b', title: 'b', dueDate: localIso(2026, 7, 20), priority: 3 }),
      task({ id: 'a', title: 'a', dueDate: localIso(2026, 7, 20), priority: 1 }),
      task({ id: 'c', title: 'c', dueDate: localIso(2026, 7, 20), priority: 1 }),
    ];
    const view = classifyTodayTasks(tasks, NOW);
    expect(view.dueToday.map((t) => t.id)).toEqual(['a', 'c', 'b']);
  });

  it('ignores invalid dueDate values', () => {
    const tasks = [task({ id: 'x', title: 'x', dueDate: 'garbage' })];
    const view = classifyTodayTasks(tasks, NOW);
    expect(view.overdue).toEqual([]);
    expect(view.dueToday).toEqual([]);
  });
});
