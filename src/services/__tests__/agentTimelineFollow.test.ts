import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '../../stores/agentStore';
import {
  isTimelineNearBottom,
  latestTimelineStart,
  previousTimelineStart,
  threadEventRevision,
} from '../agentTimelineFollow';

function event(runId: string, seq: number): AgentEvent {
  return {
    eventId: `${runId}-${seq}`,
    threadId: runId === 'run-a' ? 'thread-a' : 'thread-b',
    runId,
    seq,
    timestamp: seq,
    schemaVersion: 2,
    payload: { type: 'message_delta', delta: String(seq) },
  };
}

describe('timeline auto follow', () => {
  it('mounts only the latest long-history window', () => {
    expect(latestTimelineStart(240)).toBe(224);
    expect(latestTimelineStart(10)).toBe(0);
    expect(latestTimelineStart(240, 80)).toBe(160);
    expect(latestTimelineStart(40, 80)).toBe(0);
  });

  it('prepends history in bounded pages', () => {
    expect(previousTimelineStart(160, 60)).toBe(100);
    expect(previousTimelineStart(30, 60)).toBe(0);
  });

  it('keeps following while the reader is near the bottom', () => {
    expect(isTimelineNearBottom({ scrollHeight: 1000, scrollTop: 820, clientHeight: 100 })).toBe(true);
  });

  it('stops following after the reader scrolls into history', () => {
    expect(isTimelineNearBottom({ scrollHeight: 1000, scrollTop: 600, clientHeight: 100 })).toBe(false);
  });

  it('ignores events from a background thread', () => {
    const before = { 'run-a': [event('run-a', 0)], 'run-b': [event('run-b', 0)] };
    const after = { ...before, 'run-b': [...before['run-b'], event('run-b', 1)] };
    expect(threadEventRevision(['run-a'], after)).toBe(threadEventRevision(['run-a'], before));
  });

  it('changes when the visible thread receives a new event', () => {
    const before = { 'run-a': [event('run-a', 0)] };
    const after = { 'run-a': [...before['run-a'], event('run-a', 1)] };
    expect(threadEventRevision(['run-a'], after)).not.toBe(threadEventRevision(['run-a'], before));
  });
});
