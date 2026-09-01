/**
 * AG-27：激活 Skill 引用（RunSkillRef）前端链路单测。
 * 覆盖：① run_started 携带 skill → 用户消息带技能引用（「Run 可见版本与来源」数据源）；
 * ② 旧事件无 skill 键 → null（向后兼容，AG-21/AG-26 增量字段先例）；
 * ③ startRun 第五参 → agent_run_start 请求体 request.skill 原样透传，
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
import { useAgentStore, type AgentEvent, type AgentEventPayload, type RunSkillRef } from '../agentStore';

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

const skillRef: RunSkillRef = { name: 'research-note', version: 1, source: 'bundled' };

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

describe('AG-27 消息归约：激活 Skill 引用', () => {
  it('run_started 携带 skill → 用户消息带技能引用（版本与来源可见）', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(
      ev('r1', 0, {
        type: 'run_started',
        userMessage: '整理一份研究笔记',
        maxTurns: 5,
        skill: skillRef,
      })
    );
    handleEvent(ev('r1', 1, { type: 'run_completed', outcome: 'completed', finalAnswer: '完成', modelCalls: 1 }));

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[0].role).toBe('user');
    expect(msgs[0].skill).toEqual(skillRef);
    // assistant 消息不携带技能引用
    expect(msgs[1].skill ?? null).toBeNull();
  });

  it('旧事件无 skill 键 → 用户消息技能引用为空（向后兼容）', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '你好', maxTurns: 3 }));
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[0].skill ?? null).toBeNull();
  });
});

describe('AG-27 startRun：skill 透传', () => {
  it('提供 skill → agent_run_start 请求体 request.skill 原样传递', async () => {
    invokeMock.mockResolvedValue({ success: true, data: { threadId: 't1', runId: 'r1' }, error: null });
    const res = await useAgentStore.getState().startRun(null, '整理研究笔记', 'p1', null, 'research-note');
    expect(res).not.toBeNull();

    const [cmd, args] = invokeMock.mock.calls[0];
    expect(cmd).toBe('agent_run_start');
    const request = (args as { request: { skill?: unknown } }).request;
    expect(request.skill).toBe('research-note');
  });

  it('未提供 skill → request.skill = null（Rust serde(default) None）', async () => {
    invokeMock.mockResolvedValue({ success: true, data: { threadId: 't1', runId: 'r1' }, error: null });
    await useAgentStore.getState().startRun(null, '你好', 'p1');
    const [, args] = invokeMock.mock.calls[0];
    const request = (args as { request: { skill?: unknown } }).request;
    expect(request.skill).toBeNull();
  });
});
