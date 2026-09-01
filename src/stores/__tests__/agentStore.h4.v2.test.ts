/**
 * H4 / NEXT-021：AgentEvent v2 归约
 * - schema 1|2 可归约；更高版本显式降级
 * - message_delta 拼正文；engine_degraded 非终态（不标 completed）
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
  isKnownEvent,
  normalizeAssistantMarkdown,
  stabilizeStreamingMarkdown,
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
    threads: [],
    selectedThreadId: null,
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

describe('H4 AgentEvent v2', () => {
  it('接受 schema 1 与 2', () => {
    expect(
      isKnownEvent(
        ev('r', 0, { type: 'run_started', userMessage: 'hi', maxTurns: 1 }, { schemaVersion: 1 })
      )
    ).toBe(true);
    expect(
      isKnownEvent(
        ev('r', 0, { type: 'run_started', userMessage: 'hi', maxTurns: 1 }, { schemaVersion: 2 })
      )
    ).toBe(true);
  });

  it('message_delta 拼正文，run_completed 定稿', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(ev('r1', 1, { type: 'message_delta', text: '你好' }));
    store.handleEvent(ev('r1', 2, { type: 'message_delta', text: '世界' }));
    let msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs.map((m) => m.role)).toEqual(['user', 'assistant']);
    expect(msgs[1].content).toBe('你好世界');
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBe('r1');

    store.handleEvent(
      ev('r1', 3, {
        type: 'run_completed',
        outcome: 'completed',
        finalAnswer: '你好世界！',
        modelCalls: 0,
      })
    );
    msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[1].content).toBe('你好世界！');
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBeUndefined();
  });

  it('message_interim 只作为进度事件，对话只保留最终答案', () => {
    const store = useAgentStore.getState();
    store.handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '做网页', maxTurns: 1 }));
    store.handleEvent(ev('r1', 1, { type: 'message_delta', text: '我先检查项目。' }));
    store.handleEvent(ev('r1', 2, {
      type: 'message_interim',
      text: '我先检查项目。',
      alreadyStreamed: true,
    }));
    store.handleEvent(ev('r1', 3, { type: 'message_delta', text: '页面已经完成。' }));
    store.handleEvent(ev('r1', 4, {
      type: 'run_completed',
      outcome: 'completed',
      finalAnswer: '页面已经完成。',
      modelCalls: 1,
    }));
    const assistants = useAgentStore.getState().messagesByThreadId.t1.filter(
      (message) => message.role === 'assistant'
    );
    expect(assistants.map((message) => message.content)).toEqual(['页面已经完成。']);
  });

  it('run_completed 无 finalAnswer 时回退流式正文', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(ev('r1', 1, { type: 'message_delta', text: '仅流式' }));
    store.handleEvent(
      ev('r1', 2, {
        type: 'run_completed',
        outcome: 'completed',
        finalAnswer: '',
        modelCalls: 0,
      })
    );
    expect(useAgentStore.getState().messagesByThreadId['t1'][1].content).toBe('仅流式');
  });

  it('run_completed 无 finalAnswer 时可回退最后一条 interim，但不产生重复卡片', () => {
    const store = useAgentStore.getState();
    store.handleEvent(ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 }));
    store.handleEvent(ev('r1', 1, { type: 'message_delta', text: '可用结果' }));
    store.handleEvent(ev('r1', 2, {
      type: 'message_interim',
      text: '可用结果',
      alreadyStreamed: true,
    }));
    store.handleEvent(ev('r1', 3, {
      type: 'run_completed',
      outcome: 'completed',
      finalAnswer: '',
      modelCalls: 0,
    }));
    const assistants = useAgentStore.getState().messagesByThreadId.t1.filter(
      (message) => message.role === 'assistant'
    );
    expect(assistants.map((message) => message.content)).toEqual(['可用结果']);
  });

  it('normalizeAssistantMarkdown 拆开挤在一起的标题', () => {
    const raw = '前文结束。## 标题一 内容 ### 小节 更多';
    const out = normalizeAssistantMarkdown(raw);
    expect(out).toContain('\n## 标题一');
    expect(out).toContain('\n### 小节');
  });

  it('normalizeAssistantMarkdown 修复标点后紧邻正文的加粗收口', () => {
    const raw = '> **在 lm-eval-harness 里，记忆根本不存在。**因为它假设模型无状态。';
    expect(normalizeAssistantMarkdown(raw)).toBe(
      '> **在 lm-eval-harness 里，记忆根本不存在。** 因为它假设模型无状态。'
    );
  });

  it('normalizeAssistantMarkdown 不改写围栏代码中的加粗与空行', () => {
    const raw = ['```md', '**字面量。**正文', '', '', '## 也是字面量', '```'].join('\n');
    expect(normalizeAssistantMarkdown(raw)).toBe(raw);
  });

  it('normalize / stabilize 不得拆坏 GFM 表格', () => {
    const table = [
      '根据定义：',
      '',
      '| 事件族 | 用途 | 示例 |',
      '|--------|------|------|',
      '| `RUN_*` | Run 生命周期 | `RUN_STARTED` |',
      '| `TEXT_*` | 流式文本输出 | `TEXT_MESSAGE_CONTENT` |',
      '',
      '类比 HTTP。',
    ].join('\n');
    const normalized = normalizeAssistantMarkdown(table);
    const stabilized = stabilizeStreamingMarkdown(table);
    for (const out of [normalized, stabilized]) {
      expect(out).toContain('| 事件族 | 用途 | 示例 |');
      expect(out).toContain('| `RUN_*` | Run 生命周期 | `RUN_STARTED` |');
      expect(out.split('\n').filter((l) => l.trim().startsWith('|')).length).toBeGreaterThanOrEqual(4);
      // 回归：旧正则会把单元格内 `|` 拆成 `||` 墙
      expect(out).not.toMatch(/\|\|/);
    }
  });

  it('stabilizeStreamingMarkdown 闭合未完成代码围栏', () => {
    const out = stabilizeStreamingMarkdown('说明\n```ts\nconst a = 1');
    expect(out.trimEnd().endsWith('```')).toBe(true);
    expect(out).toContain('const a = 1');
  });

  it('stabilizeStreamingMarkdown 保留流式尾部换行', () => {
    const out = stabilizeStreamingMarkdown('## 标题：流式预览。\n\n');
    expect(out).toBe('## 标题：流式预览。\n\n');
  });

  it('streaming 消息在归约时即规范化标题换行', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(
      ev('r1', 1, { type: 'message_delta', text: '概述如下。## 分层 L0-L4 说明' })
    );
    const assistant = useAgentStore.getState().messagesByThreadId['t1'].find(
      (m) => m.role === 'assistant'
    );
    expect(assistant?.id).toContain(':streaming');
    expect(assistant?.content).toContain('\n## 分层');
  });

  it('reasoning_delta 归约到消息 reasoning，不进 content', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(ev('r1', 1, { type: 'reasoning_delta', text: '先读文档' }));
    store.handleEvent(ev('r1', 2, { type: 'message_delta', text: '结论' }));
    store.handleEvent(
      ev('r1', 3, {
        type: 'run_completed',
        outcome: 'completed',
        finalAnswer: '结论',
        modelCalls: 0,
      })
    );
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    const assistant = msgs.find((m) => m.role === 'assistant');
    expect(assistant?.content).toBe('结论');
    expect(assistant?.reasoning).toBe('先读文档');
    expect(assistant?.phase).toBe('done');
    expect(assistant?.thinkingStatus).toBe('done');
    expect(assistant?.contentStatus).toBe('done');
  });

  it('Hermes 回标尾部中间思考时从答案区移入思考区', () => {
    const store = useAgentStore.getState();
    const preamble = '先读取项目文档，再基于完整上下文回答用户的问题。';
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(ev('r1', 1, { type: 'message_delta', text: preamble }));
    store.handleEvent(ev('r1', 2, { type: 'reasoning_delta', text: preamble }));
    const assistant = useAgentStore.getState().messagesByThreadId['t1'].find(
      (message) => message.role === 'assistant'
    );
    expect(assistant?.content).toBe('');
    expect(assistant?.reasoning).toBe(preamble);
    expect(assistant?.phase).toBe('thinking');
  });

  it('首条 message_delta 将 phase 从 thinking 推进到 answering', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    let assistant = useAgentStore
      .getState()
      .messagesByThreadId['t1']
      .find((m) => m.role === 'assistant');
    expect(assistant?.phase).toBe('thinking');
    expect(assistant?.contentStatus).toBe('pending');
    store.handleEvent(ev('r1', 1, { type: 'reasoning_delta', text: '想一下' }));
    assistant = useAgentStore
      .getState()
      .messagesByThreadId['t1']
      .find((m) => m.role === 'assistant');
    expect(assistant?.phase).toBe('thinking');
    store.handleEvent(ev('r1', 2, { type: 'message_delta', text: '答' }));
    assistant = useAgentStore
      .getState()
      .messagesByThreadId['t1']
      .find((m) => m.role === 'assistant');
    expect(assistant?.phase).toBe('answering');
    expect(assistant?.contentStatus).toBe('streaming');
  });

  it('同一流式窗口批量归约，保留 delta 顺序与 Markdown 结构', () => {
    let storeCommits = 0;
    const unsubscribe = useAgentStore.subscribe(() => { storeCommits += 1; });
    useAgentStore.getState().handleEvents([
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 }),
      ev('r1', 1, { type: 'message_delta', text: '##' }),
      ev('r1', 2, { type: 'message_delta', text: ' ' }),
      ev('r1', 3, { type: 'message_delta', text: '标题\n\n' }),
      ev('r1', 4, { type: 'message_delta', text: '```\n图\n```' }),
    ]);
    const state = useAgentStore.getState();
    const assistant = state.messagesByThreadId['t1'].find((message) => message.role === 'assistant');
    expect(assistant?.content).toContain('## 标题');
    expect(assistant?.content.match(/```/g)).toHaveLength(2);
    expect(state.eventsByRunId['r1']).toHaveLength(5);
    expect(assistant?.phase).toBe('answering');
    expect(storeCommits).toBe(1);
    unsubscribe();
  });

  it('回答后到达的 reasoning.available 答案回声不进思考区', () => {
    const answer = '这是已经在结果区流式输出的完整答案前缀，长度足以识别重复内容。';
    useAgentStore.getState().handleEvents([
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 }),
      ev('r1', 1, { type: 'message_delta', text: answer }),
      ev('r1', 2, { type: 'reasoning_delta', text: answer.slice(0, 35) }),
      ev('r1', 3, {
        type: 'run_completed',
        outcome: 'completed',
        finalAnswer: answer,
        modelCalls: 0,
      }),
    ]);
    const assistant = useAgentStore
      .getState()
      .messagesByThreadId['t1']
      .find((message) => message.role === 'assistant');
    expect(assistant?.content).toBe(answer);
    expect(assistant?.reasoning).toBeNull();
  });

  it('engine_degraded 登记降级且不清除 running / 不标 completed', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(
      ev('r1', 1, {
        type: 'engine_degraded',
        reason: 'sse_dropped',
        reconnecting: true,
      })
    );
    const state = useAgentStore.getState();
    expect(state.degraded['r1']).toContain('重连中');
    expect(state.runningRunByThreadId['t1']).toBe('r1');
    // run_started 即挂流式助手气泡（边推理边可见）；engine_degraded 不得定稿 completed
    const assistant = state.messagesByThreadId['t1'].find((m) => m.role === 'assistant');
    expect(assistant?.id).toContain(':streaming');
    expect(assistant?.content ?? '').toBe('');
  });

  it('interrupted（run_failed outcome）可见失败，不出现 completed 终态消息', () => {
    const store = useAgentStore.getState();
    store.handleEvent(
      ev('r1', 0, { type: 'run_started', userMessage: '问', maxTurns: 1 })
    );
    store.handleEvent(
      ev('r1', 1, {
        type: 'run_failed',
        outcome: 'interrupted',
        error: 'SSE 对账不可恢复',
      })
    );
    const msgs = useAgentStore.getState().messagesByThreadId['t1'];
    expect(msgs[1].content).toContain('运行失败');
    expect(msgs[1].content).toContain('对账');
    expect(useAgentStore.getState().runningRunByThreadId['t1']).toBeUndefined();
  });
});
