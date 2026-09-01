import { useAgentStore, type AgentEvent } from '../stores/agentStore';
import {
  hermesModelOptions,
  type HermesModelOptions,
  type HermesModelProvider,
} from './tauri';

export type DiscoveryAnalysisMode = 'quick' | 'deep';

interface DiscoveryHermesModel {
  provider: string;
  model: string;
}

function isUsableProvider(provider: HermesModelProvider): boolean {
  return provider.authenticated === true
    && provider.slug.toLowerCase() !== 'moa'
    && provider.models.length > 0;
}

/**
 * 发现工作流固定走一个真实、已认证的 Hermes Provider。`moa/default` 是
 * Runtime 的虚拟编排入口，不是可回退的基础模型；把它当默认值会继续要求
 * 未配置的 OpenRouter aggregator，正是“重新生成”原报错的来源。
 */
export function resolveDiscoveryHermesModel(
  options: HermesModelOptions,
): DiscoveryHermesModel | null {
  const providers = options.providers.filter(isUsableProvider);
  const deepseek = providers.find((provider) => provider.slug.toLowerCase() === 'deepseek');
  const flash = deepseek?.models.find((model) =>
    model.toLowerCase().includes('deepseek-v4-flash'),
  );
  if (deepseek && flash) {
    return { provider: deepseek.slug, model: flash };
  }

  const current = providers.find((provider) => provider.slug === options.provider);
  if (current && options.model && current.models.includes(options.model)) {
    return { provider: current.slug, model: options.model };
  }

  const fallback = providers[0];
  return fallback
    ? { provider: fallback.slug, model: fallback.models[0] }
    : null;
}

export function discoveryAgentErrorMessage(error: unknown): string {
  const raw = error instanceof Error ? error.message : String(error || '');
  if (/No LLM provider configured|moa_aggregator|Run:\s*hermes setup|provider=openrouter/i.test(raw)) {
    return 'Hermes 当前没有可用的模型，请到「设置 → AI 模型」完成配置后重试。';
  }
  return raw || 'Hermes Agent 生成失败，请稍后重试。';
}

function terminalEvent(events: AgentEvent[]): AgentEvent | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (
      event.payload.type === 'run_completed' ||
      event.payload.type === 'run_failed' ||
      event.payload.type === 'run_cancelled'
    ) {
      return event;
    }
  }
  return null;
}

/**
 * 发现卡片的 AI 入口。只提交稳定动作引用；Skill 自己通过受限 Bridge 读取
 * evidence、生成并保存结果。React 不携带正文，也不解析自然语言回答落库。
 */
async function runDiscoverySkill(message: string, timeoutMs: number): Promise<void> {
  let model: DiscoveryHermesModel | null = null;
  try {
    model = resolveDiscoveryHermesModel(await hermesModelOptions());
  } catch (error) {
    throw new Error(discoveryAgentErrorMessage(error));
  }
  if (!model) {
    throw new Error('Hermes 暂未读取到已认证的模型，请到「设置 → AI 模型」完成配置后重试。');
  }

  const started = await useAgentStore.getState().startRun(
    null,
    message,
    undefined,
    null,
    'sophonote-ai-radar',
    undefined,
    [],
    model.model,
    model.provider,
  );
  if (!started) {
    throw new Error('Hermes Agent 未能启动发现解读');
  }

  await new Promise<void>((resolve, reject) => {
    let settled = false;
    const finish = (event: AgentEvent) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      unsubscribe();
      if (event.payload.type === 'run_completed') {
        resolve();
      } else if (event.payload.type === 'run_failed') {
        reject(new Error(discoveryAgentErrorMessage(event.payload.error || 'Hermes Agent 解读失败')));
      } else if (event.payload.type === 'run_cancelled') {
        reject(new Error(event.payload.reason || 'Hermes Agent 解读已取消'));
      } else {
        reject(new Error('Hermes Agent 返回了无效终态'));
      }
    };
    const inspect = () => {
      const event = terminalEvent(
        useAgentStore.getState().eventsByRunId[started.runId] ?? [],
      );
      if (event) finish(event);
    };
    const unsubscribe = useAgentStore.subscribe(inspect);
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      unsubscribe();
      reject(new Error('Hermes Agent 发现任务超时，请稍后重试或到会话查看运行状态'));
    }, timeoutMs);
    inspect();
  });
}

export async function runHermesDiscoveryAnalysis(
  itemId: string,
  mode: DiscoveryAnalysisMode,
  options?: { regenerate?: boolean },
): Promise<void> {
  const message = [
    `action=${mode}`,
    `itemId=${itemId}`,
    options?.regenerate ? 'regenerate=true' : null,
    'language=zh-CN',
  ].filter(Boolean).join(' ');
  return runDiscoverySkill(message, 180_000);
}
