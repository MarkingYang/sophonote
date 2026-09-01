import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Bot,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleDot,
  Eye,
  EyeOff,
  KeyRound,
  Layers3,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  X,
  XCircle,
  Zap,
} from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import { testConnection, testEmbeddingConnection } from '../../services/ai';
import {
  fetchProviderModels,
  getCompletionConfig,
  hermesModelCatalog,
  setCompletionEnabled,
  type HermesModelOptions,
} from '../../services/tauri';
import {
  MODEL_PROVIDER_PRESETS,
  modelProviderFamilyInstances,
  modelProviderPreset,
  orderModelProviders,
  providerConfigurationReady,
  providerFromRuntimeCandidate,
  providerCredentialReady,
  providerCredentialState,
  providerFromPreset,
  providerRequiresKey,
  runtimeModelCandidates,
  uniqueProviderId,
  type ProviderCredentialState,
  type RuntimeModelCandidate,
} from '../../services/modelProviders';
import type { ProviderConfig } from '../../types';

type TestStatus = { status: 'idle' | 'testing' | 'ok' | 'fail'; message?: string };
type SettingsSection = 'models' | 'embedding';

function Toggle({
  checked,
  disabled,
  label,
  onChange,
}: {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: () => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={onChange}
      className={`relative h-6 w-11 shrink-0 rounded-full transition-colors disabled:opacity-50 ${
        checked ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
      }`}
    >
      <span
        className={`absolute left-[3px] top-[3px] h-[18px] w-[18px] rounded-full bg-white shadow-sm transition-transform duration-200 ${
          checked ? 'translate-x-5' : 'translate-x-0'
        }`}
      />
    </button>
  );
}

function StatusMessage({ result }: { result: TestStatus }) {
  if ((result.status !== 'ok' && result.status !== 'fail') || !result.message) return null;
  const ok = result.status === 'ok';
  return (
    <div
      className={`flex items-start gap-2 rounded-lg px-3 py-2 text-xs leading-5 ${
        ok
          ? 'bg-[var(--success-subtle)] text-[var(--success)]'
          : 'bg-[var(--danger-subtle)] text-[var(--danger)]'
      }`}
    >
      {ok ? <CheckCircle2 size={14} className="mt-0.5 shrink-0" /> : <XCircle size={14} className="mt-0.5 shrink-0" />}
      <span className="min-w-0 break-words">{result.message}</span>
    </div>
  );
}

function CredentialBadge({ state, noAuth = false }: { state: ProviderCredentialState; noAuth?: boolean }) {
  if (noAuth) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-[var(--success)]">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--success)]" /> 免鉴权
      </span>
    );
  }
  if (state === 'configured') {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-[var(--success)]">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--success)]" /> 已配置
      </span>
    );
  }
  if (state === 'missing') {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-[var(--warning)]">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--warning)]" /> 待配置
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-[var(--text-tertiary)]">
      <CircleDot size={11} /> 打开后检查
    </span>
  );
}

function CredentialInput({
  providerId,
  providerName,
  onSaved,
}: {
  providerId: string;
  providerName: string;
  onSaved?: () => Promise<void>;
}) {
  const { apiKeys, setApiKey, ensureApiKeyLoaded } = useAppStore();
  const [value, setValue] = useState('');
  const [show, setShow] = useState(false);
  const [saveState, setSaveState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [message, setMessage] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);
  const credentialState = providerCredentialState(apiKeys, providerId);

  useEffect(() => {
    setValue('');
    setSaveState('idle');
    setMessage('');
    void ensureApiKeyLoaded(providerId);
  }, [ensureApiKeyLoaded, providerId]);

  const save = async () => {
    const key = value.trim();
    if (!key || saveState === 'saving') return;
    setSaveState('saving');
    setMessage('');
    try {
      await setApiKey(providerId, key);
      setValue('');
      setSaveState('saved');
      setMessage('凭据已安全保存到 macOS 钥匙串');
      await onSaved?.();
    } catch (error) {
      setSaveState('error');
      setMessage(error instanceof Error ? error.message : '保存失败，请重试');
    }
  };

  return (
    <div>
      <div className="flex gap-2">
        <div className="relative min-w-0 flex-1">
          <input
            ref={inputRef}
            id={`provider-key-${providerId}`}
            type={show ? 'text' : 'password'}
            value={value}
            onChange={(event) => {
              setValue(event.target.value);
              if (saveState !== 'idle') {
                setSaveState('idle');
                setMessage('');
              }
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault();
                void save();
              }
            }}
            placeholder={credentialState === 'configured' ? '输入新 Key 可替换现有凭据' : `输入 ${providerName} API Key`}
            autoComplete="off"
            className="input pr-10 font-mono text-[13px]"
          />
          <button
            type="button"
            onClick={() => setShow((current) => !current)}
            className="absolute right-2.5 top-1/2 -translate-y-1/2 rounded p-1 text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]"
            aria-label={show ? '隐藏 API Key' : '显示 API Key'}
          >
            {show ? <EyeOff size={15} /> : <Eye size={15} />}
          </button>
        </div>
        <button
          type="button"
          onClick={() => void save()}
          disabled={!value.trim() || saveState === 'saving'}
          className="inline-flex h-10 shrink-0 items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3.5 text-xs font-medium text-white transition-colors hover:bg-[var(--accent-strong)] disabled:opacity-40"
        >
          {saveState === 'saving' ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />}
          {saveState === 'saving' ? '保存中…' : onSaved ? '保存并验证' : '安全保存'}
        </button>
      </div>
      <div className="mt-2 flex min-h-5 items-start justify-between gap-3 text-xs">
        <CredentialBadge state={credentialState} />
        {message && (
          <span className={saveState === 'error' ? 'text-right text-[var(--danger)]' : 'text-right text-[var(--success)]'}>
            {message}
          </span>
        )}
      </div>
    </div>
  );
}

