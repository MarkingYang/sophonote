import { useEffect, useState } from 'react';
import type { AgentEngine } from '../../stores/agentStore';
import * as tauri from '../../services/tauri';
import { resolveStoredHermesSelection } from '../../services/hermesRuntimeCache';

export function useSidecarConfig(engine: AgentEngine, modelConfig: unknown) {
  const [models, setModels] = useState<tauri.HermesModelOptions | null>(null);
  const [status, setStatus] = useState<tauri.PiRuntimeStatus | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [modelError, setModelError] = useState<string | null>(null);
  const [selection, setSelection] = useState({ provider: '', model: '' });
  const [revision, refresh] = useState(0);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [runtimeLoading, setRuntimeLoading] = useState(false);
  useEffect(() => {
    if (engine === 'hermes') return;
    let cancelled = false;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const label = engine === 'pi' ? 'Pi' : 'Claude Code';
    setModels(null); setStatus(null); setRuntimeError(null); setModelError(null);
    setSelection({ provider: '', model: '' });
    setModelsLoading(true); setRuntimeLoading(true);
    function load<T>(request: Promise<T>, accept: (value: T) => void, reject: (error: string) => void, done: () => void, kind: string) {
      let settled = false;
      const timer = setTimeout(() => {
        if (cancelled || settled) return;
        settled = true;
        reject(`${label} ${kind}读取超时，请在模型菜单中重试。`); done();
      }, 30_000);
      timers.push(timer);
      void request.then((value) => {
        if (!cancelled && !settled) accept(value);
      }, (error: unknown) => {
        if (!cancelled && !settled) reject(error instanceof Error ? error.message : String(error));
      }).finally(() => {
        clearTimeout(timer);
        if (!cancelled && !settled) { settled = true; done(); }
      });
    }
    load(engine === 'pi' ? tauri.piRuntimeStatus() : tauri.claudeRuntimeStatus(), (value) => {
      setStatus(value);
      if (!value.available) setRuntimeError(value.error ?? `${label} 未就绪`);
    }, setRuntimeError, () => setRuntimeLoading(false), '运行时状态');
    load(engine === 'pi' ? tauri.piModelOptions() : tauri.claudeModelOptions(), (options) => {
      setModels(options);
      setSelection(resolveStoredHermesSelection(options, true));
    }, setModelError, () => setModelsLoading(false), '模型配置');
    return () => { cancelled = true; timers.forEach(clearTimeout); };
  }, [engine, modelConfig, revision]);
  return { models, status, error: runtimeError ?? modelError, selection, setSelection, refresh, modelsLoading, runtimeLoading };
}
