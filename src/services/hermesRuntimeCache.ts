import type { HermesCapabilities, HermesModelOptions } from './tauri';

/**
 * Hermes 模型/能力会话级快照。页签卸载后热切换用上次结果画首帧，
 * 避免挂载后再请求导致 MutationObserver settle 被二次 setState 拉长。
 */
let models: HermesModelOptions | null = null;
let capabilities: HermesCapabilities | null = null;

export function peekHermesModelOptions(): HermesModelOptions | null {
  return models;
}

export function rememberHermesModelOptions(next: HermesModelOptions | null): void {
  models = next;
}

export function peekHermesCapabilities(): HermesCapabilities | null {
  return capabilities;
}

export function rememberHermesCapabilities(next: HermesCapabilities | null): void {
  capabilities = next;
}

export function resolveStoredHermesSelection(
  options: HermesModelOptions | null,
): { provider: string; model: string } {
  if (!options) return { provider: '', model: '' };
  const configured = options.providers.filter(
    (provider) => provider.authenticated === true && provider.models.length > 0,
  );
  const storedProvider = typeof window !== 'undefined'
    ? window.localStorage.getItem('sophonote.hermes.provider') ?? ''
    : '';
  const storedModel = typeof window !== 'undefined'
    ? window.localStorage.getItem('sophonote.hermes.model') ?? ''
    : '';
  const storedRow = configured.find((provider) => provider.slug === storedProvider);
  const provider = storedRow?.models.includes(storedModel)
    ? storedRow
    : configured.find((item) => item.slug === options.provider) ?? configured[0];
  const model = storedRow?.models.includes(storedModel)
    ? storedModel
    : provider?.slug === options.provider && provider.models.includes(options.model ?? '')
      ? (options.model ?? '')
      : provider?.models[0] ?? '';
  return { provider: provider?.slug ?? '', model };
}

export function resetHermesRuntimeCacheForTests(): void {
  models = null;
  capabilities = null;
}
