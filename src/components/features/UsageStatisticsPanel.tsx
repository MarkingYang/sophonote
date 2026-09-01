import { useEffect, useMemo, useState } from 'react';
import { Activity, Banknote, Coins, Loader2, MessageSquareText, RefreshCw } from 'lucide-react';
import { hermesUsage, type HermesUsageReport } from '../../services/tauri';
import {
  fillDailyUsage,
  formatEstimatedCostCny,
  usageDayTotalTokens,
} from '../../services/usagePresentation';

type UsageDays = 7 | 30 | 90;

const formatCount = (value: number) => new Intl.NumberFormat('zh-CN').format(Math.round(value || 0));

function DailyUsageChart({ report, days }: { report: HermesUsageReport; days: UsageDays }) {
  const daily = useMemo(() => fillDailyUsage(report.daily, days), [days, report.daily]);
  const maxTokens = Math.max(1, ...daily.map(usageDayTotalTokens));
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
            const total = usageDayTotalTokens(row);
            const height = total > 0 ? Math.max(3, (total / maxTokens) * 100) : 0;
            const showLabel = index % labelEvery === 0 || index === daily.length - 1;
            const details = `${row.day}：${formatCount(total)} Token，预估费用 ${formatEstimatedCostCny(row.estimatedCost)}，${formatCount(row.apiCalls)} 次调用`;
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
  const [report, setReport] = useState<HermesUsageReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError('');
    hermesUsage(days)
      .then((next) => {
        if (cancelled) return;
        setReport(next);
        setModel((current) => (
          current === 'all' || next.byModel.some((row) => row.model === current) ? current : 'all'
        ));
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

  const selected = useMemo(
    () => report?.byModel.find((row) => row.model === model) ?? null,
    [model, report],
  );
  const summary = useMemo(() => {
    if (!report) return null;
    if (selected) {
      return {
        sessions: selected.sessions,
        calls: selected.apiCalls,
        input: selected.inputTokens,
        output: selected.outputTokens,
        cache: 0,
        reasoning: 0,
        total: selected.inputTokens + selected.outputTokens,
        cost: selected.estimatedCost,
        completeTokenBreakdown: false,
      };
    }
    const totals = report.totals;
    return {
      sessions: totals.totalSessions,
      calls: totals.totalApiCalls,
      input: totals.totalInput,
      output: totals.totalOutput,
      cache: totals.totalCacheRead,
      reasoning: totals.totalReasoning,
      total: totals.totalInput + totals.totalOutput + totals.totalCacheRead + totals.totalReasoning,
      cost: totals.totalEstimatedCost,
      completeTokenBreakdown: true,
    };
  }, [report, selected]);
  return (
    <div className="max-w-5xl space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h3 className="text-base font-semibold text-[var(--text-primary)]">模型用量</h3>
          <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">
            Session、模型调用、Token 与费用趋势。
          </p>
        </div>
        <div className="flex items-center gap-2">
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
          <select
            value={model}
            onChange={(event) => setModel(event.target.value)}
            disabled={!report || report.byModel.length === 0}
            className="input h-9 min-w-40 py-1 text-sm disabled:opacity-50"
            aria-label="模型筛选"
          >
            <option value="all">全部模型</option>
            {report?.byModel.map((row) => (
              <option key={row.model} value={row.model}>{row.model}</option>
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
          <Loader2 size={16} className="mr-2 animate-spin" /> 正在读取 Hermes 用量…
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

      {report && summary && (
        <>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {[
              { label: `近 ${days} 天会话`, value: summary.sessions, icon: MessageSquareText },
              { label: '模型调用', value: summary.calls, icon: Activity },
              { label: summary.completeTokenBreakdown ? 'Token 总量' : '输入 + 输出 Token', value: summary.total, icon: Coins },
              { label: '预估费用', value: summary.cost, icon: Banknote, currency: true },
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

          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            {[
              ['输入 Token', summary.input],
              ['输出 Token', summary.output],
              ['缓存读取', summary.completeTokenBreakdown ? summary.cache : null],
              ['推理 Token', summary.completeTokenBreakdown ? summary.reasoning : null],
            ].map(([label, value]) => (
              <div key={String(label)} className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-sunken)] px-3 py-2.5">
                <p className="text-[11px] text-[var(--text-tertiary)]">{label}</p>
                <p className="mt-1 text-sm font-semibold tabular-nums text-[var(--text-primary)]">
                  {value == null ? '—' : formatCount(Number(value))}
                </p>
              </div>
            ))}
          </div>

          {model === 'all' && report.daily.length > 0 && (
            <section className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
              <div className="mb-4 flex items-center justify-between gap-3">
                <div>
                  <h4 className="text-sm font-semibold text-[var(--text-primary)]">每日用量</h4>
                  <p className="mt-1 text-[11px] text-[var(--text-tertiary)]">全部模型 · 含缓存与推理 Token</p>
                </div>
                <span className="text-[11px] text-[var(--text-tertiary)]">柱高代表 Token</span>
              </div>
              <DailyUsageChart report={report} days={days} />
              <div className="mt-3 border-t border-[var(--border-default)] pt-3 text-[11px] text-[var(--text-tertiary)]">
                <span>悬停柱形可查看当日 Token、调用与费用</span>
              </div>
            </section>
          )}

          <section className="overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]">
            <div className="border-b border-[var(--border-default)] px-4 py-3">
              <h4 className="text-sm font-semibold text-[var(--text-primary)]">按模型统计</h4>
            </div>
            {report.byModel.length === 0 ? (
              <div className="px-4 py-12 text-center text-sm text-[var(--text-tertiary)]">当前时间范围内还没有模型用量</div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full min-w-[680px] text-left text-xs">
                  <thead className="bg-[var(--bg-sunken)] text-[var(--text-tertiary)]">
                    <tr>
                      <th className="px-4 py-2.5 font-medium">模型</th>
                      <th className="px-4 py-2.5 text-right font-medium">会话</th>
                      <th className="px-4 py-2.5 text-right font-medium">调用</th>
                      <th className="px-4 py-2.5 text-right font-medium">输入 Token</th>
                      <th className="px-4 py-2.5 text-right font-medium">输出 Token</th>
                      <th className="px-4 py-2.5 text-right font-medium">预估费用</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--border-default)]">
                    {report.byModel.map((row) => (
                      <tr key={row.model} className="hover:bg-[var(--bg-sunken)]">
                        <td className="px-4 py-3 font-mono font-medium text-[var(--text-primary)]">{row.model}</td>
                        <td className="px-4 py-3 text-right tabular-nums text-[var(--text-secondary)]">{formatCount(row.sessions)}</td>
                        <td className="px-4 py-3 text-right tabular-nums text-[var(--text-secondary)]">{formatCount(row.apiCalls)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatCount(row.inputTokens)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatCount(row.outputTokens)}</td>
                        <td className="px-4 py-3 text-right font-mono tabular-nums text-[var(--text-secondary)]">{formatEstimatedCostCny(row.estimatedCost)}</td>
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
