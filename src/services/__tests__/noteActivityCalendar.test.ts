import { describe, expect, it } from 'vitest';
import type { Article } from '../../types';
import {
  activityHeatLevel,
  articleActivityCounts,
  firstNotebookArticleIdForDate,
  monthDateCells,
  notebookDateKey,
} from '../noteActivityCalendar';

function article(overrides: Partial<Article>): Article {
  return {
    id: 'note-1',
    title: '笔记',
    content: '',
    articleType: 'manual',
    edited: false,
    createdAt: new Date(2026, 7, 14, 12).toISOString(),
    blocksJson: null,
    ...overrides,
  };
}

describe('note activity calendar', () => {
  it('uses a journal title as its notebook date', () => {
    expect(notebookDateKey(article({ articleType: 'journal', title: '2026-08-08' })))
      .toBe('2026-08-08');
  });

  it('selects only an existing note for a date and never fabricates one', () => {
    const articles = [article({ id: 'existing' })];
    expect(firstNotebookArticleIdForDate(articles, '2026-08-14')).toBe('existing');
    expect(firstNotebookArticleIdForDate(articles, '2026-08-15')).toBeNull();
    expect(articles).toHaveLength(1);
  });

  it('counts one activity per article and local day', () => {
    const counts = articleActivityCounts([
      article({ id: 'a', updatedAt: new Date(2026, 7, 14, 18).toISOString() }),
      article({ id: 'b', updatedAt: new Date(2026, 7, 15, 18).toISOString() }),
    ]);
    expect(counts.get('2026-08-14')).toBe(2);
    expect(counts.get('2026-08-15')).toBe(1);
  });

  it('builds a stable six-week month grid starting on Monday', () => {
    const cells = monthDateCells(2026, 7);
    expect(cells).toHaveLength(42);
    expect(cells.findIndex(Boolean)).toBe(5);
    expect(cells.filter(Boolean)).toHaveLength(31);
  });

  it('maps activity totals to stable heat levels', () => {
    expect([0, 1, 5, 10, 15].map((count) => activityHeatLevel(count, 20)))
      .toEqual([0, 1, 2, 3, 4]);
  });
});
