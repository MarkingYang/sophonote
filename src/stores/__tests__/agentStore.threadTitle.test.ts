/**
 * 会话标题：仅成功 run_completed 且有真实助手回复后才定名；
 * 发送中 / 失败不定名，避免「总结一下当前的这…」抢先占位。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

import {
  AGENT_EVENT_SCHEMA_VERSION,
  useAgentStore,
  type AgentEvent,
  type AgentEventPayload,
} from '../agentStore';

function ev(
  runId: string,
  seq: number,
  payload: AgentEventPayload,
  extra?: Partial<AgentEvent>
): AgentEvent {
  return {
    eventId: `${runId}:${seq}`,
    threadId: 't1',
    runId,
    seq,
    timestamp: 1000 + seq,
    schemaVersion: AGENT_EVENT_SCHEMA_VERSION,
    payload,
    ...extra,
  };
}

beforeEach(() => {
  useAgentStore.setState({
    threads: [
      {
        id: 't1',
        title: '新会话',
        status: 'running',
        projectId: 'p1',
        latestRunId: 'r1',
        createdAt: 1,
        updatedAt: 1,
        closedAt: null,
        archivedAt: null,
      },
    ],
    selectedThreadId: 't1',
    activeRuns: {},
    eventsByRunId: {},
    runIdsByThreadId: {},
    messagesByThreadId: {},
    toolCardsByThreadId: {},
    runningRunByThreadId: { t1: 'r1' },
    historyLoadingByThreadId: {},
    resumingRunByThreadId: {},
    resumeInFlight: {},
    recoveryInFlight: {},
    degraded: {},
    loading: false,
  });
});

describe('thread title timing', () => {
  it('run_started 不定名，保持「新会话」', () => {
    useAgentStore
      .getState()
      .handleEvent(
        ev('r1', 0, {
          type: 'run_started',
          userMessage: '总结一下当前的这篇文档',
          maxTurns: 6,
        })
      );
    expect(useAgentStore.getState().threads[0].title).toBe('新会话');
    expect(useAgentStore.getState().runningRunByThreadId.t1).toBe('r1');
  });

  it('run_failed 清 running 且不定名', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, {
        type: 'run_started',
        userMessage: '总结一下当前的这篇文档',
        maxTurns: 6,
      })
    );
    store.handleEvent(
      ev('r1', 1, {
        type: 'run_failed',
        outcome: 'failed',
        error: 'Hermes 引擎暂时不可用（502）',
      })
    );
    const state = useAgentStore.getState();
    expect(state.threads[0].title).toBe('新会话');
    expect(state.runningRunByThreadId.t1).toBeUndefined();
    expect(state.messagesByThreadId.t1.some((m) => m.content.startsWith('运行失败：'))).toBe(
      true
    );
  });

  it('run_completed 后用 Query · 回复摘要定名', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, {
        type: 'run_started',
        userMessage: '总结一下当前的这篇文档',
        maxTurns: 6,
      })
    );
    store.handleEvent(
      ev('r1', 1, {
        type: 'run_completed',
        outcome: 'completed',
        finalAnswer: '文档核心是三层架构。',
        modelCalls: 1,
      })
    );
    const title = useAgentStore.getState().threads[0].title;
    expect(title).toContain('总结一下当前的这篇文档');
    expect(title).toContain('文档核心是三层架构');
    expect(title).toContain('·');
    expect(useAgentStore.getState().runningRunByThreadId.t1).toBeUndefined();
  });
});