function ProviderCatalogDialog({
  open,
  providers,
  apiKeys,
  runtimeCandidates,
  runtimeState,
  onClose,
  onAdd,
  onEdit,
  onPickRuntime,
  onRefreshRuntime,
}: {
  open: boolean;
  providers: Record<string, ProviderConfig>;
  apiKeys: Record<string, string>;
  runtimeCandidates: RuntimeModelCandidate[];
  runtimeState: TestStatus;
  onClose: () => void;
  onAdd: (provider: ProviderConfig) => void;
  onEdit: (providerId: string) => void;
  onPickRuntime: (candidate: RuntimeModelCandidate) => void;
  onRefreshRuntime: () => void;
}) {
  const [query, setQuery] = useState('');
  const available = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized) return MODEL_PROVIDER_PRESETS;
    return MODEL_PROVIDER_PRESETS.filter((preset) =>
      `${preset.name} ${preset.description} ${preset.id}`.toLowerCase().includes(normalized),
    );
  }, [query]);

  const instancesFor = useCallback(
    (presetId: string) => modelProviderFamilyInstances(providers, presetId),
    [providers],
  );
  const runtimeModelCount = runtimeCandidates.reduce((sum, candidate) => sum + candidate.models.length, 0);
  const runtimeOnlyCandidates = runtimeCandidates.filter((candidate) => {
    if (modelProviderPreset(candidate.settingsProviderId)) return false;
    const normalized = query.trim().toLowerCase();
    return !normalized || `${candidate.name} ${candidate.settingsProviderId}`.toLowerCase().includes(normalized);
  });

  useEffect(() => {
    if (!open) setQuery('');
  }, [open]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-[var(--overlay-scrim)] p-6" onMouseDown={onClose}>
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="provider-catalog-title"
        onMouseDown={(event) => event.stopPropagation()}
        className="flex max-h-[min(720px,calc(100vh-48px))] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]"
      >
        <header className="flex items-start justify-between gap-4 border-b border-[var(--border-default)] px-5 py-4">
          <div>
            <h3 id="provider-catalog-title" className="text-base font-semibold text-[var(--text-primary)]">添加模型供应商</h3>
            <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">同一供应商可保留多份独立配置。</p>
          </div>
          <button type="button" onClick={onClose} className="rounded-lg p-1.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-primary)]" aria-label="关闭">
            <X size={18} />
          </button>
        </header>
        <div className="border-b border-[var(--border-default)] px-5 py-3">
          <div className="relative">
            <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} autoFocus placeholder="搜索厂商或协议" className="input pl-9" />
          </div>
          <div className="mt-2 flex items-center justify-between gap-3 text-xs text-[var(--text-tertiary)]">
            <span>
              {runtimeState.status === 'testing'
                ? 'Runtime 读取中…'
                : `Runtime 发现 ${runtimeCandidates.length} 个供应商 · ${runtimeModelCount} 个待配置模型`}
            </span>
            <button type="button" onClick={onRefreshRuntime} disabled={runtimeState.status === 'testing'} className="rounded p-1 hover:bg-[var(--bg-sunken)] disabled:opacity-40" aria-label="刷新 Runtime 模型目录">
              <RefreshCw size={13} className={runtimeState.status === 'testing' ? 'animate-spin' : ''} />
            </button>
          </div>
        </div>
        <div className="overflow-y-auto p-3">
          {available.length > 0 || runtimeOnlyCandidates.length > 0 ? (
            <div className="grid gap-2 sm:grid-cols-2">
              {available.map((preset) => {
                const instances = instancesFor(preset.id);
                const incomplete = instances.find((provider) => !providerConfigurationReady(provider, apiKeys));
                const runtimeCandidate = runtimeCandidates.find((candidate) => candidate.settingsProviderId === preset.id);
                return (
                <div
                  key={preset.id}
                  className="group flex min-h-28 items-start gap-3 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4 text-left transition-colors hover:border-[var(--accent-border)] hover:bg-[var(--accent-subtle)]"
                >
                  <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[var(--bg-sunken)] text-[var(--text-secondary)] group-hover:bg-[var(--bg-surface)] group-hover:text-[var(--accent)]">
                    <Bot size={19} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center justify-between gap-2">
                      <strong className="truncate text-sm font-semibold text-[var(--text-primary)]">{preset.name}</strong>
                      <button type="button" onClick={() => incomplete ? onEdit(incomplete.id) : onAdd(providerFromPreset(preset))} className="shrink-0 rounded-md px-2 py-1 text-[11px] font-medium text-[var(--accent)] hover:bg-[var(--bg-surface)]" aria-label={incomplete ? `继续配置 ${preset.name}` : `添加 ${preset.name}`}>
                        {incomplete ? '继续' : instances.length > 0 ? '新增' : '添加'}
                      </button>
                    </span>
                    <span className="mt-1 block text-xs leading-5 text-[var(--text-tertiary)]">{preset.description}</span>
                    <span className="mt-2 flex flex-wrap items-center gap-1.5">
                      <span className="inline-flex rounded-md bg-[var(--bg-sunken)] px-2 py-0.5 font-mono text-[11px] text-[var(--text-tertiary)]">
                        {preset.protocol === 'openai' ? 'OpenAI compatible' : 'Anthropic native'}
                      </span>
                      {preset.requiresKey === false && (
                        <span className="inline-flex rounded-md bg-[var(--success-subtle)] px-2 py-0.5 text-[11px] font-medium text-[var(--success)]">免鉴权</span>
                      )}
                      {instances.length > 0 && (
                        <span className="inline-flex rounded-md bg-[var(--accent-subtle)] px-2 py-0.5 text-[11px] font-medium text-[var(--accent)]">
                          已添加 {instances.length} 份
                        </span>
                      )}
                      {incomplete && <button type="button" onClick={() => onEdit(incomplete.id)} className="inline-flex rounded-md bg-[var(--warning-subtle)] px-2 py-0.5 text-[11px] font-medium text-[var(--warning)]">继续配置</button>}
                      {runtimeCandidate && <button type="button" onClick={() => onPickRuntime(runtimeCandidate)} className="inline-flex rounded-md bg-[var(--bg-sunken)] px-2 py-0.5 text-[11px] font-medium text-[var(--text-secondary)] hover:text-[var(--accent)]">Runtime {runtimeCandidate.models.length} 个模型</button>}
                    </span>
                  </span>
                </div>
              );})}
              {runtimeOnlyCandidates.map((candidate) => (
                <div key={candidate.settingsProviderId} className="flex min-h-28 items-start gap-3 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4 text-left hover:border-[var(--accent-border)] hover:bg-[var(--accent-subtle)]">
                  <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[var(--bg-sunken)] text-[var(--text-secondary)]"><Server size={18} /></span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center justify-between gap-2">
                      <strong className="truncate text-sm font-semibold text-[var(--text-primary)]">{candidate.name}</strong>
                      <button type="button" onClick={() => onPickRuntime(candidate)} className="shrink-0 rounded-md px-2 py-1 text-[11px] font-medium text-[var(--accent)] hover:bg-[var(--bg-surface)]">配置</button>
                    </span>
                    <span className="mt-1 block text-xs leading-5 text-[var(--text-tertiary)]">Runtime 模型供应商</span>
                    <span className="mt-2 inline-flex rounded-md bg-[var(--bg-sunken)] px-2 py-0.5 text-[11px] font-medium text-[var(--text-secondary)]">{candidate.models.length} 个模型</span>
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="flex min-h-44 flex-col items-center justify-center text-center">
              <CheckCircle2 size={24} className="text-[var(--success)]" />
              <p className="mt-3 text-sm font-medium text-[var(--text-primary)]">
                {query ? '没有匹配的供应商' : '没有可添加的供应商'}
              </p>
              <p className="mt-1 text-xs text-[var(--text-tertiary)]">调整搜索关键词，或在供应商列表中编辑已有配置。</p>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

function RuntimeModelCatalogDialog({
  candidate,
  onClose,
  onAdd,
}: {
  candidate: RuntimeModelCandidate | null;
  onClose: () => void;
  onAdd: (models: string[]) => void;
}) {
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());

  useEffect(() => {
    setQuery('');
    setSelected(new Set());
  }, [candidate?.settingsProviderId]);

  const filteredModels = useMemo(() => {
    if (!candidate) return [];
    const normalized = query.trim().toLowerCase();
    return normalized
      ? candidate.models.filter((model) => model.toLowerCase().includes(normalized))
      : candidate.models;
  }, [candidate, query]);

  if (!candidate) return null;
  const filteredAllSelected = filteredModels.length > 0 && filteredModels.every((model) => selected.has(model));
  const toggleModel = (model: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(model)) next.delete(model);
      else next.add(model);
      return next;
    });
  };
  const toggleFiltered = () => {
    setSelected((current) => {
      const next = new Set(current);
      if (filteredAllSelected) filteredModels.forEach((model) => next.delete(model));
      else filteredModels.forEach((model) => next.add(model));
      return next;
    });
  };

  return (
    <div className="fixed inset-0 z-[85] flex items-center justify-center bg-[var(--overlay-scrim)] p-6" onMouseDown={onClose}>
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby="runtime-model-catalog-title"
        onMouseDown={(event) => event.stopPropagation()}
        className="flex max-h-[min(760px,calc(100vh-48px))] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]"
      >
        <header className="flex items-center justify-between gap-4 border-b border-[var(--border-default)] px-5 py-4">
          <div className="min-w-0">
            <h3 id="runtime-model-catalog-title" className="truncate text-base font-semibold text-[var(--text-primary)]">补充模型</h3>
            <p className="mt-1 truncate text-xs text-[var(--text-tertiary)]">{candidate.name} · {candidate.models.length}</p>
          </div>
          <button type="button" onClick={onClose} className="rounded-lg p-1.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" aria-label="关闭">
            <X size={18} />
          </button>
        </header>
        <div className="flex items-center gap-2 border-b border-[var(--border-default)] px-5 py-3">
          <div className="relative min-w-0 flex-1">
            <Search size={15} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
            <input value={query} onChange={(event) => setQuery(event.target.value)} autoFocus placeholder="搜索模型" className="input pl-9" />
          </div>
          <button type="button" onClick={toggleFiltered} disabled={filteredModels.length === 0} className="shrink-0 rounded-lg border border-[var(--border-strong)] px-3 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40">
            {filteredAllSelected ? '取消全选' : '全选结果'}
          </button>
        </div>
        <div className="min-h-48 flex-1 overflow-y-auto p-2">
          {filteredModels.length > 0 ? filteredModels.map((model) => {
            const checked = selected.has(model);
            return (
              <button
                key={model}
                type="button"
                role="checkbox"
                aria-checked={checked}
                onClick={() => toggleModel(model)}
                className={`flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left hover:bg-[var(--bg-sunken)] ${checked ? 'bg-[var(--accent-subtle)]' : ''}`}
              >
                <span className={`flex size-4 shrink-0 items-center justify-center rounded border ${checked ? 'border-[var(--accent)] bg-[var(--accent)] text-white' : 'border-[var(--border-strong)]'}`}>
                  {checked && <Check size={11} />}
                </span>
                <span className="min-w-0 flex-1 truncate font-mono text-xs text-[var(--text-secondary)]">{model}</span>
              </button>
            );
          }) : (
            <div className="flex min-h-44 items-center justify-center text-sm text-[var(--text-tertiary)]">没有匹配模型</div>
          )}
        </div>
        <footer className="flex items-center justify-between gap-3 border-t border-[var(--border-default)] px-5 py-3">
          <span className="text-xs text-[var(--text-tertiary)]">已选 {selected.size}</span>
          <div className="flex items-center gap-2">
            <button type="button" onClick={onClose} className="rounded-lg border border-[var(--border-strong)] px-3.5 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]">取消</button>
            <button type="button" onClick={() => onAdd(Array.from(selected))} disabled={selected.size === 0} className="rounded-lg bg-[var(--accent)] px-3.5 py-2 text-xs font-medium text-white hover:bg-[var(--accent-strong)] disabled:opacity-40">加入配置</button>
          </div>
        </footer>
      </section>
    </div>
  );
}

function ProviderConfigDrawer({ providerId, onClose }: { providerId: string | null; onClose: () => void }) {
  const { settings, updateSettings, apiKeys, ensureApiKeyLoaded } = useAppStore();
  const providers = settings.aiConfig?.providers ?? {};
  const provider = providerId ? providers[providerId] : undefined;
  const [testState, setTestState] = useState<TestStatus>({ status: 'idle' });
  const [catalogState, setCatalogState] = useState<TestStatus>({ status: 'idle' });
  const [modelDraft, setModelDraft] = useState('');

  useEffect(() => {
    setTestState({ status: 'idle' });
    setCatalogState({ status: 'idle' });
    setModelDraft('');
    if (providerId) void ensureApiKeyLoaded(providerId);
  }, [ensureApiKeyLoaded, providerId]);

  if (!provider) return null;
  const preset = modelProviderPreset(provider.id);
  const isActive = settings.aiConfig?.activeProvider === provider.id;
  const credentialState = providerCredentialState(apiKeys, provider.id);
  const noAuth = !providerRequiresKey(provider);
  const credentialReady = noAuth || credentialState === 'configured';

  const updateProvider = (patch: Partial<ProviderConfig>) => {
    setTestState({ status: 'idle' });
    setCatalogState({ status: 'idle' });
    updateSettings({
      aiConfig: {
        ...settings.aiConfig,
        activeProvider: settings.aiConfig?.activeProvider ?? provider.id,
        providers: { ...providers, [provider.id]: { ...provider, ...patch } },
      },
    });
  };

  const toggleAuthentication = () => {
    updateProvider({ requiresKey: noAuth });
  };

  const runTest = async () => {
    if (provider.protocol !== 'openai') return;
    setTestState({ status: 'testing' });
    try {
      const { latencyMs } = await testConnection(provider.id);
      setTestState({ status: 'ok', message: `连接成功 · ${latencyMs} ms` });
    } catch (error) {
      setTestState({ status: 'fail', message: error instanceof Error ? error.message.slice(0, 500) : '连接失败' });
    }
  };

  const syncModels = async () => {
    setCatalogState({ status: 'testing' });
    try {
      const result = await fetchProviderModels(provider.id);
      const models = Array.from(new Set([provider.model, ...result.models, ...provider.models].filter(Boolean)));
      updateProvider({ models });
      setCatalogState({ status: 'ok', message: `已从供应商同步 ${result.models.length} 个模型` });
    } catch (error) {
      setCatalogState({ status: 'fail', message: error instanceof Error ? error.message.slice(0, 500) : '模型目录同步失败' });
    }
  };

  const addModel = () => {
    const model = modelDraft.trim();
    if (!model) return;
    updateProvider({
      model: provider.model || model,
      models: Array.from(new Set([...provider.models, model])),
    });
    setModelDraft('');
  };

  const removeModel = (model: string) => {
    if (provider.models.length <= 1) return;
    const models = provider.models.filter((candidate) => candidate !== model);
    updateProvider({ models, model: provider.model === model ? models[0] : provider.model });
  };

  const setActive = () => {
    if (!credentialReady) return;
    updateSettings({
      aiConfig: { ...settings.aiConfig, activeProvider: provider.id, providers },
    });
  };

  return (
    <div className="fixed inset-0 z-[70] bg-[var(--overlay-scrim)]" onMouseDown={onClose}>
      <aside
        role="dialog"
        aria-modal="true"
        aria-labelledby="provider-drawer-title"
        onMouseDown={(event) => event.stopPropagation()}
        className="ml-auto flex h-full w-[min(540px,calc(100vw-24px))] flex-col border-l border-[var(--border-default)] bg-[var(--bg-canvas)] shadow-[var(--shadow-lg)]"
      >
        <header className="flex items-start justify-between gap-4 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-5 py-4">
          <div className="flex min-w-0 items-start gap-3">
            <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[var(--accent-subtle)] text-[var(--accent)]">
              <Bot size={19} />
            </span>
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <h3 id="provider-drawer-title" className="truncate text-base font-semibold text-[var(--text-primary)]">{provider.name}</h3>
                {isActive && <span className="rounded-full bg-[var(--accent)] px-2 py-0.5 text-[11px] font-medium text-white">使用中</span>}
              </div>
              <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">{preset?.description ?? '自定义模型供应商配置'}</p>
            </div>
          </div>
          <button type="button" onClick={onClose} className="rounded-lg p-1.5 text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-primary)]" aria-label="关闭供应商配置">
            <X size={18} />
          </button>
        </header>

        <div className="flex-1 space-y-4 overflow-y-auto p-5">
          <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
            <div className="mb-3 flex items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <KeyRound size={15} className="text-[var(--accent)]" />
                <h4 className="text-sm font-semibold text-[var(--text-primary)]">需要鉴权</h4>
              </div>
              <button
                type="button"
                onClick={toggleAuthentication}
                className="shrink-0 text-xs font-medium text-[var(--accent)] hover:underline"
              >
                {noAuth ? '开启' : '关闭'}
              </button>
            </div>
            {noAuth ? (
              <p className="rounded-lg bg-[var(--bg-sunken)] px-3 py-2 text-xs leading-5 text-[var(--text-secondary)]">
                该端点无需 API Key（本地或私有化部署），请求不携带鉴权头。
              </p>
            ) : (
              <CredentialInput providerId={provider.id} providerName={provider.name} onSaved={provider.protocol === 'openai' ? runTest : undefined} />
            )}
            {provider.protocol === 'openai' ? (
              <div className="mt-3 flex items-center gap-2">
                <button
                  type="button"
                  onClick={() => void runTest()}
                  disabled={testState.status === 'testing' || !credentialReady}
                  className="inline-flex items-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3 py-1.5 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40"
                >
                  {testState.status === 'testing' ? <Loader2 size={13} className="animate-spin" /> : <Zap size={13} />}
                  {testState.status === 'testing' ? '验证中…' : noAuth ? '验证连接' : '重新验证'}
                </button>
                {!noAuth && <span className="text-xs text-[var(--text-tertiary)]">密钥不会回传到页面或日志</span>}
              </div>
            ) : null}
            <div className="mt-3"><StatusMessage result={testState} /></div>
          </section>

          <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
            <div className="mb-3 flex items-center gap-2">
              <Server size={15} className="text-[var(--accent)]" />
              <h4 className="text-sm font-semibold text-[var(--text-primary)]">接口设置</h4>
            </div>
            <div className="space-y-4">
              <label className="block">
                <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">请求地址</span>
                <input value={provider.baseUrl} onChange={(event) => updateProvider({ baseUrl: event.target.value })} className="input font-mono text-[13px]" />
              </label>
              <div>
                <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">接口协议</span>
                <div className="grid grid-cols-2 gap-2 rounded-lg bg-[var(--bg-sunken)] p-1">
                  {(['openai', 'anthropic'] as const).map((protocol) => (
                    <button
                      key={protocol}
                      type="button"
                      onClick={() => updateProvider({ protocol })}
                      className={`rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
                        provider.protocol === protocol
                          ? 'bg-[var(--bg-surface)] text-[var(--text-primary)] shadow-[var(--shadow-sm)]'
                          : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'
                      }`}
                    >
                      {protocol === 'openai' ? 'OpenAI 兼容' : 'Anthropic 原生'}
                    </button>
                  ))}
                </div>
              </div>
            </div>
          </section>

          <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
            <div className="mb-3 flex items-center justify-between gap-3">
              <div className="flex items-center gap-2">
                <Layers3 size={15} className="text-[var(--accent)]" />
                <h4 className="text-sm font-semibold text-[var(--text-primary)]">模型</h4>
              </div>
              <button
                type="button"
                onClick={() => void syncModels()}
                disabled={catalogState.status === 'testing' || provider.protocol !== 'openai' || !credentialReady}
                className="inline-flex items-center gap-1.5 rounded-lg px-2 py-1 text-xs font-medium text-[var(--accent)] hover:bg-[var(--accent-subtle)] disabled:opacity-40"
                title={provider.protocol === 'openai' ? '从供应商模型目录同步' : 'Anthropic 原生接口请手工维护模型 ID'}
              >
                {catalogState.status === 'testing' ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
                {catalogState.status === 'testing' ? '同步中…' : '同步模型目录'}
              </button>
            </div>
            <label className="block">
              <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">默认模型</span>
              <select value={provider.model} onChange={(event) => updateProvider({ model: event.target.value })} className="input font-mono text-[13px]">
                {Array.from(new Set([provider.model, ...provider.models])).map((model) => <option key={model} value={model}>{model}</option>)}
              </select>
              <span className="mt-1.5 block text-xs leading-5 text-[var(--text-tertiary)]">新会话默认使用；对话中仍可按轮切换。</span>
            </label>
            <div className="mt-3"><StatusMessage result={catalogState} /></div>

            <details className="group mt-4 border-t border-[var(--border-default)] pt-3">
              <summary className="flex cursor-pointer list-none items-center justify-between text-xs font-medium text-[var(--text-secondary)]">
                <span className="flex items-center gap-1.5"><SlidersHorizontal size={13} /> 高级：管理可选模型（{provider.models.length}）</span>
                <ChevronDown size={14} className="transition-transform group-open:rotate-180" />
              </summary>
              <div className="mt-3 space-y-2">
                {provider.models.map((model) => (
                  <div key={model} className="flex items-center justify-between gap-3 rounded-lg bg-[var(--bg-sunken)] px-3 py-2">
                    <span className="min-w-0 truncate font-mono text-xs text-[var(--text-secondary)]">{model}</span>
                    <button type="button" onClick={() => removeModel(model)} disabled={provider.models.length <= 1} className="shrink-0 rounded p-1 text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--danger)] disabled:opacity-30" aria-label={`移除 ${model}`}>
                      <X size={13} />
                    </button>
                  </div>
                ))}
                <div className="flex gap-2 pt-1">
                  <input
                    value={modelDraft}
                    onChange={(event) => setModelDraft(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter') {
                        event.preventDefault();
                        addModel();
                      }
                    }}
                    placeholder="输入模型 ID"
                    className="input min-w-0 flex-1 font-mono text-[13px]"
                  />
                  <button type="button" onClick={addModel} disabled={!modelDraft.trim()} className="inline-flex shrink-0 items-center gap-1 rounded-lg border border-[var(--accent)] px-3 text-xs font-medium text-[var(--accent)] hover:bg-[var(--accent-subtle)] disabled:opacity-40">
                    <Plus size={13} /> 添加
                  </button>
                </div>
              </div>
            </details>
          </section>
        </div>

        <footer className="flex items-center justify-between gap-3 border-t border-[var(--border-default)] bg-[var(--bg-surface)] px-5 py-3">
          <span className="text-xs text-[var(--text-tertiary)]">接口和模型更改会自动保存</span>
          <div className="flex items-center gap-2">
            <button type="button" onClick={onClose} className="rounded-lg border border-[var(--border-strong)] px-3.5 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]">完成</button>
            <button
              type="button"
              onClick={setActive}
              disabled={isActive || !credentialReady}
              className="inline-flex min-w-28 items-center justify-center gap-1.5 rounded-lg bg-[var(--accent)] px-3.5 py-2 text-xs font-medium text-white hover:bg-[var(--accent-strong)] disabled:opacity-40"
              title={credentialReady ? undefined : '请先保存 API Key'}
            >
              {isActive ? <Check size={13} /> : <Sparkles size={13} />}
              {isActive ? '当前正在使用' : noAuth ? '设为默认' : credentialState === 'unchecked' ? '检查凭据中…' : credentialState === 'missing' ? '配置凭据后启用' : '设为默认'}
            </button>
          </div>
        </footer>
      </aside>
    </div>
  );
}

function EmbeddingSettings() {
  const { settings, updateSettings } = useAppStore();
  const [testState, setTestState] = useState<TestStatus>({ status: 'idle' });
  const semanticEnabled = settings.semanticSearchEnabled ?? true;
  const embedding = settings.aiConfig?.embedding ?? { baseUrl: '', model: '', protocol: 'openai' as const };

  const updateEmbedding = (patch: Partial<typeof embedding>) => {
    updateSettings({
      aiConfig: {
        ...settings.aiConfig,
        activeProvider: settings.aiConfig?.activeProvider ?? 'deepseek',
        providers: settings.aiConfig?.providers ?? {},
        embedding: { ...embedding, ...patch },
      },
    });
  };

  const runTest = async () => {
    if (!embedding.baseUrl || !embedding.model) {
      setTestState({ status: 'fail', message: '请先填写接口地址和嵌入模型' });
      return;
    }
    setTestState({ status: 'testing' });
    try {
      const result = await testEmbeddingConnection();
      setTestState({ status: 'ok', message: `连接成功 · ${result.latencyMs} ms · ${result.dimension} 维` });
    } catch (error) {
      setTestState({ status: 'fail', message: error instanceof Error ? error.message.slice(0, 500) : '连接失败' });
    }
  };

  return (
    <div className="max-w-3xl">
      <section className="rounded-2xl border border-[var(--border-default)] bg-[var(--bg-surface)]">
        <div className="flex items-start justify-between gap-4 border-b border-[var(--border-default)] p-5">
          <div className="flex items-start gap-3">
            <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[var(--accent-subtle)] text-[var(--accent)]"><Layers3 size={19} /></span>
            <div>
              <h3 className="text-sm font-semibold text-[var(--text-primary)]">向量嵌入</h3>
              <p className="mt-1 max-w-xl text-xs leading-5 text-[var(--text-tertiary)]">为收件箱和笔记提供语义搜索。它使用独立模型和凭据，不消耗当前对话供应商配置。</p>
            </div>
          </div>
          <Toggle checked={semanticEnabled} label="语义搜索" onChange={() => updateSettings({ semanticSearchEnabled: !semanticEnabled })} />
        </div>
        <div className={`space-y-5 p-5 ${semanticEnabled ? '' : 'pointer-events-none opacity-40'}`}>
          <div>
            <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">接口协议</span>
            <div className="inline-grid grid-cols-2 gap-1 rounded-lg bg-[var(--bg-sunken)] p-1">
              {([
                { id: 'openai' as const, label: 'OpenAI 兼容' },
                { id: 'dashscope' as const, label: '阿里 DashScope' },
              ]).map((protocol) => (
                <button key={protocol.id} type="button" onClick={() => updateEmbedding({ protocol: protocol.id })} className={`rounded-md px-4 py-1.5 text-xs font-medium ${
                  (embedding.protocol ?? 'openai') === protocol.id ? 'bg-[var(--bg-surface)] text-[var(--text-primary)] shadow-[var(--shadow-sm)]' : 'text-[var(--text-tertiary)]'
                }`}>{protocol.label}</button>
              ))}
            </div>
          </div>
          <div className="grid gap-4 md:grid-cols-2">
            <label className="block">
              <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">接口地址</span>
              <input value={embedding.baseUrl} onChange={(event) => updateEmbedding({ baseUrl: event.target.value })} placeholder={(embedding.protocol ?? 'openai') === 'dashscope' ? '填写完整服务地址' : 'https://api.siliconflow.cn/v1'} className="input font-mono text-[13px]" />
            </label>
            <label className="block">
              <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">嵌入模型</span>
              <input value={embedding.model} onChange={(event) => updateEmbedding({ model: event.target.value })} placeholder={(embedding.protocol ?? 'openai') === 'dashscope' ? 'qwen3.7-text-embedding' : 'BAAI/bge-m3'} className="input font-mono text-[13px]" />
            </label>
          </div>
          {(embedding.protocol ?? 'openai') === 'dashscope' && <p className="-mt-3 text-xs text-[var(--text-tertiary)]">DashScope 原生协议需要填写完整服务地址，不会自动拼接 /embeddings。</p>}
          <div>
            <span className="mb-1.5 block text-xs font-medium text-[var(--text-secondary)]">访问凭据</span>
            <CredentialInput providerId="embedding" providerName="嵌入模型" onSaved={runTest} />
          </div>
          <div className="flex items-center gap-3">
            <button type="button" onClick={() => void runTest()} disabled={testState.status === 'testing'} className="inline-flex items-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40">
              {testState.status === 'testing' ? <Loader2 size={13} className="animate-spin" /> : <Zap size={13} />}
              {testState.status === 'testing' ? '测试中…' : '测试连接'}
            </button>
            <span className="text-xs text-[var(--text-tertiary)]">配置只影响语义检索</span>
          </div>
          <StatusMessage result={testState} />
        </div>
      </section>
    </div>
  );
}

export default function AIProviderSettingsPanel() {
  const { settings, updateSettings, apiKeys, ensureApiKeyLoaded } = useAppStore();
  const [section, setSection] = useState<SettingsSection>('models');
  const [catalogOpen, setCatalogOpen] = useState(false);
  const [drawerProviderId, setDrawerProviderId] = useState<string | null>(null);
  const [runtimeCandidate, setRuntimeCandidate] = useState<RuntimeModelCandidate | null>(null);
  const [runtimeCatalog, setRuntimeCatalog] = useState<HermesModelOptions | null>(null);
  const [runtimeCatalogState, setRuntimeCatalogState] = useState<TestStatus>({ status: 'idle' });
  const [completionEnabled, setCompletionEnabledState] = useState<boolean | null>(null);
  const providers = settings.aiConfig?.providers ?? {};
  const storedActiveProviderId = settings.aiConfig?.activeProvider ?? '';
  const credentialsLoaded = Object.values(providers)
    .filter(providerRequiresKey)
    .every((provider) => apiKeys[provider.id] !== undefined);
  const configuredProviderList = useMemo(
    () => orderModelProviders(providers, storedActiveProviderId)
      .filter((provider) => providerConfigurationReady(provider, apiKeys)),
    [apiKeys, providers, storedActiveProviderId],
  );
  const activeProviderId = configuredProviderList.some((provider) => provider.id === storedActiveProviderId)
    ? storedActiveProviderId
    : configuredProviderList[0]?.id ?? '';
  const activeProvider = providers[activeProviderId];
  const providerList = useMemo(
    () => orderModelProviders(Object.fromEntries(configuredProviderList.map((provider) => [provider.id, provider])), activeProviderId),
    [activeProviderId, configuredProviderList],
  );
  const activeMeta = activeProvider ? modelProviderPreset(activeProvider.id) : undefined;
  const activeCredentialState = activeProvider ? providerCredentialState(apiKeys, activeProvider.id) : 'unchecked';
  const configuredProviders = useMemo(
    () => Object.fromEntries(configuredProviderList.map((provider) => [provider.id, provider])),
    [configuredProviderList],
  );
  const discoveredCandidates = useMemo(
    () => runtimeModelCandidates(runtimeCatalog?.providers ?? [], configuredProviders),
    [configuredProviders, runtimeCatalog?.providers],
  );
  const refreshRuntimeCatalog = useCallback(async () => {
    setRuntimeCatalogState({ status: 'testing' });
    try {
      const result = await hermesModelCatalog();
      setRuntimeCatalog(result);
      setRuntimeCatalogState({ status: 'ok' });
    } catch (error) {
      setRuntimeCatalog(null);
      setRuntimeCatalogState({ status: 'fail', message: error instanceof Error ? error.message : 'Runtime 目录不可用' });
    }
  }, []);

  useEffect(() => {
    Object.values(providers)
      .filter(providerRequiresKey)
      .forEach((provider) => { void ensureApiKeyLoaded(provider.id); });
  }, [ensureApiKeyLoaded, providers]);

  useEffect(() => {
    if (!credentialsLoaded || !activeProviderId || activeProviderId === storedActiveProviderId) return;
    updateSettings({ aiConfig: { ...settings.aiConfig, activeProvider: activeProviderId, providers } });
  }, [activeProviderId, credentialsLoaded, providers, settings.aiConfig, storedActiveProviderId, updateSettings]);

  useEffect(() => {
    let cancelled = false;
    getCompletionConfig()
      .then((config) => { if (!cancelled) setCompletionEnabledState(config.enabled); })
      .catch(() => { if (!cancelled) setCompletionEnabledState(true); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (section === 'models' && runtimeCatalogState.status === 'idle') void refreshRuntimeCatalog();
  }, [refreshRuntimeCatalog, runtimeCatalogState.status, section]);

  useEffect(() => {
    if (!catalogOpen && !drawerProviderId && !runtimeCandidate) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (runtimeCandidate) setRuntimeCandidate(null);
      else if (catalogOpen) setCatalogOpen(false);
      else setDrawerProviderId(null);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [catalogOpen, drawerProviderId, runtimeCandidate]);

  const toggleCompletion = async () => {
    if (completionEnabled == null) return;
    const next = !completionEnabled;
    setCompletionEnabledState(next);
    try {
      await setCompletionEnabled(next);
    } catch {
      setCompletionEnabledState(!next);
    }
  };

  const addProvider = (base: ProviderConfig) => {
    // 同族多实例：唯一 id（ollama/ollama-2…）+ 名称带序号，凭据与鉴权模式逐实例独立
    const id = uniqueProviderId(providers, base.id);
    const provider =
      id === base.id ? base : { ...base, id, name: `${base.name} ${id.slice(base.id.length + 1)}` };
    updateSettings({
      aiConfig: {
        ...settings.aiConfig,
        activeProvider: settings.aiConfig?.activeProvider ?? provider.id,
        providers: { ...providers, [provider.id]: provider },
      },
    });
    setCatalogOpen(false);
    setDrawerProviderId(provider.id);
  };

  const activateProvider = async (provider: ProviderConfig) => {
    await ensureApiKeyLoaded(provider.id);
    if (!providerCredentialReady(provider, useAppStore.getState().apiKeys)) {
      setDrawerProviderId(provider.id);
      return;
    }
    updateSettings({ aiConfig: { ...settings.aiConfig, activeProvider: provider.id, providers } });
  };

  const configureRuntimeModels = (candidate: RuntimeModelCandidate, selectedModels: string[]) => {
    if (selectedModels.length === 0) return;
    const existing = modelProviderFamilyInstances(providers, candidate.settingsProviderId)
      .find((provider) => providerConfigurationReady(provider, apiKeys))
      ?? modelProviderFamilyInstances(providers, candidate.settingsProviderId)[0];
    const provider = existing
      ? {
          ...existing,
          model: existing.model || selectedModels[0],
          models: Array.from(new Set([...existing.models, ...selectedModels])),
        }
      : providerFromRuntimeCandidate(candidate, selectedModels);
    updateSettings({
      aiConfig: {
        ...settings.aiConfig,
        activeProvider: settings.aiConfig?.activeProvider || provider.id,
        providers: { ...providers, [provider.id]: provider },
      },
    });
    setRuntimeCandidate(null);
    setDrawerProviderId(provider.id);
  };

  return (
    <div className="max-w-5xl">
      <div className="mb-6 flex items-center gap-1 border-b border-[var(--border-default)]">
        {([
          { id: 'models' as const, label: '对话模型', icon: Sparkles },
          { id: 'embedding' as const, label: '向量嵌入', icon: Layers3 },
        ]).map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => setSection(item.id)}
              className={`relative inline-flex items-center gap-1.5 px-3 py-2.5 text-sm font-medium transition-colors ${
                section === item.id ? 'text-[var(--accent)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]'
              }`}
            >
              <Icon size={14} /> {item.label}
              {section === item.id && <span className="absolute inset-x-2 bottom-[-1px] h-0.5 rounded-full bg-[var(--accent)]" />}
            </button>
          );
        })}
      </div>

      {section === 'models' ? (
        <div className="space-y-5">
          <section className="overflow-hidden rounded-2xl border border-[var(--accent-border)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]">
            <div className="flex flex-col gap-5 p-5 sm:flex-row sm:items-center sm:justify-between">
              <div className="flex min-w-0 items-center gap-4">
                <span className="flex h-12 w-12 shrink-0 items-center justify-center rounded-2xl bg-[var(--accent-subtle)] text-[var(--accent)]"><Sparkles size={22} /></span>
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-[11px] font-semibold uppercase tracking-[0.12em] text-[var(--text-tertiary)]">当前默认模型</span>
                    {activeProvider && <CredentialBadge state={activeCredentialState} noAuth={!providerRequiresKey(activeProvider)} />}
                  </div>
                  {activeProvider ? (
                    <>
                      <h3 className="mt-1 truncate text-lg font-semibold tracking-[-0.02em] text-[var(--text-primary)]">{activeProvider.model}</h3>
                      <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-[var(--text-tertiary)]">
                        <span className="font-medium text-[var(--text-secondary)]">{activeProvider.name}</span>
                        <span>·</span>
                        <span>{activeProvider.protocol === 'openai' ? 'OpenAI 兼容' : 'Anthropic 原生'}</span>
                        {activeMeta && <><span>·</span><span>{activeMeta.completionCompatible ? '支持行内补全' : '仅 Agent'}</span></>}
                      </p>
                    </>
                  ) : (
                    <h3 className="mt-1 text-base font-semibold text-[var(--text-primary)]">尚未选择默认模型</h3>
                  )}
                </div>
              </div>
              <button type="button" onClick={() => activeProvider && setDrawerProviderId(activeProvider.id)} disabled={!activeProvider} className="inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3.5 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40">
                <SlidersHorizontal size={13} /> 配置当前模型
              </button>
            </div>
            <div className="flex items-center justify-between gap-4 border-t border-[var(--border-default)] bg-[var(--bg-sunken)]/60 px-5 py-3">
              <div className="min-w-0">
                <p className="text-xs font-medium text-[var(--text-secondary)]">行内补全</p>
                <p className="mt-0.5 truncate text-[11px] text-[var(--text-tertiary)]">
                  {activeProvider?.protocol === 'anthropic' ? '当前协议不支持；Hermes Agent 仍可正常使用' : '写作停顿时生成续写，使用当前默认供应商与额度'}
                </p>
              </div>
              <Toggle checked={completionEnabled === true} disabled={completionEnabled == null || activeProvider?.protocol === 'anthropic'} label="行内补全" onChange={() => void toggleCompletion()} />
            </div>
          </section>

          <section>
            <div className="mb-3 flex items-end justify-between gap-4">
              <div>
                <h3 className="text-sm font-semibold text-[var(--text-primary)]">模型供应商</h3>
                <p className="mt-1 text-xs text-[var(--text-tertiary)]">保留多个配置，需要时切换默认路由；凭据分别存放。</p>
              </div>
              <button type="button" onClick={() => setCatalogOpen(true)} className="inline-flex shrink-0 items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3.5 py-2 text-xs font-medium text-white hover:bg-[var(--accent-strong)]">
                <Plus size={13} /> 添加供应商
              </button>
            </div>
            <div className="overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]">
              {!credentialsLoaded ? (
                <div className="flex min-h-20 items-center justify-center text-xs text-[var(--text-tertiary)]"><Loader2 size={14} className="mr-2 animate-spin" />读取供应商配置</div>
              ) : providerList.length === 0 ? (
                <button type="button" onClick={() => setCatalogOpen(true)} className="flex min-h-20 w-full items-center justify-center text-xs text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]">添加模型供应商</button>
              ) : providerList.map((provider, index) => {
                const isActive = provider.id === activeProviderId;
                const credentialState = providerCredentialState(apiKeys, provider.id);
                const metadata = modelProviderPreset(provider.id);
                return (
                  <div
                    key={provider.id}
                    className={`flex items-center gap-3 px-3 py-2.5 transition-colors hover:bg-[var(--bg-sunken)] ${
                      index > 0 ? 'border-t border-[var(--border-default)]' : ''
                    }`}
                  >
                    <button
                      type="button"
                      onClick={() => setDrawerProviderId(provider.id)}
                      className="flex min-w-0 flex-1 items-center gap-3 rounded-lg p-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
                    >
                      <span className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-xl ${isActive ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]'}`}>
                        <Bot size={17} />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="flex flex-wrap items-center gap-2">
                          <strong className="truncate text-sm font-semibold text-[var(--text-primary)]">{provider.name}</strong>
                          {isActive && <span className="rounded-full bg-[var(--accent-subtle)] px-2 py-0.5 text-[10px] font-semibold text-[var(--accent)]">默认</span>}
                          <CredentialBadge state={credentialState} noAuth={!providerRequiresKey(provider)} />
                        </span>
                        <span className="mt-1 flex min-w-0 items-center gap-1.5 text-xs text-[var(--text-tertiary)]">
                          <span className="truncate font-mono">{provider.model}</span>
                          <span>·</span>
                          <span className="shrink-0">{provider.protocol === 'openai' ? 'OpenAI' : 'Anthropic'}</span>
                          {metadata && <span className="hidden truncate lg:inline">· {metadata.description}</span>}
                        </span>
                      </span>
                    </button>
                    <div className="flex shrink-0 items-center gap-1.5">
                      {!isActive && (
                        <button
                          type="button"
                          onClick={() => void activateProvider(provider)}
                          className="hidden rounded-lg border border-[var(--border-strong)] px-2.5 py-1.5 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-surface)] sm:inline-flex"
                        >
                          设为默认
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => setDrawerProviderId(provider.id)}
                        className="rounded-lg p-2 text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]"
                        aria-label={`配置 ${provider.name}`}
                      >
                        <ChevronRight size={16} />
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
            <div className="mt-3 flex items-center gap-2 rounded-lg px-1 text-xs leading-5 text-[var(--text-tertiary)]">
              <ShieldCheck size={13} className="shrink-0 text-[var(--success)]" /> API Key 只由 Rust Host 写入 macOS 钥匙串，不进入页面状态或模型配置文件。
            </div>
          </section>

        </div>
      ) : (
        <EmbeddingSettings />
      )}

      <ProviderCatalogDialog
        open={catalogOpen}
        providers={providers}
        apiKeys={apiKeys}
        runtimeCandidates={discoveredCandidates}
        runtimeState={runtimeCatalogState}
        onClose={() => setCatalogOpen(false)}
        onAdd={addProvider}
        onEdit={(providerId) => { setCatalogOpen(false); setDrawerProviderId(providerId); }}
        onPickRuntime={(candidate) => { setCatalogOpen(false); setRuntimeCandidate(candidate); }}
        onRefreshRuntime={() => void refreshRuntimeCatalog()}
      />
      <RuntimeModelCatalogDialog candidate={runtimeCandidate} onClose={() => setRuntimeCandidate(null)} onAdd={(models) => runtimeCandidate && configureRuntimeModels(runtimeCandidate, models)} />
      <ProviderConfigDrawer providerId={drawerProviderId} onClose={() => setDrawerProviderId(null)} />
    </div>
  );
}
