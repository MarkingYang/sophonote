import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowRight, RefreshCw, Search, X } from 'lucide-react';
import { agentCapabilities, listenHermesStatusChanged, restartHermesRuntime, type EngineCapabilities } from '../../services/tauri';
import { subscribeTauriListener } from '../../services/browserErrors';
import { CAPABILITY_CATEGORIES, CAPABILITY_ENGINES, capabilityEntries, filterCapabilityEntries, type CapabilityCategory, type CapabilityEngine } from '../../services/capabilityCatalog';
import { HermesCapabilitiesPanel } from '../aistudio/ProjectChatPanel';

interface CatalogState { loading: boolean; snapshot: EngineCapabilities | null; error: string | null; }
const initialCatalog = (): Record<CapabilityEngine, CatalogState> => Object.fromEntries(
  CAPABILITY_ENGINES.map(({ id }) => [id, { loading: true, snapshot: null, error: null }]),
) as Record<CapabilityEngine, CatalogState>;

export default function CapabilitySettingsPanel() {
  const [engine, setEngine] = useState<CapabilityEngine | 'all'>('all');
  const [category, setCategory] = useState<CapabilityCategory | 'all'>('all');
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState('');
  const [nativeSelection, setNativeSelection] = useState('');
  const [catalog, setCatalog] = useState(initialCatalog);
  const requests = useRef<Record<string, number>>({});
  const alive = useRef(false);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());

  const refreshEngine = useCallback((id: CapabilityEngine) => {
    const request = (requests.current[id] ?? 0) + 1;
    requests.current[id] = request;
    setCatalog((current) => ({ ...current, [id]: { ...current[id], loading: true, error: null } }));
    let settled = false;
    const complete = (snapshot: EngineCapabilities | null, error: string | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer); timers.current.delete(timer);
      if (!alive.current || requests.current[id] !== request) return;
      setCatalog((current) => ({ ...current, [id]: { loading: false, snapshot, error } }));
    };
    const timer = setTimeout(() => complete(null, '读取超时，请重试'), 30_000);
    timers.current.add(timer);
    void agentCapabilities(id).then((snapshot) => complete(snapshot, snapshot.error), (reason) => complete(null, reason instanceof Error ? reason.message : String(reason)));
  }, []);

  useEffect(() => {
    alive.current = true;
    CAPABILITY_ENGINES.forEach(({ id }) => refreshEngine(id));
    const pendingTimers = timers.current;
    return () => { alive.current = false; pendingTimers.forEach(clearTimeout); pendingTimers.clear(); };
  }, [refreshEngine]);

  useEffect(() => subscribeTauriListener(listenHermesStatusChanged((status) => {
    if (status === 'connected') refreshEngine('hermes');
    else if (alive.current) {
      requests.current.hermes = (requests.current.hermes ?? 0) + 1;
      setCatalog((current) => ({ ...current, hermes: { loading: false, snapshot: null, error: status === 'restarting' ? 'Hermes 正在重新连接' : 'Hermes 运行时未连接' } }));
    }
  })), [refreshEngine]);

  const reconnect = useCallback(async () => {
    const request = (requests.current.hermes ?? 0) + 1;
    requests.current.hermes = request;
    setCatalog((current) => ({ ...current, hermes: { loading: true, snapshot: null, error: null } }));
    try { await restartHermesRuntime(); if (alive.current && requests.current.hermes === request) refreshEngine('hermes'); }
    catch (reason) { if (alive.current && requests.current.hermes === request) setCatalog((current) => ({ ...current, hermes: { loading: false, snapshot: null, error: String(reason) } })); }
  }, [refreshEngine]);

  const entries = useMemo(() => CAPABILITY_ENGINES.flatMap(({ id }) => catalog[id].snapshot ? capabilityEntries(catalog[id].snapshot!) : []), [catalog]);
  const filtered = useMemo(() => filterCapabilityEntries(entries, engine, category, query), [entries, engine, category, query]);
  const active = filtered.find((entry) => entry.id === selected) ?? filtered[0];
  const visibleEngines = CAPABILITY_ENGINES.filter(({ id }) => engine === 'all' || id === engine);
  const loading = visibleEngines.some(({ id }) => catalog[id].loading);
  const native = engine === 'hermes' && category !== 'all';
  const unavailableCategories = category === 'all' || category === 'tools' ? [] : visibleEngines.filter(({ id }) => id !== 'hermes');
  const chooseEngine = (id: CapabilityEngine | 'all') => { setEngine(id); setNativeSelection(''); };
  const chooseCategory = (id: CapabilityCategory | 'all') => { setCategory(id); setNativeSelection(''); };
  const refresh = () => visibleEngines.forEach(({ id }) => refreshEngine(id));

  return (
    <div className="max-w-5xl space-y-4">
      <header className="flex items-start justify-between gap-4">
        <div><h3 className="text-base font-semibold text-[var(--text-primary)]">能力配置</h3><p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">查找各引擎可用的技能、工具和外部服务。</p></div>
        <button type="button" onClick={refresh} disabled={loading} className="inline-flex items-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3 py-2 text-xs text-[var(--text-secondary)] disabled:opacity-50"><RefreshCw size={13} className={loading ? 'animate-spin' : ''} />刷新</button>
      </header>
      <label className="flex items-center gap-2 rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] px-3 focus-within:border-[var(--accent)]">
        <Search size={16} className="text-[var(--text-tertiary)]" />
        <input aria-label="搜索能力" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索技能、工具、MCP 服务…" className="min-w-0 flex-1 bg-transparent py-3 text-sm text-[var(--text-primary)] outline-none" />
        {query && <button type="button" aria-label="清空能力搜索" onClick={() => setQuery('')} className="text-[var(--text-tertiary)]"><X size={14} /></button>}
      </label>
      <nav aria-label="能力引擎" className="flex flex-wrap gap-1 border-b border-[var(--border-default)] pb-2">
        {[{ id: 'all' as const, label: '全部' }, ...CAPABILITY_ENGINES].map((item) => <button type="button" key={item.id} aria-pressed={engine === item.id} onClick={() => chooseEngine(item.id)} className={`rounded-lg px-3 py-2 text-xs transition-colors ${engine === item.id ? 'bg-[var(--accent-subtle)] font-semibold text-[var(--accent)]' : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'}`}>{item.label}</button>)}
      </nav>
      <nav aria-label="能力分类" className="flex flex-wrap gap-2">
        {[{ id: 'all' as const, label: '全部' }, ...CAPABILITY_CATEGORIES].map((item) => <button type="button" key={item.id} aria-pressed={category === item.id} onClick={() => chooseCategory(item.id)} className={`rounded-md px-2.5 py-1.5 text-xs ${category === item.id ? 'bg-[var(--bg-sunken)] font-semibold text-[var(--text-primary)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-primary)]'}`}>{item.label}</button>)}
      </nav>
      <div aria-label="引擎运行状态" aria-live="polite" className="flex flex-wrap gap-x-5 gap-y-2 text-xs">
        {visibleEngines.map(({ id, label }) => {
          const state = catalog[id];
          return <div key={id} className="flex min-w-0 items-center gap-1.5"><span className={`size-1.5 shrink-0 rounded-full ${state.loading ? 'animate-pulse bg-[var(--warning)]' : state.error || !state.snapshot?.available ? 'bg-[var(--warning)]' : 'bg-[var(--success)]'}`} /><span className="text-[var(--text-secondary)]">{label}</span><span className="text-[var(--text-tertiary)]" title={state.error ?? undefined}>{state.loading ? '正在读取' : state.error || !state.snapshot?.available ? '不可用' : id === 'hermes' ? '已连接' : `可用 ${state.snapshot.version ?? ''}`}</span>{!state.loading && (state.error || !state.snapshot?.available) && <button type="button" onClick={() => refreshEngine(id)} aria-label={`重试 ${label}`} className="text-[var(--accent)]">重试</button>}</div>;
        })}
      </div>
      {visibleEngines.filter(({ id }) => catalog[id].error).map(({ id, label }) => <p key={id} role="status" className="break-words rounded-lg bg-[var(--warning-subtle)] px-3 py-2 text-xs text-[var(--warning)]">{label}：{catalog[id].error}</p>)}
      {unavailableCategories.length > 0 && <p className="text-xs leading-5 text-[var(--text-tertiary)]">{unavailableCategories.map(({ label }) => label).join('、')} 尚未接入{CAPABILITY_CATEGORIES.find((item) => item.id === category)?.label}管理。{category === 'mcp' && '会话内置工具桥不属于可管理的 MCP 服务。'}</p>}
      {native ? <>
        {category === 'hub' && <p className="text-xs text-[var(--text-tertiary)]">当前资源安装到 Hermes；切换引擎可查看对应接入状态。</p>}
        <HermesCapabilitiesPanel snapshot={catalog.hermes.snapshot?.hermes ?? null} error={catalog.hermes.error} connStatus={catalog.hermes.snapshot?.available ? 'connected' : catalog.hermes.loading ? 'restarting' : 'disconnected'} tab={category} onTab={setCategory} onRefresh={() => refreshEngine('hermes')} onReconnect={() => void reconnect()} embedded showTabs={false} searchQuery={query} initialSelection={nativeSelection} />
      </> : <section aria-label="能力目录" className="grid min-h-[400px] grid-cols-1 overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] md:grid-cols-[minmax(200px,0.9fr)_minmax(240px,1.1fr)]">
        <div className="max-h-[560px] overflow-y-auto border-b border-[var(--border-default)] md:border-r md:border-b-0">
          <p className="sticky top-0 bg-[var(--bg-surface)] px-4 py-3 text-xs text-[var(--text-tertiary)]">{filtered.length} 项能力{loading ? ' · 继续读取其它引擎' : ''}</p>
          {filtered.map((entry) => <button key={entry.id} type="button" onClick={() => setSelected(entry.id)} aria-pressed={active?.id === entry.id} className={`block w-full border-l-2 px-4 py-3 text-left ${active?.id === entry.id ? 'border-[var(--accent)] bg-[var(--accent-subtle)]' : 'border-transparent hover:bg-[var(--bg-sunken)]'}`}><span className="block break-words text-sm font-medium text-[var(--text-primary)]">{entry.name}</span><span className="mt-1 block text-xs text-[var(--text-tertiary)]">{CAPABILITY_ENGINES.find(({ id }) => id === entry.engine)?.label} · {CAPABILITY_CATEGORIES.find(({ id }) => id === entry.category)?.label} · {entry.state}</span></button>)}
          {!filtered.length && <p className="px-4 py-8 text-sm text-[var(--text-tertiary)]">{loading ? '正在读取能力目录…' : query.trim() ? '没有匹配的能力，请调整关键词或筛选。' : '当前范围暂无已接入的能力。'}</p>}
        </div>
        <div className="max-h-[560px] min-w-0 overflow-y-auto p-5">
          {active ? <><h4 className="break-words text-base font-semibold text-[var(--text-primary)]">{active.name}</h4><p className="mt-3 whitespace-pre-wrap break-words text-sm leading-6 text-[var(--text-secondary)]">{active.description || '该能力未提供说明。'}</p><dl className="mt-6 grid grid-cols-[auto_1fr] gap-x-5 gap-y-3 text-xs"><dt className="text-[var(--text-tertiary)]">适用引擎</dt><dd>{CAPABILITY_ENGINES.find(({ id }) => id === active.engine)?.label}</dd><dt className="text-[var(--text-tertiary)]">来源</dt><dd className="break-all">{active.source}</dd><dt className="text-[var(--text-tertiary)]">状态</dt><dd>{catalog[active.engine].loading ? '正在刷新，展示上次读取结果' : active.state}</dd></dl>
            {catalog[active.engine].snapshot?.managedCategories.includes(active.category) ? <button type="button" disabled={catalog[active.engine].loading || !catalog[active.engine].snapshot?.available} onClick={() => { setEngine(active.engine); setCategory(active.category); setNativeSelection(active.selection); }} className="mt-6 inline-flex items-center gap-2 rounded-lg bg-[var(--accent)] px-3 py-2 text-xs text-white disabled:opacity-50">在 Hermes 中管理<ArrowRight size={13} /></button> : <p className="mt-6 text-xs leading-5 text-[var(--text-tertiary)]">工具随会话提供，实际调用受项目范围和权限模式控制。技能、外部 MCP 和资源安装尚未接入此引擎。</p>}
          </> : <p className="text-sm text-[var(--text-tertiary)]">选择一项能力查看来源和可用状态。</p>}
        </div>
      </section>}
    </div>
  );
}
