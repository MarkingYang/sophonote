import { invoke } from '@tauri-apps/api/core';
interface ApiResponse<T> { success: boolean; data: T | null; error: string | null }

export type UsageEngine = 'hermes' | 'pi' | 'claude_code' | 'opencode';
export const usageEngineLabels: Record<UsageEngine, string> = { hermes: 'Hermes', pi: 'Pi', claude_code: 'Claude Code', opencode: 'OpenCode' };
export interface UsageNumbers {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number | null;
  cacheWriteTokens: number | null;
  reasoningTokens: number | null;
  totalTokens: number;
  sessions: number;
  apiCalls: number;
  estimatedCost: number | null;
  partialCost: boolean;
}
export interface UsageDay extends UsageNumbers { day: string }
export interface UsageModel extends UsageNumbers { model: string; daily: UsageDay[] | null }
export interface UsageSource { engine: UsageEngine; summary: UsageNumbers; daily: UsageDay[]; byModel: UsageModel[]; error: string | null }
export interface AgentUsageReport { sources: UsageSource[]; periodDays: number }
export async function agentUsage(days: number): Promise<AgentUsageReport> {
  const response = await invoke<ApiResponse<AgentUsageReport>>('agent_usage', { days });
  if (!response.success || !response.data) throw new Error(response.error || '读取 Sidecar 用量失败');
  return response.data;
}
export const emptyUsageNumbers = (): UsageNumbers => ({
  inputTokens: 0, outputTokens: 0, cacheReadTokens: 0, cacheWriteTokens: 0, reasoningTokens: 0,
  totalTokens: 0, sessions: 0, apiCalls: 0, estimatedCost: null, partialCost: false,
});
export function sumUsage(rows: UsageNumbers[]): UsageNumbers {
  const sum = emptyUsageNumbers();
  for (const row of rows) {
    for (const key of ['inputTokens', 'outputTokens', 'totalTokens', 'sessions', 'apiCalls'] as const) sum[key] += row[key];
    for (const key of ['cacheReadTokens', 'cacheWriteTokens', 'reasoningTokens'] as const) {
      if (row.apiCalls > 0 || row.totalTokens > 0) sum[key] = sum[key] != null && row[key] != null ? sum[key] + row[key] : null;
    }
    if (row.estimatedCost != null) sum.estimatedCost = (sum.estimatedCost ?? 0) + row.estimatedCost;
    sum.partialCost ||= row.partialCost || (row.apiCalls > 0 && row.estimatedCost == null);
  }
  return sum;
}
export function selectUsage(report: AgentUsageReport, engine: UsageEngine | 'all', model: string) {
  const sources = report.sources.filter((s) => engine === 'all' || s.engine === engine);
  const models = sources.flatMap((s) => s.byModel.map((m) => ({ ...m, engine: s.engine }))).filter((m) => model === 'all' || m.model === model);
  const summary = sumUsage(model === 'all' ? sources.map((s) => s.summary) : models);
  const dailyRows = model === 'all' ? sources.map((s) => s.daily) : models.map((m) => m.daily);
  const dailyComplete = dailyRows.every((r) => r != null);
  const dates = new Map<string, UsageNumbers[]>();
  for (const row of dailyRows.flatMap((r) => r ?? [])) dates.set(row.day, [...(dates.get(row.day) ?? []), row]);
  const daily = dailyComplete ? [...dates.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([day, rows]) => ({ day, ...sumUsage(rows) })) : [];
  return { summary, models, daily, dailyComplete, sources };
}
/** Rolling windows can include the boundary date as well as today. Preserve it. */
export function fillAgentUsageDays(rows: UsageDay[], days: number, endDay = new Date().toISOString().slice(0, 10)): UsageDay[] {
  const end = new Date(`${endDay}T00:00:00Z`);
  const start = new Date(end); start.setUTCDate(start.getUTCDate() - days + 1);
  const earliest = rows.map((r) => r.day).sort()[0];
  if (earliest && earliest < start.toISOString().slice(0, 10)) start.setUTCDate(start.getUTCDate() - 1);
  const byDay = new Map(rows.map((r) => [r.day, r]));
  const result: UsageDay[] = [];
  for (const d = new Date(start); d <= end; d.setUTCDate(d.getUTCDate() + 1)) {
    const day = d.toISOString().slice(0, 10); result.push(byDay.get(day) ?? { day, ...emptyUsageNumbers() });
  }
  return result;
}
