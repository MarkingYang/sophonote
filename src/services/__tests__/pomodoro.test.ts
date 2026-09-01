import { describe, expect, it } from 'vitest';
import {
  BREAK_MINUTES,
  FOCUS_MINUTES,
  createPomodoroSession,
  focusMinutesOf,
  formatClock,
  summarizeFocus,
} from '../pomodoro';
import type { PomodoroSession } from '../../types';

const NOW = new Date(2026, 7, 20, 14, 0, 0);

function session(patch: Partial<PomodoroSession> & Pick<PomodoroSession, 'id'>): PomodoroSession {
  return {
    plannedMinutes: FOCUS_MINUTES,
    startedAt: new Date(2026, 7, 20, 9, 0, 0).toISOString(),
    endedAt: new Date(2026, 7, 20, 9, 25, 0).toISOString(),
    completed: true,
    ...patch,
  };
}

describe('defaults', () => {
  it('uses 25/5', () => {
    expect(FOCUS_MINUTES).toBe(25);
    expect(BREAK_MINUTES).toBe(5);
  });
});

describe('createPomodoroSession', () => {
  it('serializes dates to ISO and keeps task link', () => {
    const started = new Date(2026, 7, 20, 9, 0, 0);
    const ended = new Date(2026, 7, 20, 9, 25, 0);
    const s = createPomodoroSession({
      id: 'p1',
      taskId: 't9',
      plannedMinutes: FOCUS_MINUTES,
      startedAt: started,
      endedAt: ended,
      completed: true,
    });
    expect(s.id).toBe('p1');
    expect(s.taskId).toBe('t9');
    expect(s.startedAt).toBe(started.toISOString());
    expect(s.endedAt).toBe(ended.toISOString());
    expect(s.completed).toBe(true);
  });
});

describe('focusMinutesOf', () => {
  it('completed session counts planned minutes', () => {
    expect(focusMinutesOf(session({ completed: true, plannedMinutes: 25 }))).toBe(25);
  });

  it('abandoned session counts elapsed whole minutes capped by plan', () => {
    const s = session({
      completed: false,
      startedAt: new Date(2026, 7, 20, 9, 0, 0).toISOString(),
      endedAt: new Date(2026, 7, 20, 9, 10, 30).toISOString(),
    });
    expect(focusMinutesOf(s)).toBe(10);
  });

  it('does not exceed planned minutes for abandoned sessions', () => {
    const s = session({
      completed: false,
      startedAt: new Date(2026, 7, 20, 9, 0, 0).toISOString(),
      endedAt: new Date(2026, 7, 20, 10, 0, 0).toISOString(),
    });
    expect(focusMinutesOf(s)).toBe(25);
  });

  it('returns 0 for missing or inverted times', () => {
    expect(focusMinutesOf(session({ completed: false, endedAt: undefined }))).toBe(0);
    const inverted = session({
      completed: false,
      startedAt: new Date(2026, 7, 20, 9, 30, 0).toISOString(),
      endedAt: new Date(2026, 7, 20, 9, 0, 0).toISOString(),
    });
    expect(focusMinutesOf(inverted)).toBe(0);
  });
});

describe('summarizeFocus', () => {
  it('aggregates only sessions started the same local day', () => {
    const sessions = [
      session({ id: 'today-done', completed: true, plannedMinutes: 25 }),
      session({
        id: 'today-partial',
        completed: false,
        startedAt: new Date(2026, 7, 20, 11, 0, 0).toISOString(),
        endedAt: new Date(2026, 7, 20, 11, 5, 0).toISOString(),
      }),
      session({
        id: 'yesterday',
        completed: true,
        startedAt: new Date(2026, 7, 19, 9, 0, 0).toISOString(),
      }),
    ];
    const summary = summarizeFocus(sessions, NOW);
    expect(summary.minutes).toBe(30); // 25 + 5
    expect(summary.completedCount).toBe(1);
  });

  it('handles empty list', () => {
    expect(summarizeFocus([], NOW)).toEqual({ minutes: 0, completedCount: 0 });
  });
});

describe('formatClock', () => {
  it('formats mm:ss with padding', () => {
    expect(formatClock(1500)).toBe('25:00');
    expect(formatClock(65)).toBe('01:05');
    expect(formatClock(0)).toBe('00:00');
  });

  it('clamps invalid input to zero', () => {
    expect(formatClock(-5)).toBe('00:00');
    expect(formatClock(Number.NaN)).toBe('00:00');
    expect(formatClock(Number.POSITIVE_INFINITY)).toBe('00:00');
  });
});
