import type { ProviderConfig } from '../types';

export interface RuntimeModelProviderLike {
  slug: string;
  name: string;
  models: string[];
  authenticated: boolean | null;
}

export interface RuntimeModelCandidate {
  settingsProviderId: string;
  runtimeSlugs: string[];
  name: string;
  models: string[];
  providerConfigured: boolean;
  authenticated: boolean | null;
}

export interface ModelProviderPreset extends ProviderConfig {
  description: string;
  completionCompatible: boolean;
}

export const MODEL_PROVIDER_PRESETS: ModelProviderPreset[] = [
  {
    id: 'openrouter',
    name: 'OpenRouter',
    protocol: 'openai',
    baseUrl: 'https://openrouter.ai/api/v1',
    model: 'openai/gpt-5.6-sol',
    models: ['openai/gpt-5.6-sol', 'anthropic/claude-opus-5'],
    description: 'OpenRouter OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    protocol: 'openai',
    baseUrl: 'https://api.deepseek.com/v1',
    model: 'deepseek-v4-pro',
    models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
    description: 'DeepSeek 官方 OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'kimi',
    name: 'Kimi / 月之暗面',
    protocol: 'openai',
    baseUrl: 'https://api.moonshot.cn/v1',
    model: 'kimi-k3',
    models: ['kimi-k3', 'kimi-k2.7-code', 'kimi-k2.6', 'kimi-k2.5'],
    description: 'Moonshot 官方 OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'alibaba',
    name: '阿里云百炼 / 千问',
    protocol: 'openai',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen3.7-plus',
    models: ['qwen3.7-plus', 'qwen3.6-plus', 'qwen3.5-plus', 'qwen3-coder-plus'],
    description: 'DashScope OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'zai',
    name: '智谱 GLM',
    protocol: 'openai',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    model: 'glm-5.2',
    models: ['glm-5.2', 'glm-5.1', 'glm-5', 'glm-4.7', 'glm-4.5-flash'],
    description: '智谱 BigModel OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'minimax-cn',
    name: 'MiniMax',
    protocol: 'anthropic',
    baseUrl: 'https://api.minimaxi.com/anthropic',
    model: 'MiniMax-M3',
    models: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.5', 'MiniMax-M2.1'],
    description: 'MiniMax Anthropic 原生接口',
    completionCompatible: false,
  },
  {
    id: 'stepfun',
    name: '阶跃星辰 StepFun',
    protocol: 'openai',
    baseUrl: 'https://api.stepfun.com/step_plan/v1',
    model: 'step-3.5-flash',
    models: ['step-3.5-flash', 'step-3.5-flash-2603'],
    description: 'Step Plan OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'xiaomi',
    name: '小米 MiMo',
    protocol: 'openai',
    baseUrl: 'https://api.xiaomimimo.com/v1',
    model: 'mimo-v2.5-pro',
    models: ['mimo-v2.5-pro', 'mimo-v2.5', 'mimo-v2-pro', 'mimo-v2-omni', 'mimo-v2-flash'],
    description: '小米 MiMo 官方 OpenAI 兼容接口',
    completionCompatible: true,
  },
  {
    id: 'ollama',
    name: 'Ollama 本地',
    protocol: 'openai',
    baseUrl: 'http://localhost:11434/v1',
    model: 'qwen3:8b',
    models: ['qwen3:8b'],
    description: 'Ollama 本地推理（OpenAI 兼容，无需鉴权）',
    completionCompatible: true,
    requiresKey: false,
  },
  {
    id: 'private-openai',
    name: '私有化 / 本地部署',
    protocol: 'openai',
    baseUrl: 'http://localhost:8000/v1',
    model: '',
    models: [],
    description: '自建 OpenAI 兼容端点（vLLM、LM Studio 等，无需鉴权）',
    completionCompatible: true,
    requiresKey: false,
  },
];

export function providerFromPreset(preset: ModelProviderPreset): ProviderConfig {
  return {
    id: preset.id,
    name: preset.name,
    protocol: preset.protocol,
    baseUrl: preset.baseUrl,
    model: preset.model,
    models: [...preset.models],
    ...(preset.requiresKey === false ? { requiresKey: false } : {}),
  };
}

export function modelProviderPreset(id: string): ModelProviderPreset | undefined {
  const direct = MODEL_PROVIDER_PRESETS.find((preset) => preset.id === id);
  if (direct) return direct;
  // 同族多实例（ollama-2 等）回退到基础预设元数据
  const dash = id.lastIndexOf('-');
  if (dash > 0 && /^\d+$/.test(id.slice(dash + 1))) {
    return MODEL_PROVIDER_PRESETS.find((preset) => preset.id === id.slice(0, dash));
  }
  return undefined;
}

/** 返回同族配置的基础 id（如 ollama-2 → ollama）。 */
export function modelProviderFamilyId(id: string): string {
  return modelProviderPreset(id)?.id ?? id;
}

/** 同一供应商允许保存多份配置；每份配置的端点、鉴权模式和凭据均独立。 */
export function modelProviderFamilyInstances(
  providers: Record<string, ProviderConfig>,
  familyId: string,
): ProviderConfig[] {
  return Object.values(providers).filter(
    (provider) => modelProviderFamilyId(provider.id) === familyId,
  );
}

