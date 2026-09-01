/**
 * AG-23：agentStore 消息归约（reducer）与 Run 生命周期单测（审计 P1-3：
 * 「把 reducer 和纯函数先纳入测试」）。
 * 覆盖：reduceMessages 四路归约（user/assistant/失败可见/取消可见）、
 * 跨 Run 多轮并存（AG-17 回归：第二轮不得覆盖第一轮）、乱序到达按 seq 归约、
 * startRun/cancelRun 命令路径与 runningRunByThreadId 生命周期（AG-18：
 * 注销只认终态事件，停止信号本身不清状态）、loadThreads 成功/失败、派生缺省。
 * 全部 mock `@tauri-apps/api/core`，零真实模型调用、零 Tauri 依赖。
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
import { useAgentStore, type AgentEvent, type AgentEventPayload } from '../agentStore';

const invokeMock = vi.mocked(invoke);

/** 事件工厂（信封六要素齐全，默认 thread=t1 / schemaVersion=1） */
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

const runStarted = (msg = '你好'): AgentEventPayload => ({
  type: 'run_started',
  userMessage: msg,
  maxTurns: 3,
});
const runCompleted = (answer = '完成'): AgentEventPayload => ({
  type: 'run_completed',
  outcome: 'completed',
  finalAnswer: answer,
  modelCalls: 1,
});
const runFailed = (error = '模型超时'): AgentEventPayload => ({
  type: 'run_failed',
  outcome: 'failed',
  error,
});
const runCancelled = (reason = ''): AgentEventPayload => ({
  type: 'run_cancelled',
  reason,
});
const modelStarted: AgentEventPayload = { type: 'model_started', turn: 1 };

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

// ------------------- 消息归约（reduceMessages） -------------------

describe('AG-23 消息归约 reducer', () => {
  it('run_started/run_completed 归约出 user + assistant，顺序与内容正确', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('今天读什么')));
    handleEvent(ev('r1', 1, modelStarted)); // 中间事件不产生消息
    handleEvent(ev('r1', 2, runCompleted('推荐《设计心理学》')));

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => `${m.role}:${m.content}`)).toEqual([
      'user:今天读什么',
      'assistant:推荐《设计心理学》',
    ]);
    // 消息 id 稳定可作 React key（runId + 角色 + seq）
    expect(msgs[0].id).toBe('r1:user:0');
    expect(msgs[1].id).toBe('r1:assistant:2');
    expect(msgs[0].createdAt).toBeLessThan(msgs[1].createdAt);
    expect(useAgentStore.getState().degradedOfThread('t1')).toBeNull();
  });

  it('run_failed 失败可见：用户消息发出后不得石沉大海', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('查资料')));
    handleEvent(ev('r1', 1, runFailed('上游 429 限流')));

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs).toHaveLength(2);
    expect(msgs[1].role).toBe('assistant');
    expect(msgs[1].content).toBe('运行失败：上游 429 限流');
  });

  it('run_cancelled 取消可见：带原因展示原因，空原因展示通用文案', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('长任务')));
    handleEvent(ev('r1', 1, runCancelled('用户停止')));
    handleEvent(ev('r2', 0, runStarted('另一个'), { threadId: 't2' }));
    handleEvent(ev('r2', 1, runCancelled(''), { threadId: 't2' }));

    expect(useAgentStore.getState().messagesByThreadId['t1'][1].content).toBe(
      '运行已取消：用户停止'
    );
    expect(useAgentStore.getState().messagesByThreadId['t2'][1].content).toBe('运行已取消');
  });

  it('多轮并存（AG-17 回归）：第二轮事件不得覆盖第一轮消息', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('第一轮问题')));
    handleEvent(ev('r1', 1, runCompleted('第一轮回答')));
    handleEvent(ev('r2', 0, runStarted('第二轮问题')));
    handleEvent(ev('r2', 1, runCompleted('第二轮回答')));

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => `${m.role}:${m.content}`)).toEqual([
      'user:第一轮问题',
      'assistant:第一轮回答',
      'user:第二轮问题',
      'assistant:第二轮回答',
    ]);
    // Run 到达顺序登记完整
    expect(useAgentStore.getState().runIdsByThreadId['t1']).toEqual(['r1', 'r2']);
  });

  it('乱序到达：按 seq 排序归约，终态先到也不会错位', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 2, runCompleted('答'))); // 终态先到
    handleEvent(ev('r1', 1, modelStarted));
    handleEvent(ev('r1', 0, runStarted('问'))); // 起始后到

    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => `${m.role}:${m.content}`)).toEqual(['user:问', 'assistant:答']);
  });

  it('Thread 隔离：t2 的事件不影响 t1 的消息', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('t1 的问题')));
    handleEvent(ev('r2', 0, runStarted('t2 的问题'), { threadId: 't2' }));

    const t1 = useAgentStore.getState().messagesByThreadId['t1'];
    const t2 = useAgentStore.getState().messagesByThreadId['t2'];
    // run_started 会立即挂载本 Thread 的思考骨架；两个空助手气泡
    // 仍必须各自绑定所属 Thread，不得串话。
    expect(t1.map((m) => `${m.threadId}:${m.role}:${m.content}`)).toEqual([
      't1:user:t1 的问题',
      't1:assistant:',
    ]);
    expect(t2.map((m) => `${m.threadId}:${m.role}:${m.content}`)).toEqual([
      't2:user:t2 的问题',
      't2:assistant:',
    ]);
  });
});

