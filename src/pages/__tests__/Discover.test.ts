import { describe, expect, it } from 'vitest';
import { recentDiscoveryFallbackQuery } from '../Discover';

describe('发现精选空态降级', () => {
  it('只读取已解读的近期积累，不降低或冒充精选门槛', () => {
    expect(recentDiscoveryFallbackQuery(null)).toEqual({
      minScore: 7,
      requireDeep: true,
      aspect: null,
      limit: 6,
    });
    expect(recentDiscoveryFallbackQuery('论文').aspect).toBe('论文');
    expect(recentDiscoveryFallbackQuery(null).windowDays).toBeUndefined();
  });
});
