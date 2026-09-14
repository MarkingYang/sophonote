/** Pi's agent loop owns reasoning; every filesystem/command effect returns to Rust. */
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { Type } from 'typebox';

export default function (pi: ExtensionAPI) {
  const model = JSON.parse(process.env.SOPHONOTE_PI_MODEL ?? '{}');
  pi.registerProvider('sophonote', {
    baseUrl: model.baseUrl,
    api: model.api,
    apiKey: process.env.SOPHONOTE_PI_MODEL_KEY ?? 'local-no-key',
    models: [{
      id: model.id, name: model.id, reasoning: false, input: ['text', 'image'],
      contextWindow: 128000, maxTokens: 8192,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    }],
  });

  pi.on('before_agent_start', () => {
    const content = process.env.SOPHONOTE_PI_CONTEXT;
    return content ? { message: { customType: 'sophonote-context', content, display: false } } : undefined;
  });
  const definitions = [
    { name: 'read', description: 'Read a text file in the authorized workspace or an explicitly attached file.',
      parameters: Type.Object({ path: Type.String(), offset: Type.Optional(Type.Number()), limit: Type.Optional(Type.Number()) }) },
    { name: 'ls', description: 'List files in an authorized directory.', parameters: Type.Object({ path: Type.Optional(Type.String()) }) },
    { name: 'write', description: 'Write a UTF-8 file. Host permission checks apply. Document working copies become reviewable patches.',
      parameters: Type.Object({ path: Type.String(), content: Type.String() }) },
    { name: 'edit', description: 'Replace exactly one occurrence of oldText in a UTF-8 file. Fails on ambiguous or missing text.',
      parameters: Type.Object({ path: Type.String(), oldText: Type.String(), newText: Type.String() }) },
    ...(process.platform === 'darwin' ? [{ name: 'bash', description: 'Run a command in the authorized workspace after user approval. Maximum 60 seconds; protected application data cannot be modified.',
      parameters: Type.Object({ command: Type.String(), timeout: Type.Optional(Type.Number()) }) }] : []),
  ];
  for (const definition of definitions) {
    pi.registerTool({
      ...definition, label: definition.name,
      async execute(_callId, args, _signal, _onUpdate, ctx) {
        const raw = await ctx.ui.input('sophonote.tool', JSON.stringify({ name: definition.name, args }));
        if (!raw) throw new Error('宿主已取消工具操作');
        const result = JSON.parse(raw);
        if (!result.ok) throw new Error(result.error ?? '工具执行失败');
        return { content: [{ type: 'text', text: result.text }], details: {} };
      },
    });
  }
  pi.registerCommand('sophonote_host_ready', { description: 'Host protocol capability check', handler: async () => {} });
  let watcher: ReturnType<typeof setInterval> | undefined;
  pi.on('session_start', () => {
    const expected = Number(process.env.SOPHONOTE_PI_PARENT);
    if (expected) watcher = setInterval(() => { if (process.ppid !== expected) process.exit(1); }, 1000);
  });
  pi.on('session_shutdown', () => { if (watcher) clearInterval(watcher); });
  // Do not permit loading workspace extensions or switching session/config through model tools.
  pi.on('project_trust', () => ({ trusted: false }));
}
