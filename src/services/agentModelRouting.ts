import type { ProviderConfig } from '../types';

/** Composer 模型清单：默认模型置顶，并与供应商配置清单去重。 */
export function configuredAgentModels(provider?: ProviderConfig): string[] {
  if (!provider) return [];
  return Array.from(
    new Set(
      [provider.model, ...(provider.models ?? [])]
        .map((model) => model.trim())
        .filter(Boolean),
    ),
  );
}

export function defaultAgentModel(provider?: ProviderConfig): string {
  return provider?.model.trim() || configuredAgentModels(provider)[0] || '';
}

/** 最近选择仍属于当前供应商清单时优先使用，否则安全回退供应商默认模型。 */
export function resolveAgentModel(
  provider?: ProviderConfig,
  rememberedModel?: string,
): string {
  const remembered = rememberedModel?.trim() ?? '';
  return configuredAgentModels(provider).includes(remembered)
    ? remembered
    : defaultAgentModel(provider);
}
