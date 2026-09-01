import { useEffect, useRef, useState } from 'react';
import { openPath } from '@tauri-apps/plugin-opener';
import { useAppStore } from '../stores/appStore';
import {
  fetchSourcesNow,
  getCoverageStats,
  getStorageLayout,
  getItems,
  getHermesSidecarStatus,
  getSetting,
  getStorageStats,
  gcOrphanAssets,
  hasApiKey,
  listenHermesSidecarProgress,
  saveApiKey,
  pullHermesSidecar,
  updateSetting,
} from '../services/tauri';
import { sourceConnStatus } from '../types';
import type { Item, Source } from '../types';
import type { HermesSidecarProgress, HermesSidecarUpdatePhase } from '../services/tauri';
import VerticalResizeHandle from '../components/ui/VerticalResizeHandle';
import InboxPanel from '../components/features/InboxPanel';
import UsageStatisticsPanel from '../components/features/UsageStatisticsPanel';
import AIProviderSettingsPanel from '../components/features/AIProviderSettingsPanel';
import CapabilitySettingsPanel from '../components/features/CapabilitySettingsPanel';
import { Key, Palette, Database, Eye, EyeOff, CheckCircle2, XCircle, Loader2, RefreshCw, Inbox as InboxIcon, ShieldCheck, BarChart3, Boxes, DownloadCloud } from 'lucide-react';

const SETTINGS_NAV_WIDTH_KEY = 'ui:settings-nav-width';

