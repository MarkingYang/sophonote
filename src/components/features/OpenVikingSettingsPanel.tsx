import { useEffect, useState } from 'react';
import { CheckCircle2, Cloud, Loader2, RefreshCw } from 'lucide-react';
import {
  disableOpenViking, getOpenVikingStatus, restartHermesRuntime,
  saveOpenVikingConfig, testOpenVikingConnection,
  type OpenVikingStatus, type OpenVikingEngineProbe,
} from '../../services/tauri';
import OpenVikingMemoryBrowser from './OpenVikingMemoryBrowser';

const cloudConfig = { endpoint: 'https://api.vikingdb.cn-beijing.volces.com/openviking', account: '', user: '', agent: 'hermes' };
const buttonClass = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--border-default)] px-3 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40';

export default function OpenVikingSettingsPanel({ onDirtyChange }: { onDirtyChange?: (dirty: boolean) => void }) {
  const [status, setStatus] = useState<OpenVikingStatus | null>(null);
  const [apiKey, setApiKey] = useState('');
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState<'load' | 'save' | 'test' | 'disable' | 'restart' | null>('load');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [probeMessage, setProbeMessage] = useState('');
  const [engineProbes, setEngineProbes] = useState<OpenVikingEngineProbe[]>([]);
  const [refreshToken, setRefreshToken] = useState(0);
  const [memoryDirty, setMemoryDirty] = useState(false);
  useEffect(() => { onDirtyChange?.(memoryDirty); return () => onDirtyChange?.(false); }, [memoryDirty, onDirtyChange]);

  useEffect(() => {
    let cancelled = false;
    getOpenVikingStatus().then((value) => {
      if (cancelled) return;
      setStatus(value); setConsent(value.activeProvider === 'openviking' && !!value.hostEnginesEnabled);
    }).catch((e: unknown) => {
      if (!cancelled) setError(e instanceof Error ? e.message : '无法读取记忆配置');
    }).finally(() => { if (!cancelled) setBusy(null); });
    return () => { cancelled = true; };
  }, []);

  const active = status?.activeProvider === 'openviking';
  const editable = !!status?.available && !busy && !memoryDirty;
  const hasKey = !!apiKey.trim() || !!status?.keyConfigured;
  const perform = async (action: Exclude<typeof busy, null>) => {
    if (busy) return;
    setBusy(action); setError(''); setMessage('');
    try {
      if (action === 'test') {
        setProbeMessage('');
        setEngineProbes([]);
        const result = await testOpenVikingConnection(cloudConfig, apiKey);
        setEngineProbes(result.engines);
        setProbeMessage('四引擎连接检查完成；此检查不写入记忆，不代表自动提取或模型召回已通过。');
      } else if (action === 'save') {
        const result = await saveOpenVikingConfig(cloudConfig, apiKey, consent, true);
        setStatus((previous) => previous ? { ...previous, config: cloudConfig, activeProvider: 'openviking', keyConfigured: true, hostEnginesEnabled: true } : previous);
        setApiKey(''); setProbeMessage(''); setEngineProbes([]); setRefreshToken((v) => v + 1);
        setMessage(`配置已保存并启用；Pi、Claude Code、OpenCode 从下一轮生效，Hermes 重启后生效。本地记忆已关闭。${result.credentialStorage?.includes('debug_fallback') ? '当前 Debug 使用开发凭据存储。' : ''}`);
      } else if (action === 'disable') {
        await disableOpenViking();
        setStatus((previous) => previous ? { ...previous, activeProvider: '' } : previous);
        setConsent(false);
        setMessage('自动记忆已停用；本地记忆保持关闭，远端数据保留。重启 Hermes 后生效。');
      } else if (action === 'restart') {
        await restartHermesRuntime();
        setMessage('Hermes 已重启，记忆配置已重新加载。');
      } else {
        const value = await getOpenVikingStatus(); setStatus(value); setConsent(value.activeProvider === 'openviking' && !!value.hostEnginesEnabled);
      }
    } catch (e) { setError(e instanceof Error ? e.message : '操作失败，请重试'); }
    finally { setBusy(null); }
  };

  return <div className="space-y-5">
    <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5">
      <div className="mb-5 flex items-start justify-between gap-4">
        <div className="flex gap-3"><span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[var(--accent-subtle)] text-[var(--accent)]"><Cloud size={20} /></span><div>
          <h3 className="text-base font-semibold">OpenViking <span className="ml-1 text-xs font-normal text-[var(--text-tertiary)]">火山引擎云</span></h3>
          <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">将长期记忆保存在自己的云端服务，使用 API Key 连接。</p>
        </div></div>
        <span className="shrink-0 text-xs text-[var(--text-tertiary)]">{busy === 'load' ? '读取配置…' : active ? '已配置自动记忆' : '自动记忆未启用'}</span>
      </div>
      {error && <p role="alert" className="mb-4 rounded-lg bg-[var(--danger-subtle)] p-3 text-sm text-[var(--danger)]">{error}</p>}
      {!status && !busy && <button type="button" className={buttonClass} onClick={() => void perform('load')}>重新读取</button>}
      <form onSubmit={(e) => { e.preventDefault(); if (editable && consent && hasKey) void perform('save'); }}>
        <fieldset disabled={!editable} className="space-y-4">
          <div>
            <label htmlFor="openviking-key" className="mb-2 block text-sm font-medium">API Key <span className="font-normal text-[var(--text-tertiary)]">{status?.keyConfigured ? '· 已保存' : '· 未配置'}</span></label>
            <input id="openviking-key" type="password" value={apiKey} onChange={(e) => { setApiKey(e.target.value); setProbeMessage(''); setEngineProbes([]); setMessage(''); }} className="input font-mono text-sm" placeholder={status?.keyConfigured ? '留空保留已有凭据；输入新 Key 替换' : '填写 OpenViking 服务端 API Key'} autoComplete="off" spellCheck={false} />
            <p className="mt-2 text-xs text-[var(--text-tertiary)]">凭据由宿主保存，页面不回显。服务地址已配置为火山引擎北京区。</p>
          </div>
          <label className="flex items-start gap-2.5 rounded-lg bg-[var(--bg-sunken)] p-3 text-xs leading-5 text-[var(--text-secondary)]">
            <input type="checkbox" checked={consent} onChange={(e) => setConsent(e.target.checked)} className="mt-1" />
            <span>允许 Hermes、Pi、Claude Code、OpenCode 将启用后的会话发送至我的云端服务，自动提取并跨会话召回记忆。公共用户记忆共享，引擎专属记忆分开，不按项目隔离。</span>
          </label>
        </fieldset>
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <button type="button" className={buttonClass} disabled={!editable || !hasKey} onClick={() => void perform('test')}>{busy === 'test' ? <Loader2 size={14} className="animate-spin" /> : <CheckCircle2 size={14} />}测试连接</button>
          <button type="submit" disabled={!editable || !consent || !hasKey} className="inline-flex items-center gap-2 rounded-lg bg-[var(--accent)] px-4 py-2 text-xs font-medium text-white disabled:opacity-40">{busy === 'save' && <Loader2 size={14} className="animate-spin" />}{active ? '保存配置' : '保存并启用'}</button>
          {active && <button type="button" className={buttonClass} disabled={!editable} onClick={() => void perform('disable')}>停用自动记忆</button>}
        </div>
      </form>
      {active && !status?.hostEnginesEnabled && <p className="mt-3 text-xs text-[var(--warning)]">旧配置仅启用 Hermes；确认会话外发并保存后，另外三个引擎才会启用。</p>}
      {engineProbes.length > 0 && <ul className="mt-3 grid gap-2 text-xs sm:grid-cols-2">{engineProbes.map((probe) => <li key={probe.engine} className="rounded-lg bg-[var(--bg-sunken)] p-3"><strong>{{hermes:'Hermes',pi:'Pi',claude_code:'Claude Code',opencode:'OpenCode'}[probe.engine] ?? probe.engine}</strong><span className={probe.success ? 'ml-2 text-[var(--success)]' : 'ml-2 text-[var(--danger)]'}>{probe.success ? '通过' : '失败'}</span><p className="mt-1 text-[var(--text-tertiary)]">{probe.message}</p></li>)}</ul>}
      {probeMessage && <p role="status" className="mt-3 text-sm text-[var(--success)]">{probeMessage}</p>}
      {message && <p role="status" className="mt-3 rounded-lg bg-[var(--accent-subtle)] p-3 text-sm leading-6 text-[var(--text-secondary)]">{message}</p>}
      <div className="mt-5 flex flex-wrap items-center justify-between gap-3 border-t border-[var(--border-default)] pt-4">
        <p className="max-w-lg text-xs leading-5 text-[var(--text-tertiary)]">四引擎共用此云端记忆库。Pi、Claude Code、OpenCode 由宿主同步成功轮次的用户消息和最终回复；Hermes 使用原生记忆插件，请在任务结束后重启生效。</p>
        <button type="button" className={buttonClass} disabled={!!busy || !status || memoryDirty} onClick={() => void perform('restart')}><RefreshCw size={14} className={busy === 'restart' ? 'animate-spin' : ''} />重启 Hermes</button>
      </div>
      {memoryDirty && <p className="mt-3 text-xs text-[var(--warning)]">请先保存或取消正在编辑的记忆，再修改连接配置。</p>}
    </section>
    <OpenVikingMemoryBrowser enabled={!!status?.keyConfigured} refreshToken={refreshToken} onDirtyChange={setMemoryDirty} />
  </div>;
}
