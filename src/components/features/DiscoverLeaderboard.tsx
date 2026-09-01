import { useEffect, useMemo, useState } from 'react';
import {
  BarChart3, Boxes, Braces, Check, CircleDollarSign, ExternalLink,
  Gauge, Loader2, Radar, Trophy,
} from 'lucide-react';
import * as tauri from '../../services/tauri';

type AnyRecord = Record<string, unknown>;
type BoardTab = 'top' | 'tasks' | 'cost' | 'market' | 'benchmarks' | 'capabilities';

const COLORS = ['#9dd400', '#18c7a5', '#2a8cff', '#7d67e8', '#f062a6', '#ff9f43', '#e6d44c', '#5cc96b'];

const tabs: Array<{ id: BoardTab; label: string; icon: typeof Trophy }> = [
  { id: 'top', label: '热门模型', icon: Trophy },
  { id: 'tasks', label: '任务榜', icon: Boxes },
  { id: 'cost', label: '会话成本', icon: CircleDollarSign },
  { id: 'market', label: '市场份额', icon: BarChart3 },
  { id: 'benchmarks', label: '基准', icon: Radar },
  { id: 'capabilities', label: '能力', icon: Braces },
];

function record(value: unknown): AnyRecord { return value && typeof value === 'object' && !Array.isArray(value) ? value as AnyRecord : {}; }
function records(value: unknown): AnyRecord[] { return Array.isArray(value) ? value.filter((v): v is AnyRecord => !!v && typeof v === 'object' && !Array.isArray(v)) : []; }
function text(value: unknown, fallback = '—'): string { return typeof value === 'string' && value.trim() ? value : fallback; }
function number(value: unknown): number { const n = typeof value === 'number' ? value : Number(value); return Number.isFinite(n) ? n : 0; }
function compact(value: number): string {
  if (value >= 1e12) return `${(value / 1e12).toFixed(value >= 1e13 ? 1 : 2)}T`;
  if (value >= 1e9) return `${(value / 1e9).toFixed(1)}B`;
  if (value >= 1e6) return `${(value / 1e6).toFixed(1)}M`;
  if (value >= 1e3) return `${(value / 1e3).toFixed(1)}K`;
  return Math.round(value).toLocaleString();
}
function modelLabel(id: string, modelMap: Map<string, AnyRecord>): string {
  return text(modelMap.get(id)?.name, id.split('/').pop()?.split('-').join(' ') || id);
}
function vendorOf(id: string): string { return id === 'other' ? 'Others' : (id.split('/')[0] || 'unknown'); }

interface RankedModel { id: string; total: number; previous: number; trend: number | null; }

function aggregate(rows: AnyRecord[], dates: Set<string>, key: 'model' | 'vendor'): Map<string, number> {
  const result = new Map<string, number>();
  for (const row of rows) {
    const date = text(row.date, '');
    if (!dates.has(date)) continue;
    const model = text(row.model_permaslug, 'other');
    const id = key === 'vendor' ? vendorOf(model) : model;
    result.set(id, (result.get(id) || 0) + number(row.total_tokens));
  }
  return result;
}

function SectionTitle({ eyebrow, title, note }: { eyebrow: string; title: string; note: string }) {
  return <div className="or-section-title"><span>{eyebrow}</span><h4>{title}</h4><p>{note}</p></div>;
}

function EmptyBoard({ configured }: { configured: boolean }) {
  return (
    <div className="or-empty">
      <span className="or-kicker">OPENROUTER MODEL INTELLIGENCE</span>
      <h4>{configured ? '等待首份模型榜快照' : '尚未连接 OpenRouter'}</h4>
      <p>{configured ? '凭据已配置。每日计划任务执行后，这里会显示完整排名。' : '请先前往「设置 → 数据源」配置 OpenRouter API Key。'}</p>
      <p className="or-empty-hint">配置后可在对话中说“更新 OpenRouter 模型榜”，或等待每日计划任务。</p>
    </div>
  );
}

