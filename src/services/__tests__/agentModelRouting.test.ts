import { describe, expect, it } from 'vitest';
import { configuredAgentModels, defaultAgentModel, resolveAgentModel } from '../agentModelRouting';
import type { ProviderConfig } from '../../types';

function provider(patch: Partial<ProviderConfig> = {}): ProviderConfig {
  return {
    id: 'deepseek',
    name: 'DeepSeek',
    protocol: 'openai',
    baseUrl: 'https://api.deepseek.com',
    model: 'deepseek-v4-flash',
    models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
    ...patch,
  };
}

describe('configuredAgentModels', () => {
  it('keeps the active provider default first and removes duplicates', () => {
    expect(configuredAgentModels(provider())).toEqual([
      'deepseek-v4-flash',
      'deepseek-v4-pro',
    ]);
  });

  it('supports provider-specific K3 entries without hardcoding providers', () => {
    const kimi = provider({
      id: 'kimi',
      name: 'Kimi',
      model: 'kimi-k3',
      models: ['kimi-latest', 'kimi-k3'],
    });
    expect(configuredAgentModels(kimi)).toEqual(['kimi-k3', 'kimi-latest']);
    expect(defaultAgentModel(kimi)).toBe('kimi-k3');
  });

  it('does not leak models from another provider', () => {
    expect(configuredAgentModels(provider())).not.toContain('kimi-k3');
    expect(configuredAgentModels(undefined)).toEqual([]);
  });

  it('keeps the last valid model selected for the active provider', () => {
    expect(resolveAgentModel(provider(), 'deepseek-v4-pro')).toBe('deepseek-v4-pro');
  });

  it('falls back only when the remembered model is no longer configured', () => {
    expect(resolveAgentModel(provider(), 'removed-model')).toBe('deepseek-v4-flash');
    expect(resolveAgentModel(provider(), '')).toBe('deepseek-v4-flash');
  });
});
