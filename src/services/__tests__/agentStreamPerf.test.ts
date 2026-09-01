import { beforeEach, describe, expect, it } from 'vitest';
import type { AgentEvent } from '../../stores/agentStore';
import {
  agentStreamPerfSummary,
  recordAgentRenderCommit,
  recordAgentStoreBatch,
  resetAgentStreamPerfForTests,
} from '../agentStreamPerf';

function event(seq: number, timestamp: number, payload: AgentEvent['payload']): AgentEvent {
  return {
    eventId: `event-${seq}`,
    threadId: 'thread-1',
    runId: 'run-1',
    seq,
    timestamp,
    schemaVersion: 4,
    payload,
  };
}

describe('agent stream performance diagnostics', () => {
  beforeEach(resetAgentStreamPerfForTests);

  it('separates upstream first-token timing from local reduce and render cost', () => {
    recordAgentStoreBatch([
      event(0, 1_000, { type: 'run_started', userMessage: 'hello', maxTurns: 10 }),
    ], 2, 10);
    recordAgentStoreBatch([
      event(1, 1_240, { type: 'reasoning_delta', text: 'think' }),
      event(2, 1_700, { type: 'message_delta', text: 'answer' }),
    ], 4, 30);
    recordAgentRenderCommit('thread-1', 36);

    expect(agentStreamPerfSummary('run-1')).toMatchObject({
      eventCount: 3,
      batchCount: 2,
      textChars: 11,
      firstReasoningMs: 240,
      firstAnswerMs: 700,
      maxArrivalGapMs: 20,
      averageStoreReduceMs: 3,
      maxStoreReduceMs: 4,
      averageRenderCommitMs: 6,
      maxRenderCommitMs: 6,
      terminal: false,
    });
  });

  it('marks a terminal batch without requiring another render event', () => {
    recordAgentStoreBatch([
      event(0, 1_000, { type: 'run_started', userMessage: 'hello', maxTurns: 10 }),
      event(1, 1_010, { type: 'run_completed', modelCalls: 1 }),
    ], 1, 10);
    expect(agentStreamPerfSummary('run-1')?.terminal).toBe(true);
  });
});
