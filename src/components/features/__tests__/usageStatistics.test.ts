import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
vi.mock('../../../services/agentUsage', async (original) => ({ ...await original<typeof import('../../../services/agentUsage')>(), agentUsage: vi.fn() }));
import { agentUsage, emptyUsageNumbers, type AgentUsageReport } from '../../../services/agentUsage';
import UsageStatisticsPanel from '../UsageStatisticsPanel';
let host: HTMLDivElement; let root: Root;
const report: AgentUsageReport = { periodDays: 30, sources: [
  { engine: 'hermes', error: '未连接', summary: emptyUsageNumbers(), daily: [], byModel: [] },
  { engine: 'pi', error: null, summary: { ...emptyUsageNumbers(), totalTokens: 125, apiCalls: 1, sessions: 1 }, daily: [], byModel: [{ ...emptyUsageNumbers(), model: 'pi-model', totalTokens: 125, apiCalls: 1, daily: [] }] },
  { engine: 'claude_code', error: null, summary: { ...emptyUsageNumbers(), totalTokens: 200, apiCalls: 1, sessions: 1 }, daily: [], byModel: [{ ...emptyUsageNumbers(), model: 'claude-model', totalTokens: 200, apiCalls: 1, daily: [] }] },
] };
beforeEach(() => {
  vi.clearAllMocks(); Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  vi.mocked(agentUsage).mockResolvedValue(report);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); });
async function change(label: string, value: string) {
  const select = host.querySelector(`select[aria-label="${label}"]`) as HTMLSelectElement;
  await act(async () => { select.value = value; select.dispatchEvent(new Event('change', { bubbles: true })); });
}
it('默认汇总、错误隔离、Sidecar 与模型联动以及时间范围刷新', async () => {
  await act(async () => root.render(createElement(UsageStatisticsPanel)));
  expect(host.textContent).toContain('325'); expect(host.textContent).toContain('Hermes：未连接');
  await change('Sidecar 筛选', 'pi');
  expect(host.querySelector('table')?.textContent).toContain('pi-model');
  expect(host.querySelector('table')?.textContent).not.toContain('claude-model');
  expect(host.querySelector('[role="status"]')).toBeNull();
  await change('模型筛选', 'pi-model'); await change('Sidecar 筛选', 'claude_code');
  expect((host.querySelector('[aria-label="模型筛选"]') as HTMLSelectElement).value).toBe('all');
  await change('统计时间范围', '7'); expect(agentUsage).toHaveBeenLastCalledWith(7);
});
it('快速切换窗口时旧请求不会覆盖新结果', async () => {
  let resolveOld!: (r: AgentUsageReport) => void;
  vi.mocked(agentUsage).mockImplementationOnce(() => new Promise((resolve) => { resolveOld = resolve; })).mockResolvedValue({ ...report, sources: [] });
  await act(async () => root.render(createElement(UsageStatisticsPanel)));
  await change('统计时间范围', '7');
  await act(async () => resolveOld(report));
  expect(host.textContent).not.toContain('pi-model');
  expect(host.textContent).toContain('当前时间范围内还没有模型用量');
});
