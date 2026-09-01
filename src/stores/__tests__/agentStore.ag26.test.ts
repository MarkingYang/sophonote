/**
 * AG-26：选区上下文（RunContext）前端链路单测。
 * 覆盖：① run_started 携带 context → 用户消息带上下文（验收场景② 数据源）；
 * ② 旧事件无 context 键 → undefined（向后兼容，AG-21 增量字段先例）；
 * ③ startRun 第四参 → agent_run_start 请求体 request.selection 原样透传，
 *    未提供 → null（Rust serde(default) None）。
 * 全部 mock `@tauri-apps/api/core`，零真实模型调用。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

/** vi.mock 工厂被提升到文件顶部——实例收集器必须先于工厂存在 */
const { channelInstances } = vi.hoisted(() => ({
  channelInstances: [] as Array<{ onmessage: ((event: unknown) => void) | null }>,
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
    constructor() {
      channelInstances.push(this);
    }
  },
}));

import { invoke } from '@tauri-apps/api/core';
import { useAgentStore, type AgentEvent, type AgentEventPayload, type RunContext } from '../agentStore';

const invokeMock = vi.mocked(invoke);

function ev(
  runId: string,
  seq: number,
  payload: AgentEventPayload,
  over: Partial<AgentEvent> = {}
): AgentEvent {
  return {
    eventId: `${runId}:${seq}`,
    threadId: 't1',
    runId,
    seq,
    timestamp: 1000 + seq,
    schemaVersion: 1,
    payload,
    ...over,
  };
}

const selection: RunContext = {
  articleId: 'doc-1',
  title: '压缩测试',
  baseVersion: 3,
  selectedMarkdown: '这是一个很长的段落，需要压缩成三句话。',
  selectedTextHash: 'h-abc',
  beforeContext: '前文',
  afterContext: '后文',
};

function resetStore() {
  useAgentStore.setState({
    threads: [],
    selectedThreadId: null,
    activeRuns: {},
    eventsByRunId: {},
    runIdsByThreadId: {},
    messagesByThreadId: {},
    toolCardsByThreadId: {},
    runningRunByThreadId: {},
    historyLoadingByThreadId: {},
    resumingRunByThreadId: {},
    resumeInFlight: {},
    recoveryInFlight: {},
    degraded: {},
    loading: false,
  });
}

beforeEach(() => {
  resetStore();
  invokeMock.mockReset();
  channelInstances.length = 0;
});

describe('AG-26 消息归约：选区上下文', () => {
  it('run_started 携带 context → 用户消息带上下文（chip 数据源）', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(
      ev('r1', 0, {
        type: 'run_started',
        userMessage: '压缩成三句话并保留数字',
        maxTurns: 3,
        context: selection,
      })
    );
    handleEvent(ev('r1', 1, { type: 'run_completed', outcome: 'completed', finalAnswer: '完成', modelCalls: 1 }));

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[0].role).toBe('user');
    expect(msgs[0].context).toEqual(selection);
    // assistant 消息不携带选区上下文
    expect(msgs[1].context ?? null).toBeNull();
  });

  it('旧事件无 context 键 → 用户消息上下文为空（向后兼容）', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '你好', maxTurns: 3 }));
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[0].context ?? null).toBeNull();
  });
});

describe('AG-26 startRun：selection 透传', () => {
  it('提供 selection → agent_run_start 请求体 request.selection 原样传递', async () => {
    invokeMock.mockResolvedValue({ success: true, data: { threadId: 't1', runId: 'r1' }, error: null });
    const res = await useAgentStore.getState().startRun(null, '压缩', 'p1', selection);
    expect(res).not.toBeNull();

    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe('agent_run_start');
    const request = (args as { request: { selection?: unknown } }).request;
    expect(request.selection).toEqual(selection);
  });

  it('未提供 selection → request.selection = null（Rust serde(default) None）', async () => {
    invokeMock.mockResolvedValue({ success: true, data: { threadId: 't1', runId: 'r1' }, error: null });
    await useAgentStore.getState().startRun(null, '你好', 'p1');
    const [, args] = invokeMock.mock.calls[0];
    const request = (args as { request: { selection?: unknown } }).request;
    expect(request.selection).toBeNull();
  });
});
