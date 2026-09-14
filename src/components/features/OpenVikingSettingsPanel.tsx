import { useEffect, useState } from 'react';
import { CheckCircle2, Database, Loader2, RefreshCw } from 'lucide-react';
import {
  disableOpenViking, getOpenVikingStatus, restartHermesRuntime,
  saveOpenVikingConfig, testOpenVikingConnection,
  type OpenVikingConfig, type OpenVikingStatus,
} from '../../services/tauri';

const defaults: OpenVikingConfig = { endpoint: 'http://127.0.0.1:1933', account: '', user: '', agent: 'hermes' };
const inputClass = 'w-full rounded-lg border border-[var(--border-default)] bg-[var(--bg-sunken)] px-3 py-2 text-sm text-[var(--text-primary)] outline-none focus:border-[var(--accent)] disabled:opacity-50';
const buttonClass = 'inline-flex items-center justify-center gap-2 rounded-lg border border-[var(--border-default)] px-3 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-50';

export default function OpenVikingSettingsPanel() {
  const [status, setStatus] = useState<OpenVikingStatus | null>(null);
  const [config, setConfig] = useState<OpenVikingConfig>(defaults);
  const [apiKey, setApiKey] = useState('');
  const [consent, setConsent] = useState(false);
  const [busy, setBusy] = useState<'load' | 'save' | 'test' | 'disable' | 'restart' | null>('load');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [probeMessage, setProbeMessage] = useState('');

  useEffect(() => {
    let cancelled = false;
    getOpenVikingStatus().then((value) => {
      if (cancelled) return;
      setStatus(value);
      setConfig(value.config);
      setConsent(value.activeProvider === 'openviking');
    }).catch((e: unknown) => {
      if (!cancelled) setError(e instanceof Error ? e.message : '无法读取 OpenViking 配置');
    }).finally(() => { if (!cancelled) setBusy(null); });
    return () => { cancelled = true; };
  }, []);

  const active = status?.activeProvider === 'openviking';
  const editable = !!status?.available && !busy;
  const updateField = (key: keyof OpenVikingConfig, value: string) => {
    setConfig((previous) => ({ ...previous, [key]: value }));
    setProbeMessage('');
    setMessage('');
    setError('');
  };
  const perform = async (action: Exclude<typeof busy, null>) => {
    if (busy) return;
    setBusy(action);
    setError('');
    setMessage('');
    try {
      if (action === 'test') {
        setProbeMessage('');
        const result = await testOpenVikingConnection(config, apiKey);
        setProbeMessage(`连接与访问权限检查通过 · OpenViking ${result.version}`);
      } else if (action === 'save') {
        const result = await saveOpenVikingConfig(config, apiKey, consent);
        // 保存与随后状态刷新分开，不能把已保存误报为失败。
        setStatus((previous) => previous ? { ...previous, config, activeProvider: 'openviking', keyConfigured: previous.keyConfigured || !!apiKey.trim() } : previous);
        setMessage(`配置已保存并启用。请在当前工作结束后重启 Hermes，使已有会话使用新配置。${result.credentialStorage?.includes('debug_fallback') ? ' 凭据使用本机 Debug 开发存储。' : ''}`);
      } else if (action === 'disable') {
        await disableOpenViking();
        setStatus((previous) => previous ? { ...previous, activeProvider: '' } : previous);
        setConsent(false);
        setMessage('已切回内置记忆，远端数据保留。请在当前工作结束后重启 Hermes，使已有会话停止使用 OpenViking。');
      } else if (action === 'restart') {
        await restartHermesRuntime();
        setMessage('Hermes 已重启，新会话将读取已保存的记忆配置。');
      } else {
        const value = await getOpenVikingStatus();
        setStatus(value);
        setConfig(value.config);
        setConsent(value.activeProvider === 'openviking');
        setProbeMessage('');
      }
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : '操作失败，请重试');
    } finally { setBusy(null); }
  };

  return (
    <div className="max-w-2xl space-y-5">
      <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5">
        <div className="flex items-start justify-between gap-4">
          <div className="flex gap-3">
            <div className="rounded-lg bg-[var(--accent-subtle)] p-2.5 text-[var(--accent)]"><Database size={20} /></div>
            <div>
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">OpenViking 记忆服务</h3>
              <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">连接已有的 OpenViking，让 Hermes 保存并检索跨会话记忆。</p>
            </div>
          </div>
          <span className="shrink-0 rounded-full bg-[var(--bg-sunken)] px-2.5 py-1 text-xs text-[var(--text-secondary)]">
            {busy === 'load' ? '读取中' : !status ? '未连接 Hermes' : active ? '配置已启用' : '未启用'}
          </span>
        </div>
        <p className="mt-4 text-xs leading-5 text-[var(--text-tertiary)]">
          请先部署并启动 OpenViking 服务。服务端的存储、Embedding 与模型配置由 OpenViking 管理。
          {status?.activeProvider && !active && ` 当前 Hermes 记忆提供者：${status.activeProvider}，启用后将切换为 OpenViking。`}
        </p>
      </section>

      {error && <div role="alert" className="rounded-lg border border-[var(--danger)] p-3 text-sm leading-6 text-[var(--danger)]">{error}</div>}
      {status && !status.available && <p role="alert" className="text-sm text-[var(--text-secondary)]">当前 Hermes 未提供 OpenViking 配置能力，请先在「Hermes 更新」中更新 Sidecar。</p>}
      {!status && <button className={buttonClass} disabled={!!busy} onClick={() => void perform('load')}><RefreshCw size={14} />重新读取</button>}

      <form className="space-y-5" onSubmit={(event) => { event.preventDefault(); void perform('save'); }}>
        <fieldset disabled={!editable} className="space-y-5">
          <div className="space-y-2">
            <label htmlFor="openviking-endpoint" className="block text-sm font-medium text-[var(--text-primary)]">服务地址</label>
            <input id="openviking-endpoint" type="url" required value={config.endpoint} onChange={(e) => updateField('endpoint', e.target.value)} className={inputClass} placeholder={defaults.endpoint} autoComplete="off" spellCheck={false} />
            <p className="text-xs text-[var(--text-tertiary)]">填写服务根地址，例如 http://127.0.0.1:1933。支持 HTTPS 和反向代理路径。</p>
          </div>
          <div className="space-y-2">
            <label htmlFor="openviking-key" className="block text-sm font-medium text-[var(--text-primary)]">API Key <span className="font-normal text-[var(--text-tertiary)]">{status?.keyConfigured ? '· 已保存' : '· 可选'}</span></label>
            <input id="openviking-key" type="password" value={apiKey} onChange={(e) => { setApiKey(e.target.value); setProbeMessage(''); setMessage(''); }} className={inputClass} placeholder={status?.keyConfigured ? '留空保留已有凭据；输入新 Key 替换' : '无鉴权的本地服务可留空'} autoComplete="new-password" spellCheck={false} />
            <p className="text-xs leading-5 text-[var(--text-tertiary)]">凭据由系统安全存储管理，重开页面不回显。连接测试使用当前输入或已保存的 Key。</p>
          </div>
          <details className="rounded-xl border border-[var(--border-default)] p-4">
            <summary className="cursor-pointer text-sm font-medium text-[var(--text-secondary)]">账户与 Agent 标识</summary>
            <div className="mt-4 space-y-4">
              <p className="text-xs leading-5 text-[var(--text-tertiary)]">使用 API Key 时由服务解析账户和用户。无 Key 时，留空使用 default。相同身份可共享记忆。</p>
              {([{ key: 'account', label: '账户', placeholder: 'default' }, { key: 'user', label: '用户', placeholder: 'default' }, { key: 'agent', label: 'Agent 标识', placeholder: 'hermes' }] as const).map(({ key, label, placeholder }) => (
                <div key={key} className="space-y-2"><label htmlFor={`openviking-${key}`} className="block text-xs text-[var(--text-secondary)]">{label}</label><input id={`openviking-${key}`} value={config[key]} placeholder={placeholder} onChange={(e) => updateField(key, e.target.value)} className={inputClass} autoComplete="off" spellCheck={false} /></div>
              ))}
            </div>
          </details>
          <label className="flex items-start gap-2.5 rounded-lg bg-[var(--bg-sunken)] p-3 text-xs leading-5 text-[var(--text-secondary)]">
            <input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} className="mt-1" />
            <span>允许 Hermes 将启用后的会话发送到此服务、自动提取并跨会话召回记忆。同一账户、用户和 Agent 可共享记忆，不按 SophoNote 项目隔离。</span>
          </label>
        </fieldset>
        <div className="flex flex-wrap items-center gap-2">
          <button type="button" className={buttonClass} disabled={!editable || !config.endpoint.trim()} onClick={() => void perform('test')}>{busy === 'test' ? <Loader2 size={14} className="animate-spin" /> : <CheckCircle2 size={14} />}测试连接</button>
          <button type="submit" disabled={!editable || !consent || !config.endpoint.trim()} className="inline-flex items-center gap-2 rounded-lg bg-[var(--accent)] px-4 py-2 text-xs font-medium text-white disabled:opacity-50">{busy === 'save' && <Loader2 size={14} className="animate-spin" />}{active ? '保存配置' : '保存并启用'}</button>
          {active && <button type="button" className={buttonClass} disabled={!!busy} onClick={() => void perform('disable')}>停用 OpenViking</button>}
        </div>
      </form>
      {probeMessage && <p role="status" className="text-sm text-[var(--success)]">{probeMessage}</p>}
      {message && <p role="status" className="rounded-lg bg-[var(--accent-subtle)] p-3 text-sm leading-6 text-[var(--text-secondary)]">{message}</p>}
      <div className="flex flex-wrap items-center justify-between gap-3 border-t border-[var(--border-default)] pt-4">
        <p className="max-w-sm text-xs leading-5 text-[var(--text-tertiary)]">重启会中断正在执行的 Hermes 工作，请在会话和计划任务结束后操作。停用保留已有远端记忆。</p>
        <button type="button" className={buttonClass} disabled={!!busy || !status} onClick={() => void perform('restart')}><RefreshCw size={14} className={busy === 'restart' ? 'animate-spin' : ''} />重启 Hermes</button>
      </div>
    </div>
  );
}
