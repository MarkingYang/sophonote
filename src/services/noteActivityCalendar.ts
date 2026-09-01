import type { Article } from '../types';
import { isJournalTitle } from './journal';

export function localDateKey(value: string): string | null {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleDateString('sv-SE');
}

/** journal 归属标题中的自然日；普通笔记归属创建日。 */
export function notebookDateKey(article: Article): string | null {
  if (article.articleType === 'journal' && isJournalTitle(article.title)) return article.title;
  return localDateKey(article.createdAt);
}

/** 只从既有列表选择目标日期的首篇笔记；没有命中时保持空态，不创建文档。 */
export function firstNotebookArticleIdForDate(
  articles: Article[],
  dateKey: string,
): string | null {
  return articles.find((article) => notebookDateKey(article) === dateKey)?.id ?? null;
}

/**
 * 月历热度事实：创建日计一次；若后续编辑发生在另一自然日，该日再计一次。
 * 单篇文档同一天只计一次，避免创建后立即保存造成虚高。
 */
export function articleActivityCounts(articles: Article[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const article of articles) {
    const dates = new Set<string>();
    const created = localDateKey(article.createdAt);
    if (created) dates.add(created);
    if (article.updatedAt) {
      const updated = localDateKey(article.updatedAt);
      if (updated) dates.add(updated);
    }
    for (const date of dates) counts.set(date, (counts.get(date) ?? 0) + 1);
  }
  return counts;
}

export function activityHeatLevel(count: number, monthMaximum = count): 0 | 1 | 2 | 3 | 4 {
  if (count <= 0) return 0;
  const ratio = count / Math.max(monthMaximum, 1);
  if (ratio <= 0.2) return 1;
  if (ratio <= 0.45) return 2;
  if (ratio <= 0.7) return 3;
  return 4;
}

export function monthDateCells(year: number, month: number): Array<Date | null> {
  const first = new Date(year, month, 1);
  const leadingMondayCells = (first.getDay() + 6) % 7;
  const days = new Date(year, month + 1, 0).getDate();
  const cells: Array<Date | null> = Array.from({ length: leadingMondayCells }, () => null);
  for (let day = 1; day <= days; day += 1) cells.push(new Date(year, month, day));
  while (cells.length < 42) cells.push(null);
  return cells;
}