function UsageBars({ rows, dates, leaders, keyMode = 'model' }: { rows: AnyRecord[]; dates: string[]; leaders: string[]; keyMode?: 'model' | 'vendor' }) {
  const series = useMemo(() => dates.map((date) => {
    const dayRows = rows.filter((row) => text(row.date, '') === date);
    const totals = new Map<string, number>();
    for (const row of dayRows) {
      const model = text(row.model_permaslug, 'other');
      const id = keyMode === 'vendor' ? vendorOf(model) : model;
      totals.set(id, (totals.get(id) || 0) + number(row.total_tokens));
    }
    const total = [...totals.values()].reduce((sum, value) => sum + value, 0) || 1;
    return { date, total, totals };
  }), [dates, keyMode, rows]);
  const max = Math.max(...series.map((day) => day.total), 1);
  return <div className="or-bars" aria-label="OpenRouter usage timeline">
    {series.map((day, index) => <div className="or-bar-column" key={day.date} title={`${day.date} · ${compact(day.total)} tokens`}>
      <div className="or-bar-stack" style={{ height: `${Math.max(8, (day.total / max) * 100)}%` }}>
        {leaders.map((id, colorIndex) => {
          const share = (day.totals.get(id) || 0) / day.total;
          return share > 0 ? <span key={id} style={{ height: `${share * 100}%`, background: COLORS[colorIndex % COLORS.length] }} /> : null;
        })}
      </div>
      {(index === 0 || index === series.length - 1 || index % 7 === 0) && <time>{day.date.slice(5)}</time>}
    </div>)}
  </div>;
}

function RankList({ ranked, modelMap }: { ranked: RankedModel[]; modelMap: Map<string, AnyRecord> }) {
  const max = Math.max(...ranked.map((item) => item.total), 1);
  return <div className="or-rank-grid">{ranked.map((item, index) => <div className="or-rank-row" key={item.id}>
    <span className="or-rank-no">{String(index + 1).padStart(2, '0')}</span>
    <div className="or-model-ident"><strong>{modelLabel(item.id, modelMap)}</strong><small>{vendorOf(item.id)}</small></div>
    <div className="or-mini-track"><span style={{ width: `${(item.total / max) * 100}%` }} /></div>
    <div className="or-rank-metric"><strong>{compact(item.total)}</strong><small className={item.trend != null && item.trend >= 0 ? 'up' : 'down'}>{item.trend == null ? 'NEW' : `${item.trend >= 0 ? '↑' : '↓'}${Math.abs(item.trend).toFixed(0)}%`}</small></div>
  </div>)}</div>;
}

