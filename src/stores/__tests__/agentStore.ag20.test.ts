/**
 * AG-20：事件可靠性与缺口恢复前端单测（审计 P0-3 整改项③④）。
 * 覆盖：seq 缺口自动检测 + replay 补齐 / Snapshot 升级 / 补不齐显式降级、
 * 未知 schemaVersion 与 payload 类型的显式降级（不静默跳过）、
 * 重复事件幂等、重放/历史流坏 JSON 登记降级。
 * 全部 mock `@tauri-apps/api/core`，零真实模型调用、零 Tauri 依赖。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

import { invoke } from '@tauri-apps/api/core';
import {
  useAgentStore,
  firstGapSeq,
  isKnownEvent,
  type AgentEvent,
  type AgentEventPayload,
} from '../agentStore';

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
const modelStarted: AgentEventPayload = { type: 'model_started', turn: 1 };
const runCompleted = (answer = '完成'): AgentEventPayload => ({
  type: 'run_completed',
  outcome: 'completed',
  finalAnswer: answer,
  modelCalls: 1,
});

/** 等待 fire-and-forget 的恢复阶梯跑完（invoke mock 均微任务解析） */
async function flush(times = 10) {
  for (let i = 0; i < times; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

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
});

// ------------------- 纯函数：缺口检测与协议判别 -------------------

describe('AG-20 纯函数', () => {
  it('firstGapSeq：空列表/连续序列无缺口', () => {
    expect(firstGapSeq([])).toBeNull();
    expect(firstGapSeq([ev('r', 0, runStarted()), ev('r', 1, modelStarted)])).toBeNull();
  });

  it('firstGapSeq：中间空洞与 seq=0 缺失；乱序/重复不干扰', () => {
    const gapped = [ev('r', 0, runStarted()), ev('r', 2, runCompleted())];
    expect(firstGapSeq(gapped)).toBe(1);
    expect(firstGapSeq([ev('r', 1, modelStarted)])).toBe(0);
    // 乱序 + 重复归一后仍连续
    const messy = [ev('r', 2, runCompleted()), ev('r', 0, runStarted()), ev('r', 1, modelStarted), ev('r', 1, modelStarted)];
    expect(firstGapSeq(messy as unknown as AgentEvent[])).toBeNull();
  });

  it('isKnownEvent：版本不符或类型未知都拒绝', () => {
    expect(isKnownEvent(ev('r', 0, runStarted()))).toBe(true);
    expect(isKnownEvent(ev('r', 0, runStarted(), { schemaVersion: 2 }))).toBe(true);
    expect(isKnownEvent(ev('r', 0, runStarted(), { schemaVersion: 3 }))).toBe(true);
    expect(isKnownEvent(ev('r', 0, runStarted(), { schemaVersion: 4 }))).toBe(true);
    expect(isKnownEvent(ev('r', 0, runStarted(), { schemaVersion: 5 }))).toBe(false);
    const delta = ev('r', 0, { type: 'message_delta', text: 'hi' } as AgentEventPayload, {
      schemaVersion: 2,
    });
    expect(isKnownEvent(delta)).toBe(true);
    const future = ev('r', 0, { type: 'totally_unknown_event' } as unknown as AgentEventPayload);
    expect(isKnownEvent(future)).toBe(false);
  });
});

// ------------------- 缺口补齐阶梯 -------------------

