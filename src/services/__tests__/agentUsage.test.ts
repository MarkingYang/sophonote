import { describe, it, expect, vi, beforeEach } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { agentUsage, emptyUsageNumbers, fillAgentUsageDays, selectUsage, sumUsage, type AgentUsageReport, type UsageSource } from '../agentUsage';
const source = (engine: UsageSource['engine'], total: number): UsageSource => ({ engine, error: null,
  summary: { ...emptyUsageNumbers(), sessions: 1, apiCalls: 2, totalTokens: total, inputTokens: total - 10, outputTokens: 10 },
  daily: [{ ...emptyUsageNumbers(), day: '2026-09-22', totalTokens: total, apiCalls: 2 }],
  byModel: [{ ...emptyUsageNumbers(), model: 'shared-model', sessions: 1, apiCalls: 1, totalTokens: total, daily: [] }],
});
beforeEach(() => vi.clearAllMocks());
describe('四 Sidecar 用量', () => {
  it('invoke 契约与失败传播', async () => {
    const report: AgentUsageReport = { sources: [], periodDays: 30 };
    vi.mocked(invoke).mockResolvedValueOnce({ success: true, data: report });
    expect(await agentUsage(30)).toEqual(report);
    expect(invoke).toHaveBeenCalledWith('agent_usage', { days: 30 });
    vi.mocked(invoke).mockResolvedValueOnce({ success: false, error: '窗口不支持' });
    await expect(agentUsage(8)).rejects.toThrow('窗口不支持');
  });
  it('同名模型跨引擎相加，Sidecar 筛选隔离，模型切换不重复计算会话', () => {
    const pi = source('pi', 100); pi.byModel.push({ ...pi.byModel[0], model: 'other', totalTokens: 0 });
    const report = { sources: [source('hermes', 200), pi, source('claude_code', 50), source('opencode', 75)], periodDays: 30 };
    expect(selectUsage(report, 'all', 'all').summary.totalTokens).toBe(425);
    expect(selectUsage(report, 'all', 'all').summary.sessions).toBe(4);
    expect(selectUsage(report, 'pi', 'all').summary.sessions).toBe(1);
    expect(selectUsage(report, 'pi', 'shared-model').models).toHaveLength(1);
    expect(selectUsage(report, 'all', 'shared-model').models).toHaveLength(4);
    expect(selectUsage(report, 'all', 'all').daily[0].totalTokens).toBe(425);
  });
  it('保留缺失细分及部分费用，来源失败不吞掉成功来源', () => {
    const pi = source('pi', 10); pi.summary.estimatedCost = 0.1;
    const claude = source('claude_code', 20); claude.summary.reasoningTokens = null;
    const hermes = source('hermes', 0); hermes.error = '未连接'; hermes.summary = emptyUsageNumbers();
    const report = { sources: [hermes, pi, claude], periodDays: 7 };
    const selected = selectUsage(report, 'all', 'all');
    expect(selected.summary.totalTokens).toBe(30); expect(selected.summary.estimatedCost).toBe(0.1);
    expect(selected.summary.partialCost).toBe(true); expect(selected.summary.reasoningTokens).toBeNull();
    expect(sumUsage([]).estimatedCost).toBeNull();
  });
  it('缺少逐模型日期则隐藏组合趋势，不显示部分趋势为全量', () => {
    const hermes = source('hermes', 100); hermes.byModel[0].daily = null;
    const report = { sources: [hermes, source('pi', 20)], periodDays: 30 };
    expect(selectUsage(report, 'all', 'shared-model').dailyComplete).toBe(false);
    expect(selectUsage(report, 'all', 'shared-model').daily).toEqual([]);
    expect(selectUsage(report, 'pi', 'shared-model').dailyComplete).toBe(true);
  });
  it('补齐自然日并保留滚动窗口边界日期', () => {
    const rows = [{ ...emptyUsageNumbers(), day: '2026-09-15', totalTokens: 100 }];
    const daily = fillAgentUsageDays(rows, 7, '2026-09-22');
    expect(daily).toHaveLength(8); expect(daily[0].totalTokens).toBe(100);
    expect(daily.at(-1)?.day).toBe('2026-09-22');
    expect(fillAgentUsageDays([], 7, '2026-09-22')).toHaveLength(7);
  });
});