export default function DiscoverLeaderboard() {
  const [snapshot, setSnapshot] = useState<tauri.OpenRouterRankingSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [configured, setConfigured] = useState(false);
  const [tab, setTab] = useState<BoardTab>('top');
  const [costApp, setCostApp] = useState('');

  const load = async () => {
    setLoading(true);
    try {
      const [data, hasKey] = await Promise.all([tauri.getOpenRouterRankings(), tauri.hasApiKey('openrouter-rankings')]);
      setSnapshot(data); setConfigured(hasKey);
    } catch (error) { console.error('Failed to load OpenRouter rankings:', error); }
    finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, []);

  const models = useMemo(() => records(snapshot?.models), [snapshot]);
  const modelMap = useMemo(() => new Map(models.map((model) => [text(model.id, ''), model])), [models]);
  const usage = useMemo(() => records(snapshot?.rankingsDaily), [snapshot]);
  const dates = useMemo(() => [...new Set(usage.map((row) => text(row.date, '')).filter(Boolean))].sort(), [usage]);
  const currentDates = useMemo(() => new Set(dates.slice(-7)), [dates]);
  const previousDates = useMemo(() => new Set(dates.slice(-14, -7)), [dates]);
  const ranking = useMemo(() => {
    const current = aggregate(usage, currentDates, 'model');
    const previous = aggregate(usage, previousDates, 'model');
    return [...current.entries()].filter(([id]) => id !== 'other').map(([id, total]) => {
      const prior = previous.get(id) || 0;
      return { id, total, previous: prior, trend: prior > 0 ? ((total - prior) / prior) * 100 : null };
    }).sort((a, b) => b.total - a.total).slice(0, 10);
  }, [currentDates, previousDates, usage]);
  const vendorRanking = useMemo(() => [...aggregate(usage, currentDates, 'vendor').entries()].sort((a, b) => b[1] - a[1]).slice(0, 10), [currentDates, usage]);
  const tasksRoot = record(snapshot?.taskClassifications);
  const taskRows = records(tasksRoot.classifications);
  const macros = records(tasksRoot.macro_categories);
  const costs = useMemo(() => records(snapshot?.sessionCost), [snapshot]);
  const apps = useMemo(() => [...new Map(costs.map((row) => [text(row.app_slug, ''), text(row.app_name, '')])).entries()].filter(([id]) => id), [costs]);
  useEffect(() => { if (!costApp && apps.length) setCostApp(apps[0][0]); }, [apps, costApp]);
  const benchmarkRows = useMemo(() => records(snapshot?.benchmarks).filter((row) => number(row.intelligence_index) > 0).sort((a, b) => number(b.intelligence_index) - number(a.intelligence_index)), [snapshot]);

  if (loading) return <div className="hb-d-state-wrap py-24"><Loader2 className="animate-spin" size={18} /></div>;
  if (!snapshot) return <div className="hb-d-section-shell"><EmptyBoard configured={configured} /></div>;

  const totalCurrent = [...aggregate(usage, currentDates, 'model').values()].reduce((sum, value) => sum + value, 0);
  const filteredCosts = costs.filter((row) => text(row.app_slug, '') === costApp).sort((a, b) => number(b.median_session_cost_usd) - number(a.median_session_cost_usd));

  return <div className="hb-d-section-shell or-board">
    <header className="or-hero">
      <div><span className="or-kicker">OPENROUTER · LIVE MODEL INTELLIGENCE</span><h4>AI Model Rankings</h4><p>真实使用、任务分布、Agent 会话成本与公开基准的同一快照。</p></div>
      <a href={snapshot.sourceUrl} target="_blank" rel="noreferrer">OpenRouter <ExternalLink size={13} /></a>
    </header>
    <div className="or-stat-strip">
      <div><span>7 日总用量</span><strong>{compact(totalCurrent)}</strong><small>TOKENS</small></div>
      <div><span>模型目录</span><strong>{models.length}</strong><small>MODELS</small></div>
      <div><span>任务分类</span><strong>{taskRows.length}</strong><small>TASKS</small></div>
      <div><span>数据截至</span><strong className="or-date-stat">{snapshot.asOf.slice(0, 10)}</strong><small>UTC</small></div>
    </div>
    <nav className="or-tabs">{tabs.map(({ id, label, icon: Icon }) => <button key={id} onClick={() => setTab(id)} className={tab === id ? 'is-active' : ''}><Icon size={14} />{label}</button>)}</nav>

    {tab === 'top' && <section className="or-section">
      <SectionTitle eyebrow="01 · WEEKLY USAGE" title="热门模型" note="OpenRouter 最近 7 天 token 使用量；趋势对比此前 7 天。" />
      <UsageBars rows={usage} dates={dates.slice(-30)} leaders={ranking.slice(0, 8).map((item) => item.id)} />
      <RankList ranked={ranking} modelMap={modelMap} />
    </section>}

    {tab === 'tasks' && <section className="or-section">
      <SectionTitle eyebrow="02 · TASK MAP" title="任务领先模型" note="过去 7 天抽样任务份额；面积与列表均按 OpenRouter 原始 share。" />
      <div className="or-macro-grid">{macros.map((macro, index) => <div key={text(macro.key, String(index))} style={{ '--macro': COLORS[index % 4] } as React.CSSProperties}><span>{text(macro.label)}</span><strong>{(number(macro.token_share) * 100).toFixed(1)}%</strong><small>token share</small></div>)}</div>
      <div className="or-task-grid">{taskRows.slice().sort((a, b) => number(b.token_share) - number(a.token_share)).map((task) => <article key={text(task.tag)}>
        <div><small>{text(task.macro_category).toUpperCase()}</small><strong>{text(task.display_name)}</strong><em>{(number(task.token_share) * 100).toFixed(1)}%</em></div>
        <ol>{records(task.models).slice(0, 3).map((model, index) => <li key={text(model.id)}><span>{index + 1}</span><b>{modelLabel(text(model.id), modelMap)}</b><small>{(number(model.tag_token_share) * 100).toFixed(1)}%</small></li>)}</ol>
      </article>)}</div>
    </section>}

    {tab === 'cost' && <section className="or-section">
      <SectionTitle eyebrow="03 · AGENT ECONOMICS" title="会话成本" note="OpenRouter 发布的真实 harness 会话成本中位数，不是价格估算。" />
      <div className="or-app-tabs">{apps.map(([id, name]) => <button key={id} className={costApp === id ? 'is-active' : ''} onClick={() => setCostApp(id)}>{name}</button>)}</div>
      <div className="or-cost-table"><div className="or-cost-head"><span>模型</span><span>会话轮次</span><span>中位成本</span></div>{filteredCosts.map((row) => <div key={`${text(row.model_permaslug)}:${text(row.turn_range)}`}><strong>{modelLabel(text(row.model_permaslug), modelMap)}</strong><span>{text(row.turn_range).split('-').join(' ')}</span><b>${number(row.median_session_cost_usd).toFixed(3)}</b></div>)}</div>
    </section>}

    {tab === 'market' && <section className="or-section">
      <SectionTitle eyebrow="04 · MARKET SHARE" title="模型厂商份额" note="按最近 7 天 token 使用量聚合；仅反映 OpenRouter 流量。" />
      <UsageBars rows={usage} dates={dates.slice(-30)} leaders={vendorRanking.slice(0, 8).map(([id]) => id)} keyMode="vendor" />
      <div className="or-vendor-grid">{vendorRanking.map(([vendor, total], index) => <div key={vendor}><span style={{ background: COLORS[index % COLORS.length] }} /><strong>{vendor}</strong><b>{compact(total)}</b><small>{totalCurrent ? ((total / totalCurrent) * 100).toFixed(1) : '0'}%</small></div>)}</div>
    </section>}

    {tab === 'benchmarks' && <section className="or-section">
      <SectionTitle eyebrow="05 · INTELLIGENCE / COST" title="公开基准" note="Artificial Analysis 指标与 OpenRouter 输入价格并列展示，不合成为隐藏总分。" />
      <div className="or-benchmark-grid">{benchmarkRows.slice(0, 20).map((row, index) => {
        const pricing = record(row.pricing); const input = number(pricing.prompt) * 1e6;
        return <article key={text(row.model_permaslug, String(index))}><span>{String(index + 1).padStart(2, '0')}</span><div><strong>{text(row.display_name, modelLabel(text(row.model_permaslug), modelMap))}</strong><small>{vendorOf(text(row.model_permaslug))}</small></div><dl><dt>INT</dt><dd>{number(row.intelligence_index).toFixed(1)}</dd><dt>CODE</dt><dd>{number(row.coding_index).toFixed(1)}</dd><dt>AGENT</dt><dd>{number(row.agentic_index).toFixed(1)}</dd><dt>IN / 1M</dt><dd>${input.toFixed(2)}</dd></dl></article>;
      })}</div>
    </section>}

    {tab === 'capabilities' && <section className="or-section">
      <SectionTitle eyebrow="06 · MODEL SURFACES" title="能力与上下文" note="从 OpenRouter 模型目录读取上下文、模态与工具参数支持。" />
      <div className="or-cap-grid">{models.slice(0, 60).map((model) => {
        const architecture = record(model.architecture); const params = Array.isArray(model.supported_parameters) ? model.supported_parameters.map(String) : [];
        const input = Array.isArray(architecture.input_modalities) ? architecture.input_modalities.map(String) : [];
        const output = Array.isArray(architecture.output_modalities) ? architecture.output_modalities.map(String) : [];
        return <article key={text(model.id)}><div><strong>{text(model.name)}</strong><small>{vendorOf(text(model.id))}</small></div><b><Gauge size={13} />{compact(number(model.context_length))} ctx</b><p>{[...input.map((v) => `IN:${v}`), ...output.map((v) => `OUT:${v}`)].join(' · ')}</p><footer>{params.includes('tools') || params.includes('tool_choice') ? <span><Check size={11} />Tools</span> : null}{params.includes('structured_outputs') || params.includes('response_format') ? <span><Check size={11} />Structured</span> : null}</footer></article>;
      })}</div>
    </section>}

    <footer className="or-source"><span>{snapshot.citation}</span><time>Fetched {new Date(snapshot.fetchedAt).toLocaleString('zh-CN')}</time></footer>
  </div>;
}