// ------------------- Run 生命周期（startRun / cancelRun / AG-18） -------------------

describe('AG-23 startRun/cancelRun 生命周期', () => {
  it('同一 Thread 正在恢复或上一轮非终态时，状态层拒绝启动下一轮', async () => {
    useAgentStore.setState({ historyLoadingByThreadId: { t1: 1 } });
    expect(await useAgentStore.getState().startRun('t1', '下一问', 'p1')).toBeNull();

    useAgentStore.setState({
      historyLoadingByThreadId: {},
      runningRunByThreadId: { t1: 'r-old' },
    });
    expect(await useAgentStore.getState().startRun('t1', '下一问', 'p1')).toBeNull();

    useAgentStore.setState({
      runningRunByThreadId: {},
      resumingRunByThreadId: { t1: 'r-old' },
    });
    expect(await useAgentStore.getState().startRun('t1', '下一问', 'p1')).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it('startRun 成功：返回 id、登记进行中 Run、Channel 推送进入归约', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_start') {
        return { success: true, data: { threadId: 't1', runId: 'r1' }, error: null };
      }
      if (cmd === 'agent_thread_list') {
        return { success: true, data: [], error: null };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    const res = await useAgentStore.getState().startRun(null, '你好', 'p1');
    expect(res).toEqual({ threadId: 't1', runId: 'r1' });
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBe('r1');
    expect(invokeMock).toHaveBeenCalledWith('agent_run_start', {
      request: {
        message: '你好',
        threadId: null,
        projectId: 'p1',
        provider: null,
        system: null,
        maxTurns: null,
        // AG-26：选区上下文增量字段（未提供 = null → Rust serde(default) None）
        selection: null,
        // AG-27：激活 Skill 增量字段（未提供 = null → Rust serde(default) None）
        skill: null,
        focusDocument: null,
        attachments: [],
        hermesModel: null,
        hermesProvider: null,
        hermesCommand: null,
        // 左侧「将项目加入会话」开关（未提供 = false → Rust serde(default) false）
        includeProjectContext: false,
        workspaceRoot: null,
        workspacePermissionMode: 'ask',
      },
      onEvent: expect.anything(),
    });

    // Channel 接线有效：模拟后端推送事件 → 消息归约、终态注销进行中 Run
    const channel = channelInstances[channelInstances.length - 1];
    expect(channel).toBeDefined();
    channel.onmessage?.(ev('r1', 0, runStarted('你好')));
    channel.onmessage?.(ev('r1', 1, runCompleted('你好，有什么可以帮你')));
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => `${m.role}:${m.content}`)).toEqual([
      'user:你好',
      'assistant:你好，有什么可以帮你',
    ]);
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBeUndefined();
  });

  it('startRun 失败（success=false / 异常）：返回 null 且不登记进行中 Run', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_start') {
        return { success: false, data: null, error: 'provider 未配置' };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);
    expect(await useAgentStore.getState().startRun(null, 'x')).toBeNull();
    expect(useAgentStore.getState().runningRunByThreadId).toEqual({});

    invokeMock.mockImplementation((async () => {
      throw new Error('network down');
    }) as unknown as typeof invoke);
    expect(await useAgentStore.getState().startRun(null, 'x')).toBeNull();
    expect(useAgentStore.getState().runningRunByThreadId).toEqual({});
  });

  it('cancelRun：信号已发返回 true，但不清 running 登记（只认终态事件）', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_cancel') {
        return { success: true, data: true, error: null };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);
    useAgentStore.setState({ runningRunByThreadId: { t1: 'r1' } });

    expect(await useAgentStore.getState().cancelRun('r1')).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith('agent_run_cancel', { runId: 'r1' });
    // 停止信号已发但运行可能仍在跑：登记保留，避免状态双口径
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBe('r1');

    // 终态事件到达后才注销，且取消消息可见
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, runStarted('长任务')));
    handleEvent(ev('r1', 1, runCancelled('用户停止')));
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBeUndefined();
    expect(useAgentStore.getState().messagesByThreadId['t1'][1].content).toBe(
      '运行已取消：用户停止'
    );
  });

  it('cancelRun 失败（success=false / 异常）：返回 false', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_cancel') {
        return { success: false, data: false, error: 'run 不存在' };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);
    expect(await useAgentStore.getState().cancelRun('nope')).toBe(false);

    invokeMock.mockImplementation((async () => {
      throw new Error('boom');
    }) as unknown as typeof invoke);
    expect(await useAgentStore.getState().cancelRun('nope')).toBe(false);
  });

  it('三种终态事件都注销进行中 Run（completed/failed/cancelled）', () => {
    useAgentStore.setState({
      runningRunByThreadId: { tA: 'rA', tB: 'rB', tC: 'rC' },
    });
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('rA', 0, runCompleted('ok'), { threadId: 'tA' }));
    handleEvent(ev('rB', 0, runFailed('err'), { threadId: 'tB' }));
    handleEvent(ev('rC', 0, runCancelled('stop'), { threadId: 'tC' }));
    expect(useAgentStore.getState().runningRunByThreadId).toEqual({});
  });

  it('旧 Run 的迟到终态不会清掉同 Thread 新 Run 的门禁', () => {
    useAgentStore.setState({ runningRunByThreadId: { t1: 'r-new' } });
    useAgentStore.getState().handleEvent(ev('r-old', 0, runCompleted('旧回答')));
    expect(useAgentStore.getState().runningRunByThreadId.t1).toBe('r-new');
  });
});