/** 同族多实例唯一 id：首个实例用基础 id，其后依次 -2、-3……（如本地免鉴权 + 远程带 Key 并存）。 */
export function uniqueProviderId(
  providers: Record<string, ProviderConfig>,
  baseId: string,
): string {
  if (!providers[baseId]) return baseId;
  for (let n = 2; ; n += 1) {
    const candidate = `${baseId}-${n}`;
    if (!providers[candidate]) return candidate;
  }
}

const RUNTIME_PROVIDER_SETTINGS_IDS: Record<string, string> = {
  moonshot: 'kimi',
  moonshotai: 'kimi',
  'kimi-coding': 'kimi',
  dashscope: 'alibaba',
  aliyun: 'alibaba',
  qwen: 'alibaba',
  glm: 'zai',
  zhipu: 'zai',
  'z-ai': 'zai',
  minimax: 'minimax-cn',
};

export function runtimeSettingsProviderId(runtimeSlug: string): string {
  const normalized = runtimeSlug.trim().toLowerCase();
  return RUNTIME_PROVIDER_SETTINGS_IDS[normalized] ?? normalized;
}

export function runtimeModelCandidates(
  runtimeProviders: RuntimeModelProviderLike[],
  configuredProviders: Record<string, ProviderConfig>,
): RuntimeModelCandidate[] {
  const candidates = new Map<string, RuntimeModelCandidate>();

  for (const runtimeProvider of runtimeProviders) {
    if (runtimeProvider.slug.trim().toLowerCase() === 'moa') continue;
    const settingsProviderId = runtimeSettingsProviderId(runtimeProvider.slug);
    const configuredInstances = modelProviderFamilyInstances(configuredProviders, settingsProviderId);
    const configured = configuredInstances[0];
    const configuredModels = new Set(
      configuredInstances.flatMap((provider) => [provider.model, ...provider.models].filter(Boolean)),
    );
    const missingModels = Array.from(new Set(runtimeProvider.models.filter(Boolean)))
      .filter((model) => !configuredModels.has(model));
    if (missingModels.length === 0) continue;

    const existing = candidates.get(settingsProviderId);
    if (existing) {
      existing.runtimeSlugs = Array.from(new Set([...existing.runtimeSlugs, runtimeProvider.slug]));
      existing.models = Array.from(new Set([...existing.models, ...missingModels]));
      if (runtimeProvider.authenticated === true) existing.authenticated = true;
      continue;
    }

    candidates.set(settingsProviderId, {
      settingsProviderId,
      runtimeSlugs: [runtimeProvider.slug],
      name: configured?.name ?? modelProviderPreset(settingsProviderId)?.name ?? runtimeProvider.name ?? runtimeProvider.slug,
      models: missingModels,
      providerConfigured: configuredInstances.length > 0,
      authenticated: runtimeProvider.authenticated,
    });
  }

  return Array.from(candidates.values()).sort((left, right) => {
    if (left.providerConfigured !== right.providerConfigured) return left.providerConfigured ? -1 : 1;
    return left.name.localeCompare(right.name, 'zh-CN');
  });
}

export function providerFromRuntimeCandidate(
  candidate: RuntimeModelCandidate,
  models: string[],
): ProviderConfig {
  const selectedModels = Array.from(new Set(models.filter(Boolean)));
  const preset = modelProviderPreset(candidate.settingsProviderId);
  return {
    id: candidate.settingsProviderId,
    name: preset?.name ?? candidate.name,
    protocol: preset?.protocol ?? 'openai',
    baseUrl: preset?.baseUrl ?? '',
    model: selectedModels[0] ?? '',
    models: selectedModels,
  };
}

export function orderModelProviders(
  providers: Record<string, ProviderConfig>,
  activeProviderId: string,
): ProviderConfig[] {
  return Object.values(providers).sort((left, right) => {
    if (left.id === activeProviderId) return -1;
    if (right.id === activeProviderId) return 1;
    return left.name.localeCompare(right.name, 'zh-CN');
  });
}

export type ProviderCredentialState = 'configured' | 'missing' | 'unchecked';

export function providerCredentialState(
  apiKeys: Record<string, string>,
  providerId: string,
): ProviderCredentialState {
  if (apiKeys[providerId] === undefined) return 'unchecked';
  return apiKeys[providerId] ? 'configured' : 'missing';
}

/** 供应商是否需要 API Key：仅 requiresKey === false 视为免鉴权（本地/私有化端点），缺省 true。 */
export function providerRequiresKey(provider: Pick<ProviderConfig, 'requiresKey'> | null | undefined): boolean {
  return provider?.requiresKey !== false;
}

/** 供应商凭据是否就绪：免鉴权供应商直接就绪；其余需要 Keychain 已配置。 */
export function providerCredentialReady(
  provider: Pick<ProviderConfig, 'id' | 'requiresKey'>,
  apiKeys: Record<string, string>,
): boolean {
  if (!providerRequiresKey(provider)) return true;
  return providerCredentialState(apiKeys, provider.id) === 'configured';
}

/** 主供应商列表只展示真正可用的配置；预设和未完成草稿留在“添加供应商”。 */
export function providerConfigurationReady(
  provider: ProviderConfig,
  apiKeys: Record<string, string>,
): boolean {
  if (!provider.baseUrl.trim() || !provider.model.trim()) return false;
  if (!provider.models.some((model) => model.trim() === provider.model.trim())) return false;
  return providerCredentialReady(provider, apiKeys);
}
