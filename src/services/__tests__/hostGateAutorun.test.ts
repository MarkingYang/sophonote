import { describe, expect, it } from 'vitest';
import { feedRowToItem, ISSUE044_NEEDLES, pickFeedRowsByNeedles } from '../hostGateAutorun';
import type { DiscoveryFeedRow } from '../tauri';

const row = (over: Partial<DiscoveryFeedRow> & Pick<DiscoveryFeedRow, 'id' | 'title'>): DiscoveryFeedRow => ({
  sourceId: 'aihot',
  sourceName: 'AIHot',
  type: 'article',
  url: 'https://example.com',
  aiScore: 8,
  aiScoredAt: '2026-08-27T00:00:00Z',
  aiTopics: [],
  status: 'unread',
  ...over,
});

describe('ISSUE-044 发现打开路径投影', () => {
  it('按标题关键词命中 feed 行，缺失保持 null', () => {
    const rows = [
      row({ id: 'a', title: 'Google WeatherNext 2 发布' }),
      row({ id: 'b', title: 'Apple introduces M6 and M5 Ultra' }),
    ];
    const picks = pickFeedRowsByNeedles(rows, ISSUE044_NEEDLES);
    expect(picks.map((p) => [p.needle, p.row?.id ?? null])).toEqual([
      ['WeatherNext', 'a'],
      ['introduces M6', 'b'],
      ['FID', null],
    ]);
  });

  it('feed 行注入 Item 后详情能按 id 找到（与 Discover 点击同字段）', () => {
    const item = feedRowToItem(
      row({
        id: 'hn-1',
        title: 'FID 论文',
        description: '摘要',
        author: 'Ann',
        aiTags: 'a, b',
        contentStatus: 'ready',
        qualityLevel: 3,
      }),
    );
    expect(item.id).toBe('hn-1');
    expect(item.title).toBe('FID 论文');
    expect(item.sourceId).toBe('aihot');
    expect(item.aiTags).toEqual(['a', 'b']);
    expect(item.qualityLevel).toBe(3);
    expect(item.contentStatus).toBe('ready');
  });
});
