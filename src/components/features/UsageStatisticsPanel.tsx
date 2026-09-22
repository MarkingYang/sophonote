import { useEffect, useMemo, useState } from 'react';
import { Activity, Banknote, Coins, Loader2, MessageSquareText, RefreshCw } from 'lucide-react';
import { agentUsage, fillAgentUsageDays, selectUsage, usageEngineLabels, type AgentUsageReport, type UsageDay, type UsageEngine } from '../../services/agentUsage';
import {
  formatEstimatedCostCny,
} from '../../services/usagePresentation';

type UsageDays = 7 | 30 | 90;

const formatCount = (value: number) => new Intl.NumberFormat('zh-CN').format(Math.round(value || 0));

function DailyUsageChart({ rows, days }: { rows: UsageDay[]; days: UsageDays }) {
  const daily = useMemo(() => fillAgentUsageDays(rows, days), [days, rows]);
  const maxTokens = Math.max(1, ...daily.map((row) => row.totalTokens));
  const labelEvery = days === 7 ? 1 : days === 30 ? 5 : 15;
  const chartWidth = Math.max(560, daily.length * (days === 90 ? 22 : 28));

  return (
    <div className="overflow-x-auto pb-1" tabIndex={0} aria-label={`近 ${days} 天每日 Token 柱状图`}>
      <div className="relative h-56" style={{ minWidth: chartWidth }}>
        {[1, 0.5, 0].map((ratio) => (
          <div
            key={ratio}
            className="pointer-events-none absolute left-12 right-0 border-t border-[var(--border-default)]"
            style={{ top: `${8 + (1 - ratio) * 192}px` }}
          >
            <span className="absolute -left-12 -top-2.5 w-10 text-right text-[10px] tabular-nums text-[var(--text-tertiary)]">
              {formatCount(maxTokens * ratio)}
            </span>
          </div>
        ))}

        <div className="absolute bottom-6 left-12 right-0 top-2 flex items-end gap-1.5">
          {daily.map((row, index) => {
            const total = row.totalTokens;
            const height = total > 0 ? Math.max(3, (total / maxTokens) * 100) : 0;
            const showLabel = index % labelEvery === 0 || index === daily.length - 1;
            const details = `${row.day}：${formatCount(total)} Token，${row.partialCost && row.estimatedCost != null ? '已知预估费用' : '预估费用'} ${formatEstimatedCostCny(row.estimatedCost ?? 0)}，${formatCount(row.apiCalls)} 次调用`;
            return (
              <div key={row.day} className="group relative flex min-w-0 flex-1 flex-col items-center justify-end" style={{ height: '100%' }}>
                <div
                  role="img"
                  aria-label={details}
                  title={details}
                  className="w-full max-w-5 rounded-t-[4px] bg-[var(--accent)] opacity-85 transition-[height,opacity] duration-200 hover:opacity-100"
                  style={{ height: `${height}%` }}
                />
                {showLabel && (
                  <span className="absolute -bottom-5 whitespace-nowrap text-[10px] tabular-nums text-[var(--text-tertiary)]">
                    {row.day.slice(5)}
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

export default function UsageStatisticsPanel() {
  const [days, setDays] = useState<UsageDays>(30);
  const [model, setModel] = useState('all');
  const [engine, setEngine] = useState<UsageEngine | 'all'>('all');
  const [report, setReport] = useState<AgentUsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError('');
    setReport(null);
    agentUsage(days)
      .then((next) => {
        if (cancelled) return;
        setReport(next);
        setModel('all');
      })
      .catch((reason) => {
        if (cancelled) return;
        setReport(null);
        setError(reason instanceof Error ? reason.message : '读取模型用量失败');
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => { cancelled = true; };
  }, [days, refreshKey]);

  const selected = useMemo(() => report ? selectUsage(report, engine, model) : null, [report, engine, model]);
  const summary = selected?.summary;
  const availableModels = [...new Set(report?.sources.filter((source) => engine === 'all' || source.engine === engine).flatMap((source) => source.byModel.map((row) => row.model)) ?? [])].sort();

  return (
    <div className="max-w-5xl space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h3 className="text-base font-semibold text-[var(--text-primary)]">模型用量</h3>
          <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">
            汇总各 Sidecar 的会话、模型调用、Token 与费用。
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <select
            value={days}
            onChange={(event) => setDays(Number(event.target.value) as UsageDays)}
            className="input h-9 w-28 py-1 text-sm"
            aria-label="统计时间范围"
          >
            <option value={7}>近 7 天</option>
            <option value={30}>近 30 天</option>
            <option value={90}>近 90 天</option>
          </select>
          <select value={engine} onChange={(event) => { setEngine(event.target.value as UsageEngine | 'all'); setModel('all'); }} className="input h-9 w-36 py-1 text-sm" aria-label="Sidecar 筛选">
            <option value="all">全部 Sidecar</option>
            {Object.entries(usageEngineLabels).map(([key, label]) => <option key={key} value={key}>{label}</option>)}
          </select>
          <select
            value={model}
            onChange={(event) => setModel(event.target.value)}
            disabled={!report || availableModels.length === 0}
            className="input h-9 w-48 max-w-full py-1 text-sm disabled:opacity-50"
            aria-label="模型筛选"
          >
            <option value="all">全部模型</option>
            {availableModels.map((name) => (
              <option key={name} value={name}>{name}</option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => setRefreshKey((value) => value + 1)}
            disabled={loading}
            className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-[var(--border-default)] px-3 text-xs font-medium text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] disabled:opacity-50"
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} /> 刷新
          </button>
        </div>
      </div>

      {loading && !report && (
        <div className="flex min-h-52 items-center justify-center rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] text-sm text-[var(--text-tertiary)]">
          <Loader2 size={16} className="mr-2 animate-spin" /> 正在读取各 Sidecar 用量…
        </div>
      )}

      {error && (
        <div className="rounded-xl border border-[var(--danger)] bg-[var(--danger-subtle)] p-4">
          <p className="text-sm font-medium text-[var(--danger)]">暂时无法读取用量</p>
          <p className="mt-1 text-xs leading-5 text-[var(--text-secondary)]">{error}</p>
          <button type="button" onClick={() => setRefreshKey((value) => value + 1)} className="mt-3 text-xs font-medium text-[var(--accent)] hover:underline">
            重新读取
          </button>
        </div>
      )}

      {report && selected && summary && (
        <>
          {selected.sources.some((source) => source.error) && (
            <div role="status" className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-sunken)] p-3 text-xs text-[var(--text-secondary)]">
              <p className="font-medium">部分用量暂不可用，以下显示已读取的数据</p>
              {selected.sources.filter((source) => source.error).map((source) => <p key={source.engine} className="mt-1">{usageEngineLabels[source.engine]}：{source.error}</p>)}
            </div>
          )}
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {[
              { label: `近 ${days} 天会话`, value: summary.sessions, icon: MessageSquareText },
              { label: '模型调用', value: summary.apiCalls, icon: Activity },
              { label: '已记录 Token', value: summary.totalTokens, icon: Coins },
              { label: summary.partialCost && summary.estimatedCost != null ? '已知预估费用' : '预估费用', value: summary.estimatedCost ?? 0, icon: Banknote, currency: true },
            ].map((card) => {
              const Icon = card.icon;
              return (
                <div key={card.label} className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
                  <Icon size={16} className="text-[var(--accent)]" />
                  <p className="mt-4 text-2xl font-semibold tabular-nums text-[var(--text-primary)]">
                    {card.currency ? formatEstimatedCostCny(card.value) : formatCount(card.value)}
                  </p>
                  <p className="mt-1 text-xs text-[var(--text-tertiary)]">{card.label}</p>
                </div>
              );
            })}
          </div>

          <div className="grid grid-cols-2 gap-3 xl:grid-cols-5">
            {[
              ['输入 Token', summary.inputTokens],
              ['输出 Token', summary.outputTokens],
              ['缓存读取', summary.cacheReadTokens],
              ['缓存写入', summary.cacheWriteTokens],
              ['推理（已含在输出）', summary.reasoningTokens],
            ].map(([label, value]) => (
              <div key={String(label)} className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-sunken)] px-3 py-2.5">
                <p className="text-[11px] text-[var(--text-tertiary)]">{label}</p>
                <p className="mt-1 text-sm font-semibold tabular-nums text-[var(--text-primary)]">
                  {value == null ? '—' : formatCount(Number(value))}
                </p>
              </div>
            ))}
          </div>

          {selected.dailyComplete && selected.daily.length > 0 && (
            <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
              <div className="mb-4 flex items-center justify-between gap-3">
                <div>
                  <h4 className="text-sm font-semibold text-[var(--text-primary)]">每日用量</h4>
                  <p className="mt-1 text-[11px] text-[var(--text-tertiary)]">{engine === 'all' ? '全部 Sidecar' : usageEngineLabels[engine]} · {model === 'all' ? '全部模型' : model}</p>
                </div>
                <span className="text-[11px] text-[var(--text-tertiary)]">柱高代表 Token</span>
              </div>
              <DailyUsageChart rows={selected.daily} days={days} />
              <div className="mt-3 border-t border-[var(--border-default)] pt-3 text-[11px] text-[var(--text-tertiary)]">
                <span>悬停柱形可查看当日 Token、调用与费用</span>
              </div>
            </section>
          )}

          {!selected.dailyComplete && <p className="text-xs text-[var(--text-tertiary)]">当前筛选包含未提供逐日模型用量的 Sidecar，暂不展示趋势。</p>}
          <section aria-label="按 Sidecar 统计" className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {report.sources.map((source) => {
              const compared = selectUsage(report, source.engine, model).summary;
              return (
              <button type="button" key={source.engine} onClick={() => { setEngine(source.engine); setModel('all'); }} aria-pressed={engine === source.engine}
                className={`rounded-xl border p-4 text-left transition-colors hover:bg-[var(--bg-sunken)] ${engine === source.engine ? 'border-[var(--accent)] bg-[var(--accent-subtle)]' : 'border-[var(--border-default)] bg-[var(--bg-surface)]'}`}>
                <p className="text-sm font-medium text-[var(--text-primary)]">{usageEngineLabels[source.engine]}</p>
                <p className="mt-2 text-lg font-semibold tabular-nums text-[var(--text-primary)]">{source.error && compared.apiCalls === 0 ? '—' : formatCount(compared.totalTokens)} <span className="text-xs font-normal">Token</span></p>
                <p className="mt-1 text-xs text-[var(--text-tertiary)]">{source.error ? '部分数据不可用' : `${formatCount(compared.apiCalls)} 次调用 · ${model === 'all' ? source.byModel.length : source.byModel.filter((row) => row.model === model).length} 个模型`}</p>
              </button>
            ); })}
          </section>
          <section className="overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]">
            <div className="border-b border-[var(--border-default)] px-4 py-3">
              <h4 className="text-sm font-semibold text-[var(--text-primary)]">按模型统计</h4>
            </div>
            {selected.models.length === 0 ? (
              <div className="px-4 py-12 text-center text-sm text-[var(--text-tertiary)]">当前时间范围内还没有模型用量</div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full min-w-[680px] text-left text-xs">
                  <thead className="bg-[var(--bg-sunken)] text-[var(--text-tertiary)]">
                    <tr>
                      <th className="px-4 py-2.5 font-medium">Sidecar / 模型</th>
                      <th className="px-4 py-2.5 text-right font-medium">会话</th>
                      <th className="px-4 py-2.5 text-right font-medium">调用</th>
                      <th className="px-4 py-2.5 text-right font-medium">输入 Token</th>
                      <th className="px-4 py-2.5 text-right font-medium">输出 Token</th>
                      <th className="px-4 py-2.5 text-right font-medium">预估费用</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--border-default)]">
                    {selected.models.map((row) => (
                      <tr key={`${row.engine}:${row.model}`} className="hover:bg-[var(--bg-sunken)]">
                        <td className="px-4 py-3 font-mono font-medium text-[var(--text-primary)]"><span className="mb-1 block font-sans text-[10px] text-[var(--text-tertiary)]">{usageEngineLabels[row.engine]}</span>{row.model}</td>
                        <td className="px-4 py-3 text-right tabular-nums text-[var(--text-secondary)]">{formatCount(row.sessions)}</td>
                        <td className="px-4 py-3 text-right tabular-nums text-[var(--text-secondary)]">{formatCount(row.apiCalls)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatCount(row.inputTokens)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatCount(row.outputTokens)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatEstimatedCostCny(row.estimatedCost ?? 0)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        </>
      )}
    </div>
  );
}