describe('AG-20 seq 缺口检测 + 自动 replay', () => {
  it('检测到缺口自动 replay 补齐，补齐后无降级', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_events_replay') {
        return {
          success: true,
          data: [JSON.stringify(ev('r1', 1, modelStarted))],
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    const store = useAgentStore.getState();
    store.handleEvent(ev('r1', 0, runStarted()));
    store.handleEvent(ev('r1', 2, runCompleted())); // seq=1 缺失 → 触发阶梯
    await flush();

    const events = useAgentStore.getState().eventsByRunId['r1'];
    expect(events.map((e) => e.seq)).toEqual([0, 1, 2]);
    expect(invokeMock).toHaveBeenCalledWith('agent_run_events_replay', {
      runId: 'r1',
      afterSeq: 0, // gap=1 → 排他语义取 seq>0
    });
    expect(useAgentStore.getState().degraded['r1']).toBeUndefined();
    // 消息归约不受缺口影响：补齐前后终态一致（user + assistant）
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => m.role)).toEqual(['user', 'assistant']);
  });

  it('replay 填不上时升级 Snapshot 补齐', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_events_replay') {
        return { success: true, data: [], error: null }; // DB 重放拿不到（模拟瞬态）
      }
      if (cmd === 'agent_run_snapshot') {
        return {
          success: true,
          data: {
            runId: 'r2',
            threadId: 't1',
            runStatus: 'completed',
            latestSeq: 2,
            events: [JSON.stringify(ev('r2', 1, modelStarted))],
            messages: [],
            toolCalls: [],
            pendingApprovals: [],
          },
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    const store = useAgentStore.getState();
    store.handleEvent(ev('r2', 0, runStarted()));
    store.handleEvent(ev('r2', 2, runCompleted()));
    await flush();

    expect(useAgentStore.getState().eventsByRunId['r2'].map((e) => e.seq)).toEqual([0, 1, 2]);
    expect(invokeMock).toHaveBeenCalledWith('agent_run_snapshot', { runId: 'r2' });
    expect(useAgentStore.getState().degraded['r2']).toBeUndefined();
  });

  it('缺口补不齐 → 显式降级登记，不猜测、不崩溃', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_run_events_replay') {
        return { success: true, data: [], error: null };
      }
      if (cmd === 'agent_run_snapshot') {
        return {
          success: true,
          data: {
            runId: 'r3',
            threadId: 't1',
            runStatus: 'completed',
            latestSeq: 2,
            events: [], // 快照也没有 → 事件真实丢失
            messages: [],
            toolCalls: [],
            pendingApprovals: [],
          },
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    const store = useAgentStore.getState();
    store.handleEvent(ev('r3', 0, runStarted()));
    store.handleEvent(ev('r3', 2, runCompleted()));
    await flush();

    const degraded = useAgentStore.getState().degraded['r3'];
    expect(degraded).toBeDefined();
    expect(degraded).toContain('缺口');
    // 降级后不再循环触发恢复（handleEvent 见 degraded 即停）：
    // 再投一条事件不应产生新的 invoke 调用
    const callsBefore = invokeMock.mock.calls.length;
    useAgentStore.getState().handleEvent(ev('r3', 3, { type: 'model_started', turn: 2 }));
    await flush();
    expect(invokeMock.mock.calls.length).toBe(callsBefore);
    // degradedOfThread 派生可读
    expect(useAgentStore.getState().degradedOfThread('t1')).toContain('缺口');
  });
});

// ------------------- schema 降级 -------------------

describe('AG-20 schema 降级（未知版本/类型不静默跳过）', () => {
  it('未知 schemaVersion：登记降级且不进归约流', () => {
    const store = useAgentStore.getState();
    store.handleEvent(ev('r4', 0, runStarted(), { schemaVersion: 5 }));
    expect(useAgentStore.getState().degraded['r4']).toContain('未知事件协议');
    expect(useAgentStore.getState().eventsByRunId['r4'] ?? []).toHaveLength(0);
    expect(useAgentStore.getState().messagesByThreadId['t1'] ?? []).toHaveLength(0);
  });

  it('未知 payload 类型（未来版本新事件）：同样显式降级', () => {
    const future = ev('r5', 0, { type: 'document_patch_proposed' } as unknown as AgentEventPayload);
    useAgentStore.getState().handleEvent(future);
    expect(useAgentStore.getState().degraded['r5']).toContain('未知事件协议');
    expect(useAgentStore.getState().eventsByRunId['r5'] ?? []).toHaveLength(0);
  });
});

// ------------------- 幂等与坏数据 -------------------

