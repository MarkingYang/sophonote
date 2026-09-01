import { describe, expect, it } from 'vitest';
import {
  MODEL_PROVIDER_PRESETS,
  modelProviderPreset,
  modelProviderFamilyInstances,
  orderModelProviders,
  providerCredentialReady,
  providerConfigurationReady,
  providerCredentialState,
  providerFromPreset,
  providerFromRuntimeCandidate,
  providerRequiresKey,
  runtimeModelCandidates,
  uniqueProviderId,
} from '../modelProviders';

describe('model provider presets', () => {
  it('covers the providers supported by the bundled Hermes runtime', () => {
    expect(MODEL_PROVIDER_PRESETS.map((preset) => preset.id)).toEqual([
      'openrouter',
      'deepseek',
      'kimi',
      'alibaba',
      'zai',
      'minimax-cn',
      'stepfun',
      'xiaomi',
      'ollama',
      'private-openai',
    ]);
  });

  it('ships a keyless Ollama preset pointing at the local OpenAI-compatible endpoint', () => {
    const preset = MODEL_PROVIDER_PRESETS.find((candidate) => candidate.id === 'ollama');
    expect(preset).toBeDefined();
    expect(preset?.protocol).toBe('openai');
    expect(preset?.baseUrl).toBe('http://localhost:11434/v1');
    expect(preset?.model).toBe('qwen3:8b');
    expect(preset?.requiresKey).toBe(false);
  });

  it('marks cloud presets as key-required while local presets are keyless', () => {
    for (const preset of MODEL_PROVIDER_PRESETS) {
      if (preset.id === 'ollama' || preset.id === 'private-openai') {
        expect(preset.requiresKey, preset.id).toBe(false);
      } else {
        expect(preset.requiresKey, preset.id).not.toBe(false);
      }
    }
  });

  it('carries the keyless flag from preset into the stored provider config', () => {
    const preset = MODEL_PROVIDER_PRESETS.find((candidate) => candidate.id === 'ollama');
    expect(preset).toBeDefined();
    const provider = providerFromPreset(preset!);
    expect(provider.requiresKey).toBe(false);
    const cloud = providerFromPreset(MODEL_PROVIDER_PRESETS[1]);
    expect(cloud.requiresKey).toBeUndefined();
  });

  it('copies model arrays so user edits never mutate preset metadata', () => {
    const provider = providerFromPreset(MODEL_PROVIDER_PRESETS[0]);
    provider.models.push('manual-model');
    expect(MODEL_PROVIDER_PRESETS[0].models).not.toContain('manual-model');
  });

  it('keeps the active provider first without changing the source record', () => {
    const providers = Object.fromEntries(
      MODEL_PROVIDER_PRESETS.slice(1, 4).map((preset) => [preset.id, providerFromPreset(preset)]),
    );
    expect(orderModelProviders(providers, 'kimi').map((provider) => provider.id)).toEqual([
      'kimi',
      'alibaba',
      'deepseek',
    ]);
    expect(Object.keys(providers)).toEqual(['deepseek', 'kimi', 'alibaba']);
  });

  it('distinguishes an unchecked key from a confirmed missing key', () => {
    expect(providerCredentialState({}, 'deepseek')).toBe('unchecked');
    expect(providerCredentialState({ deepseek: '' }, 'deepseek')).toBe('missing');
    expect(providerCredentialState({ deepseek: 'configured' }, 'deepseek')).toBe('configured');
  });

  it('treats keyless providers as ready without any stored credential', () => {
    expect(providerRequiresKey({ id: 'deepseek' })).toBe(true);
    expect(providerRequiresKey({ id: 'ollama', requiresKey: false })).toBe(false);
    expect(providerCredentialReady({ id: 'ollama', requiresKey: false }, {})).toBe(true);
    expect(providerCredentialReady({ id: 'deepseek' }, {})).toBe(false);
    expect(providerCredentialReady({ id: 'deepseek' }, { deepseek: 'sk-x' })).toBe(true);
    expect(providerCredentialReady({ id: 'deepseek' }, { deepseek: '' })).toBe(false);
  });

  it('assigns sequential unique ids so auth and keyless instances of one family coexist', () => {
    const ollama = { id: 'ollama', name: 'Ollama 本地', protocol: 'openai' as const, baseUrl: 'http://localhost:11434/v1', model: 'qwen3:8b', models: ['qwen3:8b'], requiresKey: false };
    expect(uniqueProviderId({}, 'ollama')).toBe('ollama');
    expect(uniqueProviderId({ ollama }, 'ollama')).toBe('ollama-2');
    expect(uniqueProviderId({ ollama, 'ollama-2': ollama }, 'ollama')).toBe('ollama-3');
  });

  it('resolves instance ids back to the base preset metadata', () => {
    expect(modelProviderPreset('ollama')?.id).toBe('ollama');
    expect(modelProviderPreset('ollama-2')?.id).toBe('ollama');
    expect(modelProviderPreset('private-openai-3')?.id).toBe('private-openai');
    expect(modelProviderPreset('minimax-cn')?.id).toBe('minimax-cn');
    expect(modelProviderPreset('unknown-1')).toBeUndefined();
  });

  it('keeps auth and keyless instances in the same provider family without sharing readiness', () => {
    const providers = {
      ollama: {
        id: 'ollama', name: 'Ollama 本地', protocol: 'openai' as const,
        baseUrl: 'http://localhost:11434/v1', model: 'qwen3:8b', models: ['qwen3:8b'], requiresKey: false,
      },
      'ollama-2': {
        id: 'ollama-2', name: 'Ollama 远程', protocol: 'openai' as const,
        baseUrl: 'https://ollama.example/v1', model: 'qwen3:32b', models: ['qwen3:32b'],
      },
    };
    expect(modelProviderFamilyInstances(providers, 'ollama').map((provider) => provider.id)).toEqual(['ollama', 'ollama-2']);
    expect(providerConfigurationReady(providers.ollama, {})).toBe(true);
    expect(providerConfigurationReady(providers['ollama-2'], {})).toBe(false);
    expect(providerConfigurationReady(providers['ollama-2'], { 'ollama-2': 'configured' })).toBe(true);
  });

  it('groups Runtime discoveries by the settings provider and removes configured models', () => {
    const configured = {
      kimi: {
        id: 'kimi',
        name: 'Kimi',
        protocol: 'openai' as const,
        baseUrl: 'https://api.moonshot.cn/v1',
        model: 'kimi-k3',
        models: ['kimi-k3'],
      },
    };
    const candidates = runtimeModelCandidates([
      { slug: 'moa', name: 'Mixture of Agents', models: ['default'], authenticated: true },
      { slug: 'moonshot', name: 'Moonshot', models: ['kimi-k3', 'kimi-k3-thinking'], authenticated: true },
      { slug: 'openrouter', name: 'OpenRouter', models: ['openai/gpt-5.6-sol'], authenticated: false },
    ], configured);

    expect(candidates).toEqual([
      expect.objectContaining({ settingsProviderId: 'kimi', models: ['kimi-k3-thinking'], providerConfigured: true }),
      expect.objectContaining({ settingsProviderId: 'openrouter', models: ['openai/gpt-5.6-sol'], providerConfigured: false }),
    ]);
  });

  it('subtracts models configured by every instance in a provider family', () => {
    const configured = {
      kimi: {
        id: 'kimi', name: 'Kimi', protocol: 'openai' as const,
        baseUrl: 'https://api.moonshot.cn/v1', model: 'kimi-k3', models: ['kimi-k3'],
      },
      'kimi-2': {
        id: 'kimi-2', name: 'Kimi 私有端点', protocol: 'openai' as const,
        baseUrl: 'https://kimi.example/v1', model: 'kimi-k3-thinking', models: ['kimi-k3-thinking'], requiresKey: false,
      },
    };
    const candidates = runtimeModelCandidates([
      { slug: 'moonshot', name: 'Moonshot', models: ['kimi-k3', 'kimi-k3-thinking', 'kimi-k3-long'], authenticated: true },
    ], configured);
    expect(candidates[0]).toEqual(expect.objectContaining({
      settingsProviderId: 'kimi', models: ['kimi-k3-long'], providerConfigured: true,
    }));
  });

  it('creates a selected-only provider config from a Runtime candidate', () => {
    const provider = providerFromRuntimeCandidate({
      settingsProviderId: 'openrouter',
      runtimeSlugs: ['openrouter'],
      name: 'OpenRouter',
      models: ['openai/gpt-5.6-sol', 'anthropic/claude-opus-5'],
      providerConfigured: false,
      authenticated: false,
    }, ['anthropic/claude-opus-5']);

    expect(provider).toEqual({
      id: 'openrouter',
      name: 'OpenRouter',
      protocol: 'openai',
      baseUrl: 'https://openrouter.ai/api/v1',
      model: 'anthropic/claude-opus-5',
      models: ['anthropic/claude-opus-5'],
    });
  });
});
