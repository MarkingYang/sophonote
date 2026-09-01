import type { HermesUsageDaily } from './tauri';

/** Hermes analytics 兼容层：同一份历史报告始终使用一致的金额换算。 */
export const USD_TO_CNY_RATE = 7.2;

export const usageDayTotalTokens = (row: HermesUsageDaily) => (
  row.inputTokens + row.outputTokens + row.cacheReadTokens + row.reasoningTokens
);

export const usdToCny = (usd: number) => (
  Number.isFinite(usd) && usd > 0 ? usd * USD_TO_CNY_RATE : 0
);

export const formatEstimatedCostCny = (usd: number) => {
  const cny = usdToCny(usd);
  if (cny <= 0) return '—';
  return new Intl.NumberFormat('zh-CN', {
    style: 'currency',
    currency: 'CNY',
    currencyDisplay: 'narrowSymbol',
    minimumFractionDigits: cny < 1 ? 4 : 2,
    maximumFractionDigits: cny < 1 ? 4 : 2,
  }).format(cny);
};

const emptyUsageDay = (day: string): HermesUsageDaily => ({
  day,
  inputTokens: 0,
  outputTokens: 0,
  cacheReadTokens: 0,
  reasoningTokens: 0,
  estimatedCost: 0,
  actualCost: 0,
  sessions: 0,
  apiCalls: 0,
});

const isoDay = (date: Date) => date.toISOString().slice(0, 10);

/** 按自然日补齐窗口；输入缺失的日期代表账本中没有用量。 */
export function fillDailyUsage(
  rows: HermesUsageDaily[],
  days: number,
  endDay = isoDay(new Date()),
): HermesUsageDaily[] {
  const byDay = new Map(rows.map((row) => [row.day, row]));
  const end = new Date(`${endDay}T00:00:00.000Z`);
  if (Number.isNaN(end.getTime()) || days <= 0) return [];

  return Array.from({ length: days }, (_, index) => {
    const date = new Date(end);
    date.setUTCDate(end.getUTCDate() - (days - index - 1));
    const day = isoDay(date);
    return byDay.get(day) ?? emptyUsageDay(day);
  });
}