describe('AG-20 幂等与坏数据登记', () => {
  it('重复事件（实时+重放双路送达）只保留一条', () => {
    const store = useAgentStore.getState();
    const e = ev('r6', 0, runStarted());
    store.handleEvent(e);
    store.handleEvent(e);
    expect(useAgentStore.getState().eventsByRunId['r6']).toHaveLength(1);
  });

  it('历史恢复中的坏 JSON 登记 Thread 级降级，好事件照常归约', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_thread_history') {
        return {
          success: true,
          data: [
            JSON.stringify(ev('r7', 0, runStarted('第二轮'))),
            '{坏掉的JSON',
            JSON.stringify(ev('r7', 1, runCompleted('第二轮答复'))),
          ],
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    await useAgentStore.getState().loadThreadHistory('t1');
    expect(useAgentStore.getState().degraded['thread:t1']).toContain('无法解析');
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs).toHaveLength(2);
    expect(msgs[0].content).toBe('第二轮');
  });

  it('重挂载追平：恢复时未终态的 Run 立即 replay 补齐后续已提交事件', async () => {
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_thread_history') {
        // 恢复时刻 Run 仍在进行：只有 seq0/seq1，无终态事件
        return {
          success: true,
          data: [
            JSON.stringify(ev('r8', 0, runStarted())),
            JSON.stringify(ev('r8', 1, modelStarted)),
          ],
          error: null,
        };
      }
      if (cmd === 'agent_run_events_replay') {
        // 恢复时刻之后 DB 又提交了终态事件
        return {
          success: true,
          data: [JSON.stringify(ev('r8', 2, runCompleted()))],
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    await useAgentStore.getState().loadThreadHistory('t1');
    // afterSeq=已见最大 seq（排他语义正好取恢复后提交的事件）
    expect(invokeMock).toHaveBeenCalledWith('agent_run_events_replay', {
      runId: 'r8',
      afterSeq: 1,
    });
    const events = useAgentStore.getState().eventsByRunId['r8'];
    expect(events.map((e) => e.seq)).toEqual([0, 1, 2]);
    // 终态已归约：消息含 user + assistant，且无降级
    expect(useAgentStore.getState().messagesByThreadId['t1'].map((m) => m.role)).toEqual([
      'user',
      'assistant',
    ]);
    expect(useAgentStore.getState().degradedOfThread('t1')).toBeNull();
  });

  it('重挂载非终态 Run：恢复期间占用发送门禁，Snapshot 终态后才释放', async () => {
    let resolveHistory!: (value: unknown) => void;
    const historyPending = new Promise((resolve) => { resolveHistory = resolve; });
    let historyResolved = false;
    invokeMock.mockImplementation((async (cmd: string) => {
      if (cmd === 'agent_thread_history') {
        const result = await historyPending;
        historyResolved = true;
        return result;
      }
      if (cmd === 'agent_run_events_replay') {
        return { success: true, data: [], error: null };
      }
      if (cmd === 'agent_run_reconcile') {
        return {
          success: true,
          data: {
            runId: 'r9',
            threadId: 't1',
            runStatus: 'completed',
            latestSeq: 2,
            events: [JSON.stringify(ev('r9', 2, runCompleted('恢复完成')))],
            messages: [],
            toolCalls: [],
            pendingApprovals: [],
          },
          error: null,
        };
      }
      throw new Error(`unexpected command: ${cmd}`);
    }) as unknown as typeof invoke);

    const loading = useAgentStore.getState().loadThreadHistory('t1');
    expect(historyResolved).toBe(false);
    expect(useAgentStore.getState().historyLoadingByThreadId.t1).toBe(1);
    expect(await useAgentStore.getState().startRun('t1', '不应发送')).toBeNull();

    resolveHistory({
      success: true,
      data: [
        JSON.stringify(ev('r9', 0, runStarted('未完成问题'))),
        JSON.stringify(ev('r9', 1, modelStarted)),
      ],
      error: null,
    });
    await loading;

    // 初次 replay 无终态，恢复监视器接管；在 Snapshot 归约终态前保持锁。
    await flush();
    const state = useAgentStore.getState();
    expect(state.historyLoadingByThreadId.t1).toBeUndefined();
    expect(state.runningRunByThreadId.t1).toBeUndefined();
    expect(state.resumingRunByThreadId.t1).toBeUndefined();
    expect(state.messagesByThreadId.t1.at(-1)?.content).toBe('恢复完成');
  });
});
