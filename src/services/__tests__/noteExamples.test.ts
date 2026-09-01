import { describe, expect, it } from 'vitest';
import { missingNoteExamples, noteExampleArticles, noteExamples, stripExampleFrontmatter } from '../noteExamples';

describe('note examples', () => {
  it('keeps bundled examples read-only until an explicit caller creates drafts', () => {
    expect(noteExamples).toHaveLength(6);
    expect(new Set(noteExamples.map((example) => example.title)).size).toBe(6);
    expect(noteExamples.every((example) => example.content.length > 20)).toBe(true);
    expect(noteExamples.every((example) => !example.content.startsWith('---'))).toBe(true);
  });

  it('only creates missing notes and never replaces the same title', () => {
    const existing = ['样例·任务清单', '用户自己的笔记'];
    const ids = ['one', 'two', 'three', 'four', 'five'];
    const drafts = noteExampleArticles(existing, new Date('2026-09-01T00:00:00.000Z'), () => ids.shift()!);

    expect(drafts).toHaveLength(5);
    expect(drafts.some((article) => article.title === '样例·任务清单')).toBe(false);
    expect(drafts.every((article) => article.articleType === 'manual')).toBe(true);
    expect(missingNoteExamples(noteExamples.map((example) => example.title))).toEqual([]);
  });

  it('strips only the leading YAML frontmatter', () => {
    expect(stripExampleFrontmatter('---\ntitle: Example\n---\n# Body\n---')).toBe('# Body\n---');
  });
});