// ------------------- Thread 列表与派生函数 -------------------

describe('AG-23 loadThreads 与派生函数', () => {
  const threadRow = {
    id: 't1',
    title: '阅读助手',
    status: 'completed' as const,
    projectId: 'p1',
    latestRunId: 'r1',
    createdAt: 1000,
    updatedAt: 2000,
  };

  it('loadThreads 已有列表时不把 loading 拉回 true', async () => {
    useAgentStore.setState({ threads: [threadRow], loading: false });
    let release: (value: unknown) => void = () => {};
    invokeMock.mockImplementation(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }) as never,
    );
    const pending = useAgentStore.getState().loadThreads('p1');
    expect(useAgentStore.getState().loading).toBe(false);
    release({ success: true, data: [threadRow], error: null });
    await pending;
    expect(useAgentStore.getState().loading).toBe(false);
  });

  it('loadThreads 成功：threads 更新且 loading 归位', async () => {
    invokeMock.mockResolvedValue({
      success: true,
      data: [threadRow],
      error: null,
    } as never);
    await useAgentStore.getState().loadThreads('p1');
    expect(useAgentStore.getState().threads).toEqual([threadRow]);
    expect(useAgentStore.getState().loading).toBe(false);
    expect(invokeMock).toHaveBeenCalledWith('agent_thread_list', { projectId: 'p1', scope: 'active' });
  });

  it('loadThreads 失败（success=false / 异常）：threads 不脏、loading 归位', async () => {
    useAgentStore.setState({ threads: [threadRow] });
    invokeMock.mockResolvedValue({
      success: false,
      data: null,
      error: 'db locked',
    } as never);
    await useAgentStore.getState().loadThreads();
    expect(useAgentStore.getState().threads).toEqual([threadRow]); // 旧数据保留
    expect(useAgentStore.getState().loading).toBe(false);

    invokeMock.mockRejectedValue(new Error('network down') as never);
    await useAgentStore.getState().loadThreads();
    expect(useAgentStore.getState().threads).toEqual([threadRow]);
    expect(useAgentStore.getState().loading).toBe(false);
  });

  it('派生函数缺省：未知 thread/run 返回空集合与 null', () => {
    const s = useAgentStore.getState();
    expect(s.messagesOfThread('nope')).toEqual([]);
    expect(s.toolCardsOfThread('nope')).toEqual([]);
    expect(s.eventsOfRun('nope')).toEqual([]);
    expect(s.degradedOfThread('nope')).toBeNull();
  });
});
