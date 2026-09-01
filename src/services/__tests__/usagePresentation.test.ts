import { describe, expect, it } from 'vitest';
import {
  fillDailyUsage,
  formatEstimatedCostCny,
  usageDayTotalTokens,
  usdToCny,
} from '../usagePresentation';

describe('usage presentation', () => {
  it('converts Runtime USD estimates to the fixed CNY display rate', () => {
    expect(usdToCny(1)).toBe(7.2);
    expect(formatEstimatedCostCny(0.01)).toBe('¥0.0720');
    expect(formatEstimatedCostCny(0)).toBe('—');
  });

  it('fills missing natural days without changing Runtime rows', () => {
    const rows = fillDailyUsage([{
      day: '2026-08-18',
      inputTokens: 120,
      outputTokens: 30,
      cacheReadTokens: 10,
      reasoningTokens: 5,
      estimatedCost: 0.01,
      actualCost: 0,
      sessions: 2,
      apiCalls: 4,
    }], 3, '2026-08-19');

    expect(rows.map((row) => row.day)).toEqual(['2026-08-17', '2026-08-18', '2026-08-19']);
    expect(rows[0].inputTokens).toBe(0);
    expect(usageDayTotalTokens(rows[1])).toBe(165);
  });
});