// 同步结果条目列表的类型标签与热度缩写
const TYPE_LABEL: Record<string, string> = { repo: '仓库', paper: '论文', model: '模型', article: '文章', product: '产品' };
const compactNum = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
/** NB-12：字节数格式化（≥1MB 显 MB，≥1KB 显 KB） */
const fmtBytes = (n: number) =>
  n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB`
  : n >= 1024 ? `${(n / 1024).toFixed(1)} KB` : `${n} B`;

const SIDECAR_PROGRESS_STEPS: Array<{ phase: HermesSidecarUpdatePhase; label: string }> = [
  { phase: 'checking', label: '检查版本' },
  { phase: 'downloading', label: '下载' },
  { phase: 'unpacking', label: '解包' },
  { phase: 'copying', label: '复制 Runtime' },
  { phase: 'installing', label: '安装依赖' },
  { phase: 'verifying', label: '校验导入' },
  { phase: 'signing', label: '签名' },
  { phase: 'hashing', label: '生成哈希' },
  { phase: 'staging', label: '等待重启' },
];

// API Key 输入框：停止输入 800ms 后自动写入钥匙串，并给出明确的保存状态反馈
function ApiKeyField({ providerId, placeholder }: { providerId: string; placeholder?: string }) {
  const { apiKeys, setApiKey, ensureApiKeyLoaded } = useAppStore();
  const [value, setValue] = useState('');
  const [show, setShow] = useState(false);
  const [saveState, setSaveState] = useState<'idle' | 'pending' | 'saving' | 'saved' | 'error'>('idle');
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // 切换供应商时惰性读取对应 Key（此时才可能弹一次钥匙串授权）
  useEffect(() => {
    let cancelled = false;
    (async () => {
      await ensureApiKeyLoaded(providerId);
      if (!cancelled) {
        // Existing secrets are deliberately not read back into the WebView.
        setValue('');
        setSaveState('idle');
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerId]);

  useEffect(() => () => {
    if (saveTimer.current) clearTimeout(saveTimer.current);
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, []);

  const handleChange = (v: string) => {
    setValue(v);
    if (!v) {
      setSaveState('idle');
      if (saveTimer.current) clearTimeout(saveTimer.current);
      if (resetTimer.current) clearTimeout(resetTimer.current);
      return;
    }
    setSaveState('pending');
    if (saveTimer.current) clearTimeout(saveTimer.current);
    if (resetTimer.current) clearTimeout(resetTimer.current);
    saveTimer.current = setTimeout(async () => {
      setSaveState('saving');
      try {
        await setApiKey(providerId, v);
        // 短期体验优先：保存后保留当前页面的掩码值，让用户明确看到已填写。
        // 该值只存在组件内存；页面刷新/应用重开后 Host 仍不会回传明文。
        setSaveState('saved');
        resetTimer.current = setTimeout(() => setSaveState('idle'), 3000);
      } catch {
        setSaveState('error');
      }
    }, 800);
  };

  return (
    <div>
      <div className="relative">
        <input
          type={show ? 'text' : 'password'}
          value={value}
          onChange={(e) => handleChange(e.target.value)}
          placeholder={placeholder}
          className="input pr-10"
        />
        <button
          type="button"
          onClick={() => setShow(!show)}
          className="absolute right-2.5 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors"
          title={show ? '隐藏 API Key' : '显示 API Key'}
        >
          {show ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </div>
      {(saveState !== 'idle' || apiKeys[providerId]) && (
        <p className={`text-xs mt-1 flex items-center gap-1 ${
          saveState === 'saved' ? 'text-[var(--success)]'
          : saveState === 'error' ? 'text-[var(--danger)]'
          : 'text-[var(--text-tertiary)]'
        }`}>
          {saveState === 'pending' && '编辑中，停止输入后自动保存…'}
          {saveState === 'saving' && (<><Loader2 size={11} className="animate-spin" /> 正在安全保存…</>)}
          {saveState === 'saved' && (<><CheckCircle2 size={11} /> API Key 已保存</>)}
          {saveState === 'error' && (<><XCircle size={11} /> 保存失败，请重试或查看运行日志</>)}
          {saveState === 'idle' && apiKeys[providerId] && 'API Key 已配置；输入新值可替换（旧值不会回传到页面）'}
        </p>
      )}
    </div>
  );
}

function OpenRouterCredentialCard() {
  const [value, setValue] = useState('');
  const [show, setShow] = useState(false);
  const [configured, setConfigured] = useState<boolean | null>(null);
  const [saveState, setSaveState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');
  const [message, setMessage] = useState('');

  useEffect(() => {
    let cancelled = false;
    hasApiKey('openrouter-rankings')
      .then((exists) => { if (!cancelled) setConfigured(exists); })
      .catch((error) => {
        if (cancelled) return;
        setConfigured(false);
        setSaveState('error');
        setMessage(error instanceof Error ? error.message : '读取 OpenRouter 凭据状态失败');
      });
    return () => { cancelled = true; };
  }, []);

  const save = async () => {
    const key = value.trim();
    if (!key || saveState === 'saving') return;
    setSaveState('saving');
    setMessage('');
    try {
      const result = await saveApiKey('openrouter-rankings', key);
      const usedDebugFallback = result.includes('debug_fallback');
      const runtimeWarning = result.includes('hermes_restart_failed');
      setConfigured(true);
      setSaveState('saved');
      setMessage(
        `${usedDebugFallback ? '已保存到 Debug 本机开发回退' : '已安全保存到 macOS 钥匙串'}${
          runtimeWarning ? '；Hermes 将在下次启动时读取新凭据' : ''
        }`,
      );
    } catch (error) {
      setSaveState('error');
      setMessage(error instanceof Error ? error.message : 'OpenRouter API Key 保存失败');
    }
  };

  return (
    <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className={`h-2 w-2 shrink-0 rounded-full ${configured ? 'bg-[var(--success)]' : 'bg-[var(--border-strong)]'}`} />
            <h3 className="text-sm font-semibold text-[var(--text-primary)]">OpenRouter 模型榜</h3>
            <span className="hb-chip">官方 API</span>
            {configured && (
              <span className="inline-flex items-center rounded bg-[var(--success-subtle)] px-1.5 py-0.5 text-xs text-[var(--success)]">
                凭据已配置
              </span>
            )}
          </div>
          <p className="mt-1.5 text-xs leading-5 text-[var(--text-tertiary)]">
            用于更新发现中的模型榜。正式版本只写入 macOS 钥匙串；未签名 Debug 版在钥匙串不可用时使用本机开发回退。
          </p>
        </div>
        <ShieldCheck size={18} className="mt-0.5 shrink-0 text-[var(--accent)]" />
      </div>

      <div className="mt-3 flex items-center gap-2">
        <div className="relative min-w-0 flex-1">
          <input
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
              if (event.key === 'Enter') void save();
            }}
            placeholder={configured ? '输入新 Key 可替换现有凭据' : 'sk-or-v1-…'}
            aria-label="OpenRouter API Key"
            className="input pr-10"
          />
          <button
            type="button"
            onClick={() => setShow((current) => !current)}
            className="absolute right-2.5 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-secondary)]"
            title={show ? '隐藏 API Key' : '显示 API Key'}
          >
            {show ? <EyeOff size={16} /> : <Eye size={16} />}
          </button>
        </div>
        <button
          type="button"
          onClick={() => void save()}
          disabled={!value.trim() || saveState === 'saving'}
          className="flex h-10 shrink-0 items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3.5 text-xs font-medium text-white transition-colors hover:bg-[var(--accent-strong)] disabled:opacity-50"
        >
          {saveState === 'saving' ? <Loader2 size={13} className="animate-spin" /> : <ShieldCheck size={13} />}
          {saveState === 'saving' ? '保存中…' : configured ? '更新凭据' : '安全保存'}
        </button>
      </div>
      {message && (
        <p className={`mt-2 flex items-start gap-1.5 text-xs leading-5 ${saveState === 'error' ? 'text-[var(--danger)]' : 'text-[var(--success)]'}`}>
          {saveState === 'error' ? <XCircle size={13} className="mt-0.5 shrink-0" /> : <CheckCircle2 size={13} className="mt-0.5 shrink-0" />}
          <span className="break-words">{message}</span>
        </p>
      )}
    </section>
  );
}

const SOURCE_POLICY_DEFAULTS: Record<Source['type'], { prompt: string; rule: string }> = {
  github: {
    prompt: '从技术架构、扩展点、工程成熟度、维护活跃度与真实使用价值解释仓库。',
    rule: '架构或工程创新 35%，可验证采用/热度 25%，维护质量 20%，时效性 20%；纯合集、镜像、营销仓库降分。',
  },
  arxiv: {
    prompt: '从 Research 视角解释方法创新、实验设计、基线对比、可复现性与对模型演进的意义。',
    rule: '研究新颖性 35%，实验可信度 30%，影响潜力 20%，证据完整度 15%；缺实验或仅包装旧方法降分。',
  },
  hackernews: {
    prompt: '先判断属于模型研究还是产品信号，再从原创信息、证据质量和高价值讨论中提炼结论。',
    rule: '信息增量 30%，讨论质量 25%，证据与来源 25%，时效性 20%；标题党、重复新闻、低信息评论降分。',
  },
  producthunt: {
    prompt: '从产品定位、目标用户、核心工作流、差异化、早期采用信号与商业可行性解释产品。',
    rule: '用户问题与定位 30%，产品差异化 25%，采用信号 20%，完成度 15%，时效性 10%；包装站和低完成度 Demo 降分。',
  },
  huggingface: {
    prompt: '从模型能力、评测证据、训练/部署成本、License、适用边界和 Research 价值解释。',
    rule: '能力或研究增量 30%，评测证据 25%，可用性 20%，采用信号 15%，时效性 10%；缺 Model Card 或不可验证声明降分。',
  },
  huggingface_papers: {
    prompt: '从 Research 视角解释方法创新、实验结果、可复现性以及对模型能力的实际影响。',
    rule: '研究新颖性 35%，实验可信度 30%，影响潜力 20%，证据完整度 15%；重复工作或缺少有效实验降分。',
  },
  aihot: {
    prompt: '从中文 AI 资讯视角解释：事件本身、关键主体与出处、行业影响与后续看点；只基于条目证据，不臆测未披露细节。',
    rule: '信息增量 30%，出处与证据质量 25%，行业影响 25%，时效性 20%；二手转述、软文与低信息量合集降分。',
  },
  modelscope: { prompt: '从模型能力、评测、部署与适用边界解释。', rule: '能力增量、证据、可用性与时效性综合评分。' },
  custom: { prompt: '基于来源正文与证据，解释信息价值、局限与建议行动。', rule: '信息增量、证据质量、相关性与时效性综合评分。' },
};

function DiscoveryPolicyEditor({ source }: { source: Source }) {
  const updateSourceDiscoveryConfig = useAppStore((state) => state.updateSourceDiscoveryConfig);
  const defaults = SOURCE_POLICY_DEFAULTS[source.type];
  const [generationPrompt, setGenerationPrompt] = useState(
    typeof source.config.generationPrompt === 'string' ? source.config.generationPrompt : defaults.prompt,
  );
  const [scoringRule, setScoringRule] = useState(
    typeof source.config.scoringRule === 'string' ? source.config.scoringRule : defaults.rule,
  );
  const [minScore, setMinScore] = useState(
    typeof source.config.minScore === 'number' ? source.config.minScore : 8,
  );
  const [state, setState] = useState<'idle' | 'saving' | 'saved' | 'error'>('idle');

  const save = async () => {
    setState('saving');
    try {
      await updateSourceDiscoveryConfig(source.id, generationPrompt, scoringRule, minScore);
      setState('saved');
    } catch {
      setState('error');
    }
  };

  return (
    <details className="mt-3 border-t border-[var(--border-default)] pt-3">
      <summary className="cursor-pointer select-none text-xs font-medium text-[var(--text-secondary)]">
        AI 筛选与生成规则
      </summary>
      <div className="mt-3 space-y-3">
        <label className="block">
          <span className="mb-1 block text-[13px] font-medium text-[var(--text-secondary)]">深度解读 Prompt（输出结构与关注范围）</span>
          <textarea value={generationPrompt} onChange={(event) => setGenerationPrompt(event.target.value)} rows={3} className="w-full resize-y rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] p-2.5 text-xs leading-5 text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />
        </label>
        <label className="block">
          <span className="mb-1 block text-[13px] font-medium text-[var(--text-secondary)]">过滤评分规则</span>
          <textarea value={scoringRule} onChange={(event) => setScoringRule(event.target.value)} rows={3} className="w-full resize-y rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] p-2.5 text-xs leading-5 text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />
        </label>
        <div className="flex items-end gap-3">
          <label className="block">
            <span className="mb-1 block text-[13px] font-medium text-[var(--text-secondary)]">最低入选分</span>
            <input type="number" min={0} max={10} step={0.5} value={minScore} onChange={(event) => setMinScore(Number(event.target.value))} className="h-8 w-24 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-2 text-xs text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />
          </label>
          <button type="button" onClick={() => void save()} disabled={state === 'saving'} className="h-8 rounded-lg border border-[var(--accent-border)] px-3 text-xs text-[var(--accent)] transition-colors hover:bg-[var(--accent-subtle)] disabled:opacity-50">
            {state === 'saving' ? '保存中…' : '保存规则'}
          </button>
          {state === 'saved' && <span className="pb-1 text-xs text-[var(--success)]">已保存</span>}
          {state === 'error' && <span className="pb-1 text-xs text-[var(--danger)]">保存失败</span>}
        </div>
      </div>
    </details>
  );
}

export default function Settings() {
  const { settings, updateSettings, sources, toggleSource, updateSourceInterval, updateSourceTier, updateSourceAdmission, loadSources, setSelectedItemId, stats } = useAppStore();
  const [activeTab, setActiveTab] = useState<'ai' | 'capabilities' | 'usage' | 'sources' | 'inbox' | 'storage' | 'runtime' | 'general'>('ai');
  const [settingsNavWidth, setSettingsNavWidth] = useState(192);
  useEffect(() => {
    getSetting(SETTINGS_NAV_WIDTH_KEY)
      .then((raw) => {
        const saved = Number(raw);
        if (Number.isFinite(saved)) setSettingsNavWidth(Math.max(160, Math.min(300, saved)));
      })
      .catch(() => {});
  }, []);
  // NB-12：设置→存储页（数据目录 + 笔记本容量 + 孤儿清理）
  const [storageLayout, setStorageLayout] = useState<import('../services/tauri').StorageLayoutInfo | null>(null);
  const [storageStats, setStorageStats] = useState<import('../services/tauri').StorageStats | null>(null);
  const [gcConfirm, setGcConfirm] = useState(false);
  const [gcBusy, setGcBusy] = useState(false);
  // 同步状态（每源）：loading/结果信息 + 本次同步新增条目列表（直接展示在数据源界面）
  const [sync, setSync] = useState<Record<string, { loading: boolean; ok?: boolean; msg?: string; items: Item[]; totalNew: number }>>({});
  const [coverage, setCoverage] = useState<import('../services/tauri').CoverageStats | null>(null);
  const [sidecarStatus, setSidecarStatus] = useState<import('../services/tauri').HermesSidecarStatus | null>(null);
  const [sidecarState, setSidecarState] = useState<'idle' | 'loading' | 'pulling' | 'ready' | 'error'>('idle');
  const [sidecarMessage, setSidecarMessage] = useState('');
  const [sidecarProgress, setSidecarProgress] = useState<HermesSidecarProgress | null>(null);

  // 进入数据源页时加载覆盖率统计
  const loadCoverage = async () => {
    try {
      setCoverage(await getCoverageStats());
    } catch (e) {
      console.error('Failed to load coverage stats:', e);
    }
  };
  useEffect(() => {
    if (activeTab === 'sources') loadCoverage();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab]);

  useEffect(() => {
    if (activeTab !== 'runtime') return;
    let cancelled = false;
    setSidecarState('loading');
    getHermesSidecarStatus()
      .then((status) => {
        if (cancelled) return;
        setSidecarStatus(status);
        setSidecarState(status.updateReady ? 'ready' : 'idle');
      })
      .catch((error) => {
        if (cancelled) return;
        setSidecarState('error');
        setSidecarMessage(error instanceof Error ? error.message : '无法读取 Hermes Runtime 状态');
      });
    return () => {
      cancelled = true;
    };
  }, [activeTab]);

  const handlePullSidecar = async () => {
    let unlisten: Awaited<ReturnType<typeof listenHermesSidecarProgress>> | undefined;
    try {
      // AG-26：先完成事件订阅再发命令，避免丢失检查/下载的首个阶段。
      unlisten = await listenHermesSidecarProgress((progress) => {
        setSidecarProgress(progress);
        setSidecarMessage(progress.message);
        if (progress.state === 'failed') setSidecarState('error');
      });
      setSidecarProgress(null);
      setSidecarState('pulling');
      setSidecarMessage('正在准备更新…');
      const status = await pullHermesSidecar();
      setSidecarStatus(status);
      setSidecarState('ready');
      setSidecarMessage('更新已完整校验并准备就绪，重启 SophoNote 后生效。');
    } catch (error) {
      setSidecarState('error');
      setSidecarMessage(error instanceof Error ? error.message : 'Hermes Sidecar 更新失败');
    } finally {
      unlisten?.();
    }
  };

  // NB-12：进入存储页时加载数据目录与容量统计
  useEffect(() => {
    if (activeTab !== 'storage') return;
    let cancelled = false;
    (async () => {
      try {
        const [layout, stats] = await Promise.all([getStorageLayout(), getStorageStats()]);
        if (!cancelled) {
          setStorageLayout(layout);
          setStorageStats(stats);
        }
      } catch (e) {
        console.error('Failed to load storage stats:', e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [activeTab]);

  // NB-12：孤儿资产清理（两步确认在 UI 层），结束刷新统计
  const handleGc = async () => {
    setGcBusy(true);
    try {
      const report = await gcOrphanAssets();
      setStorageStats(report.after);
    } catch (e) {
      console.error('Failed to gc orphan assets:', e);
    }
    setGcConfirm(false);
    setGcBusy(false);
  };

  // 同步数据源（与定时调度同一抓取入口）：ids 为空 = 全部启用源（一键同步）
  const runSync = async (ids?: string[]) => {
    const targets = ids ?? sources.filter((s) => s.enabled).map((s) => s.id);
    setSync((m) => {
      const next = { ...m };
      targets.forEach((id) => {
        next[id] = { loading: true, items: next[id]?.items ?? [], totalNew: 0 };
      });
      return next;
    });
    try {
      const results = await fetchSourcesNow(ids);
      for (const r of results) {
        let items: Item[] = [];
        if (r.success && r.newItems > 0) {
          try {
            // db_get_items 按 fetched_at 倒序：取前 newItems 条即本次新增（上限 8 条展示）
            items = await getItems({ sourceId: r.sourceId, limit: Math.min(r.newItems, 8) });
          } catch {
            /* 列表获取失败不影响同步结果本身 */
          }
        }
        setSync((m) => ({
          ...m,
          [r.sourceId]: {
            loading: false,
            ok: r.success,
            msg: r.success ? `新增 ${r.newItems} 条（共抓取 ${r.fetched}）` : (r.error || '抓取失败').slice(0, 80),
            items,
            totalNew: r.newItems,
          },
        }));
      }
      await loadSources();
      await loadCoverage();
    } catch (e) {
      const msg = e instanceof Error ? e.message.slice(0, 80) : '同步失败';
      setSync((m) => {
        const next = { ...m };
        targets.forEach((id) => {
          next[id] = { loading: false, ok: false, msg, items: [], totalNew: 0 };
        });
        return next;
      });
    }
  };
  const anySyncing = Object.values(sync).some((s) => s.loading);
  const sidecarProgressStep = sidecarProgress
    ? Math.max(0, SIDECAR_PROGRESS_STEPS.findIndex((step) => step.phase === sidecarProgress.phase))
    : 0;
  return (
    <div className="flex h-full">
      {/* 左侧菜单 */}
      {/* NB-24：首行 h-10 底线与侧栏/右内容首行对齐 */}
      <div
        className="bg-[var(--bg-sunken)] flex flex-col shrink-0 overflow-hidden"
        style={{ width: settingsNavWidth }}
      >
        <div className="h-10 border-b border-[var(--border-default)] flex items-center px-3 shrink-0" data-tauri-drag-region>
          <h3 className="text-xs font-bold text-[var(--text-tertiary)] uppercase tracking-wider" data-tauri-drag-region>设置</h3>
        </div>
        <div className="flex-1 overflow-y-auto p-3">
        {[
          { id: 'ai' as const, label: 'AI 配置', icon: Key },
          { id: 'capabilities' as const, label: '能力配置', icon: Boxes },
          { id: 'usage' as const, label: '用量统计', icon: BarChart3 },
          { id: 'sources' as const, label: '数据源', icon: Database },
          { id: 'inbox' as const, label: '收件箱', icon: InboxIcon },
          { id: 'runtime' as const, label: 'Hermes 更新', icon: DownloadCloud },
          { id: 'general' as const, label: '通用', icon: Palette },
        ].map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              onClick={() => setActiveTab(item.id)}
              className={`w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm transition-colors text-left ${
                activeTab === item.id
                  ? 'bg-[var(--accent-subtle)] text-[var(--accent)] font-semibold'
                  : 'text-[var(--text-secondary)] hover:bg-[var(--bg-surface)]'
              }`}
            >
              <Icon size={15} />
              {item.label}
              {item.id === 'inbox' && stats.unreadItems > 0 && (
                <span className="ml-auto text-xs font-bold px-1.5 py-0.5 rounded-full bg-[var(--accent-subtle)] text-[var(--accent)]">
                  {stats.unreadItems > 99 ? '99+' : stats.unreadItems}
                </span>
              )}
            </button>
          );
        })}
        </div>
      </div>

      <VerticalResizeHandle
        value={settingsNavWidth}
        min={160}
        max={300}
        defaultValue={192}
        onChange={setSettingsNavWidth}
        onCommit={(width) => void updateSetting(SETTINGS_NAV_WIDTH_KEY, String(Math.round(width)))}
        label="调整设置导航宽度"
      />

      {/* 右侧内容：NB-24 首行 h-10 标题栏，底线与左菜单首行对齐；各 tab 页内 h2 移除防重复 */}
      <div className="flex-1 flex flex-col min-w-0">
        <div className="h-10 border-b border-[var(--border-default)] flex items-center justify-between gap-3 px-5 shrink-0 bg-[var(--bg-surface)]" data-tauri-drag-region>
          <h2 className="text-sm font-semibold text-[var(--text-primary)]" data-tauri-drag-region>
            {activeTab === 'ai' ? 'AI 配置' : activeTab === 'capabilities' ? '能力配置' : activeTab === 'usage' ? '用量统计' : activeTab === 'sources' ? '数据源' : activeTab === 'inbox' ? '收件箱' : activeTab === 'storage' ? '存储' : activeTab === 'runtime' ? 'Hermes Sidecar 更新' : '通用设置'}
          </h2>
          {activeTab === 'sources' && (
            <button
              onClick={() => runSync()}
              disabled={anySyncing}
              className="flex items-center gap-1.5 px-3.5 py-1.5 rounded-lg text-xs font-medium bg-[var(--accent)] text-white hover:bg-[var(--accent-strong)] transition-colors disabled:opacity-50"
              title="同步全部启用的数据源"
            >
              <RefreshCw size={13} className={anySyncing ? 'animate-spin' : ''} />
              {anySyncing ? '同步中…' : '一键同步'}
            </button>
          )}
        </div>
        <div className="flex-1 overflow-y-auto p-8">
        {activeTab === 'ai' && <AIProviderSettingsPanel />}
        {activeTab === 'capabilities' && <CapabilitySettingsPanel />}
        {activeTab === 'usage' && <UsageStatisticsPanel />}

        {activeTab === 'sources' && (
          <div className="max-w-2xl">
            {/* 源健康度已移入各源卡片（成功率 / 24h 产量 / 最后成功），不再单独面板 */}

            <div className="space-y-3">
              {sources.map((source) => {
                const st = sourceConnStatus(source);
                const health = coverage?.health?.find((h) => h.id === source.id);
                const syncSt = sync[source.id];
                return (
                <div
                  key={source.id}
                  className={`p-4 rounded-xl border transition-all ${
                    source.enabled ? 'bg-[var(--bg-surface)] border-[var(--border-default)]' : 'bg-[var(--bg-sunken)] border-[var(--border-default)]'
                  }`}
                >
                  <div className="flex items-center justify-between">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        {/* 联通状态圆点：绿=最近抓取成功 / 红=最近失败 / 灰=未抓取（停用源不指示） */}
                        <span
                          className={`w-2 h-2 rounded-full shrink-0 ${
                            !source.enabled
                              ? 'bg-[var(--border-default)]'
                              : st === 'ok'
                                ? 'bg-[var(--success)]'
                                : st === 'error'
                                  ? 'bg-[var(--danger)]'
                                  : 'bg-[var(--border-strong)]'
                          }`}
                          title={
                            !source.enabled
                              ? '已停用'
                              : st === 'ok'
                                ? '正常联通'
                                : st === 'error'
                                  ? `联通异常：${source.lastError || '未知错误'}`
                                  : '尚未抓取'
                          }
                        />
                        <h3 className="text-sm font-semibold text-[var(--text-primary)]">{source.name}</h3>
                        <span className="hb-chip">
                          {source.type}
                        </span>
                        {source.enabled ? (
                          st === 'ok' ? (
                            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-xs bg-[var(--success-subtle)] text-[var(--success)]">正常联通</span>
                          ) : st === 'error' ? (
                            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-xs bg-[var(--danger-subtle)] text-[var(--danger)]">联通异常</span>
                          ) : (
                            <span className="inline-flex items-center rounded px-1.5 py-0.5 text-xs bg-[var(--bg-sunken)] text-[var(--text-tertiary)]">尚未抓取</span>
                          )
                        ) : (
                          <span className="inline-flex items-center rounded px-1.5 py-0.5 text-xs bg-[var(--bg-sunken)] text-[var(--text-tertiary)]">已停用</span>
                        )}
                      </div>
                      <div className="flex items-center gap-2 mt-0.5">
                        <span className="text-xs text-[var(--text-tertiary)]">抓取频率</span>
                        <select
                          value={source.fetchIntervalMinutes}
                          onChange={(e) =>
                            updateSourceInterval(source.id, Number(e.target.value))
                          }
                          onClick={(e) => e.stopPropagation()}
                          className="text-xs px-1.5 py-0.5 rounded border border-[var(--border-strong)] bg-[var(--bg-surface)] text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
                        >
                          <option value={30}>每 30 分钟</option>
                          <option value={60}>每 1 小时</option>
                          <option value={180}>每 3 小时</option>
                          <option value={360}>每 6 小时</option>
                          <option value={720}>每 12 小时</option>
                          <option value={1440}>每 24 小时</option>
                        </select>
                        {source.lastFetchedAt && (
                          <span className="text-xs text-[var(--text-tertiary)]">
                            · 上次 {new Date(source.lastFetchedAt).toLocaleString('zh-CN')}
                          </span>
                        )}
                      </div>
                      {/* 信源分层 + 准入状态（借鉴 ai-news-radar source_tier + 观察区） */}
                      <div className="flex items-center gap-2 mt-0.5">
                        <span className="text-xs text-[var(--text-tertiary)]">分层</span>
                        <select
                          value={source.tier}
                          onChange={(e) =>
                            updateSourceTier(source.id, e.target.value as 'core' | 'standard' | 'experimental')
                          }
                          onClick={(e) => e.stopPropagation()}
                          className="text-xs px-1.5 py-0.5 rounded border border-[var(--border-strong)] bg-[var(--bg-surface)] text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
                        >
                          <option value="core">核心</option>
                          <option value="standard">标准</option>
                          <option value="experimental">实验</option>
                        </select>
                        <span className="text-xs text-[var(--text-tertiary)]">准入</span>
                        <select
                          value={source.admission}
                          onChange={(e) =>
                            updateSourceAdmission(source.id, e.target.value as 'active' | 'probation' | 'skipped')
                          }
                          onClick={(e) => e.stopPropagation()}
                          className="text-xs px-1.5 py-0.5 rounded border border-[var(--border-strong)] bg-[var(--bg-surface)] text-[var(--text-secondary)] focus:outline-none focus:border-[var(--accent)]"
                        >
                          <option value="active">正式</option>
                          <option value="probation">试用观察期</option>
                          <option value="skipped">跳过</option>
                        </select>
                        {source.admission === 'probation' && (
                          <span className="inline-flex items-center rounded px-1.5 py-0.5 text-xs bg-[var(--warning-subtle)] text-[var(--warning)]">
                            观察期：参与抓取，不进默认视图
                          </span>
                        )}
                      </div>
                      {/* 联通异常原因（最近一次抓取失败信息） */}
                      {source.enabled && st === 'error' && source.lastError && (
                        <p className="text-xs text-[var(--danger)] mt-1 truncate" title={source.lastError}>
                          异常原因：{source.lastError.slice(0, 80)}
                        </p>
                      )}
                    </div>
                    <div className="flex items-center gap-2 shrink-0">
                      {source.enabled && (
                        <button
                          onClick={() => runSync([source.id])}
                          disabled={syncSt?.loading}
                          className="p-1.5 rounded-lg text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:bg-[var(--accent-subtle)] transition-colors disabled:opacity-50"
                          title="立即同步该数据源"
                        >
                          <RefreshCw size={14} className={syncSt?.loading ? 'animate-spin' : ''} />
                        </button>
                      )}
                      <button
                        onClick={() => toggleSource(source.id)}
                        className={`relative w-11 h-6 rounded-full transition-colors ${
                          source.enabled ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
                        }`}
                      >
                        <span
                          className={`absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform ${
                            source.enabled ? 'translate-x-5' : ''
                          }`}
                        />
                      </button>
                    </div>
                  </div>
                  {/* 源健康三件套（原独立面板移入卡片：成功率 / 24h 产量 / 最后成功） */}
                  {source.enabled && health && (
                    <div className="mt-2.5 pt-2 border-t border-[var(--border-default)] flex items-center gap-3 text-xs text-[var(--text-tertiary)]">
                      <span
                        className={
                          health.successRate >= 90
                            ? 'text-[var(--success)]'
                            : health.successRate >= 60
                              ? 'text-[var(--warning)]'
                              : 'text-[var(--danger)]'
                        }
                      >
                        成功率 {health.successRate}%
                      </span>
                      <span>24h 产 {health.yield24h} 条</span>
                      <span>共 {health.itemsTotal} 条</span>
                      <span className="ml-auto truncate" title={health.lastError || undefined}>
                        {health.lastSuccessAt
                          ? `最后成功 ${new Date(health.lastSuccessAt).toLocaleString('zh-CN')}`
                          : '尚无成功记录'}
                      </span>
                    </div>
                  )}
                  <DiscoveryPolicyEditor source={source} />
                  {/* 同步结果信息 */}
                  {syncSt?.msg && (
                    <p className={`text-xs mt-2 ${
                      syncSt.ok ? 'text-[var(--success)]' : 'text-[var(--danger)]'
                    }`}>
                      {syncSt.msg}
                    </p>
                  )}
                  {/* 本次同步新增条目列表（点击进入阅读视图） */}
                  {syncSt && !syncSt.loading && syncSt.ok && syncSt.items.length > 0 && (
                    <div className="mt-2 rounded-lg border border-[var(--border-default)] bg-[var(--bg-sunken)] overflow-hidden">
                      <p className="px-3 pt-2 pb-1 text-xs font-semibold text-[var(--text-tertiary)] uppercase tracking-wider">
                        本次同步新增 · 点击阅读
                      </p>
                      <ul className="divide-y divide-[var(--border-default)]">
                        {syncSt.items.map((it) => (
                          <li key={it.id}>
                            <button
                              onClick={() => setSelectedItemId(it.id)}
                              className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-[var(--bg-surface)] transition-colors"
                            >
                              <span className="inline-flex shrink-0 items-center rounded px-1.5 py-0.5 text-xs bg-[var(--border-default)] text-[var(--text-tertiary)]">
                                {TYPE_LABEL[it.type] || it.type}
                              </span>
                              <span className="flex-1 text-xs text-[var(--text-secondary)] truncate">{it.title}</span>
                              {it.stars != null && it.stars > 0 && (
                                <span className="text-xs text-[var(--text-tertiary)] shrink-0">▲ {compactNum(it.stars)}</span>
                              )}
                            </button>
                          </li>
                        ))}
                      </ul>
                      {syncSt.totalNew > syncSt.items.length && (
                        <p className="px-3 py-1.5 text-xs text-[var(--text-tertiary)] border-t border-[var(--border-default)]">
                          仅显示前 {syncSt.items.length} 条，其余 {syncSt.totalNew - syncSt.items.length} 条见收件箱
                        </p>
                      )}
                    </div>
                  )}
                  {/* ProductHunt developer token（存钥匙串） */}
                  {source.id === 'producthunt' && (
                    <div className="mt-3 pt-3 border-t border-[var(--border-default)]">
                      <label className="text-[13px] font-medium text-[var(--text-secondary)] mb-1.5 block">
                        Developer Token
                      </label>
                      <ApiKeyField
                        providerId="producthunt"
                        placeholder="producthunt.com/v2/oauth/applications 申请"
                      />
                    </div>
                  )}
                </div>
                );
              })}
              <OpenRouterCredentialCard />
            </div>
          </div>
        )}

        {activeTab === 'inbox' && (
          <div className="max-w-6xl">
            <InboxPanel />
          </div>
        )}

        {/* 存储页入口已从设置菜单隐藏（低优先级暂不迭代）；实现保留，恢复时把 nav 项加回即可。 */}
        {activeTab === 'storage' && (
          <div className="max-w-2xl">
            <p className="text-sm text-[var(--text-tertiary)] mb-6">
              SophoNote 的数据库、文档、Agent 工作区和私有 Hermes Runtime 统一存放在应用数据根目录。
            </p>

            {/* 数据目录 */}
            <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5 mb-4">
              <h3 className="text-base font-semibold text-[var(--text-primary)] mb-3">数据目录</h3>
              <p className="font-mono text-[13px] text-[var(--text-secondary)] break-all bg-[var(--bg-sunken)] rounded-[6px] px-3 py-2">
                {storageLayout?.root || '…'}
              </p>
              <div className="mt-3 grid gap-2 text-xs">
                {([
                  ['文档真相源（受保护）', storageLayout?.notes, '由 DocumentService、版本与 Lease 控制写入'],
                  ['Agent 工作空间', storageLayout?.workspace, 'Hermes 可直接读写这里的所有文件'],
                  ['Hermes 私有数据', storageLayout?.hermes, 'Session、Memory、Skills 与附件；不使用 ~/.hermes'],
                  ['数据库', storageLayout?.database, 'SQLite 索引与应用状态'],
                  ['运行时目录', storageLayout?.runtime, '宿主与 Sidecar 的临时运行状态'],
                  ['日志', storageLayout?.logs, 'Sidecar 与宿主诊断日志'],
                ] as const).map(([label, path, note]) => (
                  <div key={label} className="rounded-lg border border-[var(--border-default)] px-3 py-2">
                    <div className="flex items-start justify-between gap-3">
                      <span className="shrink-0 font-medium text-[var(--text-secondary)]">{label}</span>
                      <span className="min-w-0 break-all text-right font-mono text-[var(--text-tertiary)]">{path || '…'}</span>
                    </div>
                    <p className="mt-1 text-[var(--text-tertiary)]">{note}</p>
                  </div>
                ))}
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={!storageLayout}
                  onClick={() => storageLayout && void openPath(storageLayout.workspace)}
                  className="rounded-lg border border-[var(--border-strong)] px-3 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-50"
                >
                  在 Finder 中打开工作空间
                </button>
                <button
                  type="button"
                  disabled={!storageLayout}
                  onClick={() => storageLayout && void openPath(storageLayout.root)}
                  className="rounded-lg border border-[var(--border-strong)] px-3 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-50"
                >
                  打开数据根目录
                </button>
              </div>
              <p className="mt-3 text-xs leading-relaxed text-[var(--success)]">
                当前数据无需迁移：旧版 sophonote.db 与 notes/ 已在此根目录；本次仅补齐 workspace/、hermes/、runtime/ 与 logs/。
              </p>
            </div>

            {/* 笔记本容量 */}
            <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5 mb-4">
              <h3 className="text-base font-semibold text-[var(--text-primary)] mb-3">笔记本容量</h3>
              {storageStats ? (
                <div className="space-y-2 text-sm text-[var(--text-secondary)]">
                  <p className="flex justify-between">
                    <span>笔记正文</span>
                    <span className="font-mono">
                      {storageStats.note_count} 篇 · {fmtBytes(storageStats.notes_bytes)}
                    </span>
                  </p>
                  <p className="flex justify-between">
                    <span>图片资产</span>
                    <span className="font-mono">
                      {storageStats.asset_count} 个 · {fmtBytes(storageStats.assets_bytes)}
                    </span>
                  </p>
                  <p className="flex justify-between font-medium">
                    <span>合计</span>
                    <span className="font-mono">
                      {fmtBytes(storageStats.notes_bytes + storageStats.assets_bytes)}
                    </span>
                  </p>
                  {storageStats.orphan_count > 0 && (
                    <div className="pt-2 mt-1 border-t border-[var(--border-default)] flex items-center justify-between">
                      <span className="text-[var(--warning)]">
                        孤儿资产（不被任何笔记引用） {storageStats.orphan_count} 个 ·{' '}
                        {fmtBytes(storageStats.orphan_bytes)}
                      </span>
                      {gcConfirm ? (
                        <button
                          onClick={() => void handleGc()}
                          disabled={gcBusy}
                          className="btn-danger px-2.5 py-1 text-xs disabled:opacity-50"
                        >
                          {gcBusy ? '清理中…' : `确认清理 ${storageStats.orphan_count} 个？`}
                        </button>
                      ) : (
                        <button
                          onClick={() => {
                            setGcConfirm(true);
                            setTimeout(() => setGcConfirm(false), 3000);
                          }}
                          className="text-xs px-2.5 py-1 rounded-lg border border-[var(--gold-border)] text-[var(--warning)] hover:bg-[var(--warning-subtle)]"
                        >
                          清理
                        </button>
                      )}
                    </div>
                  )}
                </div>
              ) : (
                <p className="text-sm text-[var(--text-tertiary)]">加载中…</p>
              )}
            </div>

            {/* 自定义数据根必须整体迁移，不能只搬 notes。 */}
            <div className="rounded-xl border border-dashed border-[var(--border-default)] p-5 opacity-70">
              <h3 className="text-base font-semibold text-[var(--text-primary)] mb-1">切换存储地址</h3>
              <p className="text-xs text-[var(--text-tertiary)] leading-relaxed">
                规划中。未来若支持自定义位置，会原子迁移整个数据根（数据库、notes、workspace 与 Hermes 状态），
                不提供只迁笔记目录的半迁移模式。
              </p>
            </div>
          </div>
        )}

        {activeTab === 'runtime' && (
          <div className="max-w-2xl space-y-4">
            <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5">
              <div className="flex items-start justify-between gap-5">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <h3 className="text-base font-semibold text-[var(--text-primary)]">Hermes Agent Runtime</h3>
                    {sidecarStatus?.updateReady && (
                      <span className="rounded-full bg-[var(--success-subtle)] px-2 py-0.5 text-[11px] font-medium text-[var(--success)]">待重启</span>
                    )}
                  </div>
                  <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">
                    从 NousResearch 官方稳定 Release 拉取，在 SophoNote 私有目录构建并校验。不会中断当前会话或修改已签名应用文件。
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => void handlePullSidecar()}
                  disabled={sidecarState === 'pulling' || sidecarState === 'loading'}
                  className="inline-flex h-9 shrink-0 items-center gap-2 rounded-lg bg-[var(--accent)] px-4 text-sm font-medium text-white transition-colors hover:bg-[var(--accent-strong)] disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {sidecarState === 'pulling' ? <Loader2 size={15} className="animate-spin" /> : <DownloadCloud size={15} />}
                  {sidecarState === 'pulling' ? '拉取中…' : '拉取更新'}
                </button>
              </div>

              <dl className="mt-5 grid grid-cols-[7rem_minmax(0,1fr)] gap-x-4 gap-y-2 border-t border-[var(--border-default)] pt-4 text-xs">
                <dt className="text-[var(--text-tertiary)]">当前运行</dt>
                <dd className="font-mono text-[var(--text-secondary)]">
                  {sidecarStatus ? `v${sidecarStatus.currentVersion} · ${sidecarStatus.currentCommit.slice(0, 12)}` : '读取中…'}
                </dd>
                <dt className="text-[var(--text-tertiary)]">运行来源</dt>
                <dd className="text-[var(--text-secondary)]">
                  {sidecarStatus?.currentSource === 'official-update' ? '应用私有更新槽' : '随 SophoNote 分发的钉扎版本'}
                </dd>
                <dt className="text-[var(--text-tertiary)]">待启用</dt>
                <dd className="font-mono text-[var(--text-secondary)]">
                  {sidecarStatus?.pendingVersion
                    ? `v${sidecarStatus.pendingVersion} · ${sidecarStatus.pendingCommit?.slice(0, 12) ?? ''}`
                    : '无'}
                </dd>
                <dt className="text-[var(--text-tertiary)]">更新源</dt>
                <dd className="truncate text-[var(--text-secondary)]" title={sidecarStatus?.repository}>
                  NousResearch/hermes-agent · stable release
                </dd>
              </dl>

              {sidecarProgress && (sidecarState === 'pulling' || sidecarState === 'error') && (
                <div className={`mt-4 rounded-lg border px-3 py-3 ${
                  sidecarState === 'error'
                    ? 'border-[var(--danger)]/20 bg-[var(--danger-subtle)]'
                    : 'border-[var(--accent)]/20 bg-[var(--accent-subtle)]'
                }`}>
                  <div className="flex items-center justify-between gap-3 text-xs">
                    <span className={sidecarState === 'error' ? 'font-medium text-[var(--danger)]' : 'font-medium text-[var(--text-secondary)]'}>
                      {SIDECAR_PROGRESS_STEPS[sidecarProgressStep]?.label ?? '准备更新'}
                    </span>
                    <span className="font-mono tabular-nums text-[var(--text-tertiary)]">
                      {Math.round(sidecarProgress.percent)}%
                    </span>
                  </div>
                  <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-[var(--bg-sunken)]">
                    <div
                      className={`h-full rounded-full transition-[width] duration-300 ${sidecarState === 'error' ? 'bg-[var(--danger)]' : 'bg-[var(--accent)]'}`}
                      style={{ width: `${Math.max(0, Math.min(100, sidecarProgress.percent))}%` }}
                    />
                  </div>
                  <div className="mt-2 grid grid-cols-9 gap-1" aria-label="Hermes 更新阶段">
                    {SIDECAR_PROGRESS_STEPS.map((step, index) => (
                      <span
                        key={step.phase}
                        title={step.label}
                        className={`h-1 rounded-full ${
                          index <= sidecarProgressStep
                            ? sidecarState === 'error' && index === sidecarProgressStep
                              ? 'bg-[var(--danger)]'
                              : 'bg-[var(--accent)]'
                            : 'bg-[var(--border-default)]'
                        }`}
                      />
                    ))}
                  </div>
                  <p className={`mt-2 text-xs leading-5 ${sidecarState === 'error' ? 'text-[var(--danger)]' : 'text-[var(--text-tertiary)]'}`}>
                    {sidecarProgress.message}
                    {typeof sidecarProgress.bytesDownloaded === 'number' && (
                      <span className="ml-1.5 whitespace-nowrap font-mono">
                        {fmtBytes(sidecarProgress.bytesDownloaded)}
                        {typeof sidecarProgress.totalBytes === 'number' ? ` / ${fmtBytes(sidecarProgress.totalBytes)}` : ''}
                      </span>
                    )}
                  </p>
                </div>
              )}

              {(sidecarState === 'ready' || (sidecarMessage && sidecarState !== 'pulling' && !sidecarProgress)) && (
                <p className={`mt-4 flex items-start gap-2 rounded-lg px-3 py-2.5 text-xs leading-5 ${
                  sidecarState === 'error'
                    ? 'bg-[var(--danger-subtle)] text-[var(--danger)]'
                    : 'bg-[var(--success-subtle)] text-[var(--success)]'
                }`}>
                  {sidecarState === 'error' ? <XCircle size={14} className="mt-0.5 shrink-0" /> : <CheckCircle2 size={14} className="mt-0.5 shrink-0" />}
                  <span>{sidecarMessage || '更新已准备就绪，重启 SophoNote 后生效。'}</span>
                </p>
              )}
            </div>

            <p className="px-1 text-xs leading-5 text-[var(--text-tertiary)]">
              新 Runtime 只会在下次启动通过完整性与健康检查后启用；如果启动失败，SophoNote 会自动回退到随包版本。
            </p>
          </div>
        )}

        {activeTab === 'general' && (
          <div className="max-w-lg">
            <div className="space-y-5">
              <div>
                <label className="text-[13px] font-medium text-[var(--text-secondary)] mb-1.5 block">主题</label>
                <div className="flex gap-2">
                  {(['light', 'dark', 'system'] as const).map((t) => (
                    <button
                      key={t}
                      onClick={() => updateSettings({ theme: t })}
                      className={`px-4 py-2 rounded-lg text-sm font-medium border transition-colors ${
                        settings.theme === t
                          ? 'border-[var(--accent)] bg-[var(--accent-subtle)] text-[var(--accent)]'
                          : 'border-[var(--border-default)] text-[var(--text-secondary)] hover:border-[var(--border-strong)]'
                      }`}
                    >
                      {t === 'light' ? '浅色' : t === 'dark' ? '深色' : '跟随系统'}
                    </button>
                  ))}
                </div>
              </div>

              <div>
                <label className="text-[13px] font-medium text-[var(--text-secondary)] mb-1.5 block">自动抓取间隔</label>
                <div className="flex items-center gap-3">
                  <input
                    type="range"
                    min={1}
                    max={24}
                    value={settings.fetchIntervalHours}
                    onChange={(e) => updateSettings({ fetchIntervalHours: Number(e.target.value) })}
                    className="flex-1"
                  />
                  <span className="text-sm text-[var(--text-secondary)] w-16 text-right">
                    {settings.fetchIntervalHours} 小时
                  </span>
                </div>
              </div>

              <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--border-default)]">
                <div>
                  <p className="text-[13px] font-medium text-[var(--text-secondary)]">桌面通知</p>
                  <p className="text-xs text-[var(--text-tertiary)]">新内容到达时推送通知</p>
                </div>
                <button
                  onClick={() => updateSettings({ notificationEnabled: !settings.notificationEnabled })}
                  className={`relative w-11 h-6 rounded-full transition-colors ${
                    settings.notificationEnabled ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform ${
                      settings.notificationEnabled ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>

              <div className="flex items-center justify-between p-3 rounded-lg border border-[var(--border-default)]">
                <div>
                  <p className="text-[13px] font-medium text-[var(--text-secondary)]">自动抓取</p>
                  <p className="text-xs text-[var(--text-tertiary)]">应用启动时自动开始数据抓取</p>
                </div>
                <button
                  onClick={() => updateSettings({ autoFetch: !settings.autoFetch })}
                  className={`relative w-11 h-6 rounded-full transition-colors ${
                    settings.autoFetch ? 'bg-[var(--accent)]' : 'bg-[var(--border-strong)]'
                  }`}
                >
                  <span
                    className={`absolute top-0.5 left-0.5 w-5 h-5 rounded-full bg-white shadow transition-transform ${
                      settings.autoFetch ? 'translate-x-5' : ''
                    }`}
                  />
                </button>
              </div>
            </div>
          </div>
        )}
        </div>
      </div>
    </div>
  );
}
