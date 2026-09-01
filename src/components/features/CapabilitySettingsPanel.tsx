import { useCallback, useEffect, useMemo, useState } from 'react';
import { Boxes, RefreshCw, Search, Server, Wrench, type LucideIcon } from 'lucide-react';
import {
  hermesCapabilities,
  listenHermesStatusChanged,
  restartHermesRuntime,
  type HermesCapabilities,
  type HermesConnectionStatus,
} from '../../services/tauri';
import {
  HermesCapabilitiesPanel,
  type CapabilityTab,
} from '../aistudio/ProjectChatPanel';

interface CapabilityCard {
  id: CapabilityTab;
  label: string;
  description: string;
  icon: LucideIcon;
  summary: (snapshot: HermesCapabilities | null) => string;
}

const CAPABILITY_CARDS: CapabilityCard[] = [
  {
    id: 'skills',
    label: 'Skill',
    description: '管理已安装的任务能力、启用状态与说明文件。',
    icon: Boxes,
    summary: (snapshot) => snapshot
      ? `${snapshot.skills.filter((item) => item.enabled).length} / ${snapshot.skills.length} 已启用`
      : '正在读取',
  },
  {
    id: 'tools',
    label: 'Tools',
    description: '查看工具集、执行工具与终端后端的可用状态。',
    icon: Wrench,
    summary: (snapshot) => snapshot
      ? `${snapshot.toolsets.filter((item) => item.enabled).length} 个工具集 · ${snapshot.tools.length} 个工具`
      : '正在读取',
  },
  {
    id: 'mcp',
    label: 'MCP',
    description: '连接、测试并管理外部 MCP Server。',
    icon: Server,
    summary: (snapshot) => snapshot
      ? `${snapshot.mcpServers.filter((item) => item.enabled).length} / ${snapshot.mcpServers.length} 已连接`
      : '正在读取',
  },
  {
    id: 'hub',
    label: 'Browse Hub',
    description: '从已连接的 Hub 搜索、预览并安装 Skill。',
    icon: Search,
    summary: (snapshot) => snapshot
      ? `${snapshot.hubSources.sources.filter((item) => item.available !== false && !item.rateLimited).length} 个来源可用`
      : '正在读取',
  },
];

export default function CapabilitySettingsPanel() {
  const [tab, setTab] = useState<CapabilityTab>('skills');
  const [snapshot, setSnapshot] = useState<HermesCapabilities | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connStatus, setConnStatus] = useState<HermesConnectionStatus>('connected');

  const refresh = useCallback(() => {
    setError(null);
    hermesCapabilities()
      .then((next) => {
        setSnapshot(next);
        setConnStatus('connected');
      })
      .catch((reason) => {
        setError(reason instanceof Error ? reason.message : String(reason));
        setConnStatus('disconnected');
      });
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    listenHermesStatusChanged((status) => {
      setConnStatus(status);
      if (status === 'connected') refresh();
    }).then((next) => { unlisten = next; });
    return () => { unlisten?.(); };
  }, [refresh]);

  const reconnect = useCallback(() => {
    setConnStatus('restarting');
    setError(null);
    restartHermesRuntime()
      .then(refresh)
      .catch((reason) => {
        setError(reason instanceof Error ? reason.message : String(reason));
        setConnStatus('disconnected');
      });
  }, [refresh]);

  const statusLabel = useMemo(() => {
    if (connStatus === 'connected') return '运行时已连接';
    if (connStatus === 'restarting') return '正在重新连接';
    return '运行时未连接';
  }, [connStatus]);

  return (
    <div className="max-w-5xl space-y-5">
      <section className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h3 className="text-base font-semibold tracking-[-0.01em] text-[var(--text-primary)]">能力配置</h3>
          <p className="mt-1 max-w-2xl text-xs leading-5 text-[var(--text-tertiary)]">
            集中管理 Agent 可调用的能力、外部服务连接与可安装资源。
          </p>
        </div>
        <div className="flex items-center gap-2">
          <span className="inline-flex items-center gap-1.5 text-xs text-[var(--text-tertiary)]">
            <span className={`size-1.5 rounded-full ${connStatus === 'connected' ? 'bg-[var(--success)]' : connStatus === 'restarting' ? 'animate-pulse bg-[var(--warning)]' : 'bg-[var(--danger)]'}`} />
            {statusLabel}
          </span>
          <button
            type="button"
            onClick={connStatus === 'disconnected' ? reconnect : refresh}
            className="inline-flex items-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3 py-2 text-xs font-medium text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
          >
            <RefreshCw size={13} className={connStatus === 'restarting' ? 'animate-spin' : ''} />
            {connStatus === 'disconnected' ? '重新连接' : '刷新'}
          </button>
        </div>
      </section>

      <section className="grid gap-3 sm:grid-cols-2" aria-label="能力类型">
        {CAPABILITY_CARDS.map((item) => {
          const Icon = item.icon;
          const active = tab === item.id;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => setTab(item.id)}
              aria-pressed={active}
              className={`group flex min-h-32 items-start gap-4 rounded-2xl border p-4 text-left shadow-[var(--shadow-sm)] transition-[border-color,background-color,transform] duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] active:translate-y-px ${
                active
                  ? 'border-[var(--accent-border)] bg-[var(--accent-subtle)]'
                  : 'border-[var(--border-default)] bg-[var(--bg-surface)] hover:border-[var(--border-strong)] hover:bg-[var(--bg-sunken)]'
              }`}
            >
              <span className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-xl transition-colors ${active ? 'bg-[var(--bg-surface)] text-[var(--accent)]' : 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)] group-hover:text-[var(--text-secondary)]'}`}>
                <Icon size={20} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-center justify-between gap-3">
                  <strong className="text-sm font-semibold text-[var(--text-primary)]">{item.label}</strong>
                  {active && <span className="text-[10px] font-semibold text-[var(--accent)]">当前</span>}
                </span>
                <span className="mt-1.5 block text-xs leading-5 text-[var(--text-tertiary)]">{item.description}</span>
                <span className="mt-3 block text-xs font-medium tabular-nums text-[var(--text-secondary)]">{item.summary(snapshot)}</span>
              </span>
            </button>
          );
        })}
      </section>

      <HermesCapabilitiesPanel
        snapshot={snapshot}
        error={error}
        connStatus={connStatus}
        tab={tab}
        onTab={setTab}
        onRefresh={refresh}
        onReconnect={reconnect}
        embedded
        showTabs={false}
      />
    </div>
  );
}
