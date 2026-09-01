import { beforeEach, describe, expect, it, vi } from 'vitest';

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
import { useAgentStore, type AgentEvent } from '../../stores/agentStore';
import {
  discoveryAgentErrorMessage,
  resolveDiscoveryHermesModel,
  runHermesDiscoveryAnalysis,
} from '../discoveryAgent';

const invokeMock = vi.mocked(invoke);

function event(payload: AgentEvent['payload'], seq = 1): AgentEvent {
  return {
    eventId: `event-${seq}`,
    threadId: 'thread-discovery',
    runId: 'run-discovery',
    seq,
    timestamp: Date.now(),
    schemaVersion: 1,
    payload,
  };
}

const modelOptionsResponse = {
  success: true,
  data: {
    model: 'default',
    provider: 'moa',
    providers: [
      {
        slug: 'moa',
        name: 'Mixture of Agents',
        models: ['default'],
        authenticated: true,
        isCurrent: true,
      },
      {
        slug: 'deepseek',
        name: 'DeepSeek',
        models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
        authenticated: true,
        isCurrent: false,
      },
    ],
  },
  error: null,
};

beforeEach(() => {
  invokeMock.mockReset();
  channelInstances.length = 0;
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
});

describe('Hermes discovery card actions', () => {
  it('submits only action/itemId through the native discovery Skill and waits for completion', async () => {
    invokeMock.mockImplementation(async (command, args) => {
      if (command === 'agent_hermes_models') return modelOptionsResponse;
      if (command === 'agent_thread_list') {
        return { success: true, data: [], error: null };
      }
      expect(command).toBe('agent_run_start');
      const request = (args as { request: Record<string, unknown> }).request;
      expect(request.message).toBe('action=quick itemId=item-42 language=zh-CN');
      expect(request.skill).toBe('sophonote-ai-radar');
      expect(request.hermesProvider).toBe('deepseek');
      expect(request.hermesModel).toBe('deepseek-v4-flash');
      setTimeout(() => {
        channelInstances[0].onmessage?.(event({
          type: 'run_started',
          userMessage: 'action=quick itemId=item-42',
          maxTurns: 6,
        }, 0));
        channelInstances[0].onmessage?.(event({
          type: 'run_completed',
          outcome: 'completed',
          finalAnswer: '已保存',
          modelCalls: 1,
        }));
      }, 0);
      return {
        success: true,
        data: { threadId: 'thread-discovery', runId: 'run-discovery' },
        error: null,
      };
    });

    await expect(runHermesDiscoveryAnalysis('item-42', 'quick')).resolves.toBeUndefined();
  });

  it('surfaces the real Hermes terminal error', async () => {
    invokeMock.mockImplementation(async (command) => {
      if (command === 'agent_hermes_models') return modelOptionsResponse;
      if (command === 'agent_thread_list') {
        return { success: true, data: [], error: null };
      }
      setTimeout(() => {
        channelInstances[0].onmessage?.(event({
          type: 'run_started',
          userMessage: 'action=deep itemId=item-42',
          maxTurns: 6,
        }, 0));
        channelInstances[0].onmessage?.(event({
          type: 'run_failed',
          outcome: 'failed',
          error: '证据不足',
        }));
      }, 0);
      return {
        success: true,
        data: { threadId: 'thread-discovery', runId: 'run-discovery' },
        error: null,
      };
    });

    await expect(runHermesDiscoveryAnalysis('item-42', 'deep')).rejects.toThrow('证据不足');
  });

  it('marks regeneration explicitly while keeping the native discovery Skill route', async () => {
    invokeMock.mockImplementation(async (command, args) => {
      if (command === 'agent_hermes_models') return modelOptionsResponse;
      if (command === 'agent_thread_list') return { success: true, data: [], error: null };
      const request = (args as { request: Record<string, unknown> }).request;
      expect(request.message).toBe(
        'action=deep itemId=item-42 regenerate=true language=zh-CN',
      );
      expect(request.skill).toBe('sophonote-ai-radar');
      setTimeout(() => {
        channelInstances[0].onmessage?.(event({
          type: 'run_started',
          userMessage: String(request.message),
          maxTurns: 6,
        }, 0));
        channelInstances[0].onmessage?.(event({
          type: 'run_completed',
          outcome: 'completed',
          finalAnswer: '已重新生成并保存',
          modelCalls: 1,
        }));
      }, 0);
      return {
        success: true,
        data: { threadId: 'thread-discovery', runId: 'run-discovery' },
        error: null,
      };
    });

    await expect(runHermesDiscoveryAnalysis(
      'item-42',
      'deep',
      { regenerate: true },
    )).resolves.toBeUndefined();
  });
});

describe('Hermes discovery model routing', () => {
  it('does not treat moa/default as a usable fallback', () => {
    expect(resolveDiscoveryHermesModel(modelOptionsResponse.data)).toEqual({
      provider: 'deepseek',
      model: 'deepseek-v4-flash',
    });
  });

  it('returns no route when only the virtual MoA provider is available', () => {
    expect(resolveDiscoveryHermesModel({
      model: 'default',
      provider: 'moa',
      providers: [modelOptionsResponse.data.providers[0]],
    })).toBeNull();
  });

  it('does not expose a provider whose authentication is unknown', () => {
    expect(resolveDiscoveryHermesModel({
      model: 'anthropic/claude-opus-5',
      provider: 'openrouter',
      providers: [{
        slug: 'openrouter',
        name: 'OpenRouter',
        models: ['anthropic/claude-opus-5'],
        authenticated: null,
        isCurrent: true,
      }],
    })).toBeNull();
  });

  it('turns the OpenRouter aggregator failure into an actionable Chinese message', () => {
    expect(discoveryAgentErrorMessage(
      'No LLM provider configured for task=moa_aggregator provider=openrouter. Run: hermes setup',
    )).toBe('Hermes 当前没有可用的模型，请到「设置 → AI 模型」完成配置后重试。');
  });
});
