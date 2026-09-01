/**
 * AG-21：agentStore 工具结果卡归约单测。
 * 覆盖：tool_started/tool_completed 经 handleEvent 归约进 toolCardsByThreadId、
 * 五件套字段（structured/uiArtifact/truncated/provenance）贯通、
 * 旧事件（无 AG-21 字段）缺省兜底不降级、卡片不含 model_text 通道。
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
import { useAgentStore, type AgentEvent, type AgentEventPayload } from '../agentStore';

const invokeMock = vi.mocked(invoke);

function ev(runId: string, seq: number, payload: AgentEventPayload, over: Partial<AgentEvent> = {}): AgentEvent {
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

describe('AG-21 handleEvent 工具卡归约', () => {
  it('started→completed 归约出完成卡，五件套字段全贯通', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '查天气', maxTurns: 3 }));
    handleEvent(ev('r1', 1, { type: 'tool_started', callId: 'c1', name: 'get_weather', argumentsJson: '{}' }));
    handleEvent(
      ev('r1', 2, {
        type: 'tool_completed',
        callId: 'c1',
        name: 'get_weather',
        ok: true,
        error: null,
        preresolved: false,
        structured: { city: '杭州', temperature_c: 26 },
        uiArtifact: {
          kind: 'key-value',
          schemaVersion: 1,
          payload: { rows: [['city', '杭州']] },
          fallbackMarkdown: '**杭州**多云',
          provenance: [{ source: 'tool', sourceId: 'get_weather' }],
        },
        truncated: false,
        provenance: [{ source: 'tool', sourceId: 'get_weather' }],
      })
    );

    const cards = useAgentStore.getState().toolCardsByThreadId['t1'];
    expect(cards).toHaveLength(1);
    const card = cards[0];
    expect(card.status).toBe('completed');
    expect(card.name).toBe('get_weather');
    expect(card.structured).toEqual({ city: '杭州', temperature_c: 26 });
    expect(card.uiArtifact?.kind).toBe('key-value');
    expect(card.truncated).toBe(false);
    expect(card.provenance?.[0].sourceId).toBe('get_weather');
    // 「工具结果卡不解析 model_text」：卡片上没有该通道
    expect('model_text' in card).toBe(false);
    expect('modelText' in card).toBe(false);
    // 无降级登记（AG-21 字段扩展不改已知类型集）
    expect(useAgentStore.getState().degraded['r1']).toBeUndefined();
  });

  it('旧事件（AG-21 前落库）无新字段 → 缺省兜底，仍归约为完成卡', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '算加法', maxTurns: 3 }));
    handleEvent(ev('r1', 1, { type: 'tool_started', callId: 'c1', name: 'calculator', argumentsJson: '{}' }));
    // 旧 JSON 反序列化出来没有 structured/uiArtifact/truncated/provenance
    handleEvent(
      ev('r1', 2, {
        type: 'tool_completed',
        callId: 'c1',
        name: 'calculator',
        ok: true,
        error: null,
        preresolved: false,
      })
    );

    const cards = useAgentStore.getState().toolCardsByThreadId['t1'];
    expect(cards).toHaveLength(1);
    expect(cards[0].status).toBe('completed');
    expect(cards[0].truncated).toBe(false);
    expect(cards[0].provenance).toEqual([]);
    expect(cards[0].uiArtifact).toBeNull();
    expect(useAgentStore.getState().degraded['r1']).toBeUndefined();
  });

  it('running 中间态可见；失败卡带错误文本', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '算', maxTurns: 3 }));
    handleEvent(ev('r1', 1, { type: 'tool_started', callId: 'c1', name: 'calculator', argumentsJson: '{}' }));
    let cards = useAgentStore.getState().toolCardsByThreadId['t1'];
    expect(cards[0].status).toBe('running');

    handleEvent(
      ev('r1', 2, {
        type: 'tool_completed',
        callId: 'c1',
        name: 'calculator',
        ok: false,
        error: '除数不能为零',
        preresolved: false,
      })
    );
    cards = useAgentStore.getState().toolCardsByThreadId['t1'];
    expect(cards[0].status).toBe('failed');
    expect(cards[0].error).toBe('除数不能为零');
  });

  it('跨 Run 归约：同 Thread 第二轮的工具卡与第一轮并存', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '第一轮', maxTurns: 3 }));
    handleEvent(ev('r1', 1, { type: 'tool_started', callId: 'c1', name: 'get_weather', argumentsJson: '{}' }));
    handleEvent(
      ev('r1', 2, {
        type: 'tool_completed', callId: 'c1', name: 'get_weather', ok: true, error: null, preresolved: false,
      })
    );
    handleEvent(ev('r1', 3, { type: 'run_completed', outcome: 'completed', finalAnswer: '答1', modelCalls: 2 }));
    handleEvent(ev('r2', 0, { type: 'run_started', userMessage: '第二轮', maxTurns: 3 }, { eventId: 'r2:0' }));
    handleEvent(ev('r2', 1, { type: 'tool_started', callId: 'c2', name: 'calculator', argumentsJson: '{}' }));
    handleEvent(
      ev('r2', 2, {
        type: 'tool_completed', callId: 'c2', name: 'calculator', ok: true, error: null, preresolved: false,
      })
    );

    const cards = useAgentStore.getState().toolCardsByThreadId['t1'];
    expect(cards.map((c) => c.callId)).toEqual(['c1', 'c2']);
    expect(useAgentStore.getState().toolCardsOfThread('t1')).toHaveLength(2);
  });

  it('重复事件幂等：同 eventId 双路送达不重复建卡', () => {
    const { handleEvent } = useAgentStore.getState();
    handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '查', maxTurns: 3 }));
    const startedEvent = ev('r1', 1, { type: 'tool_started', callId: 'c1', name: 'get_weather', argumentsJson: '{}' });
    handleEvent(startedEvent);
    handleEvent(startedEvent); // 实时 + 重放双路
    handleEvent(
      ev('r1', 2, {
        type: 'tool_completed', callId: 'c1', name: 'get_weather', ok: true, error: null, preresolved: false,
      })
    );
    expect(useAgentStore.getState().toolCardsByThreadId['t1']).toHaveLength(1);
  });
});
