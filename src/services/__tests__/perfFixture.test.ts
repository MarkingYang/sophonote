import { describe, expect, it } from 'vitest';
import {
  FIXTURE_50K_ID,
  FIXTURE_5K_ID,
  FIXTURE_DOC_COUNT,
  FIXTURE_ID_PREFIX,
  FIXTURE_TITLE_PREFIX,
  SIZE_50K_BYTES,
  SIZE_5K_BYTES,
  fixtureContent,
  fixtureDocSpecs,
  isFixtureArticle,
  percentile,
  planFixtureSeed,
  summarizeSamples,
  utf8ByteLength,
} from '../perfFixture';
import type { Article } from '../../types';

const article = (over: Partial<Article> & Pick<Article, 'id' | 'title' | 'content'>): Article => ({
  articleType: 'manual',
  edited: false,
  createdAt: '2026-08-16T00:00:00.000Z',
  ...over,
});

describe('NEXT-001 夹具清单', () => {
  it('固定 200 篇：5KB/50KB 基准 + 列表样本，ID/标题前缀稳定', () => {
    const specs = fixtureDocSpecs();
    expect(specs).toHaveLength(FIXTURE_DOC_COUNT);
    const ids = new Set(specs.map((s) => s.id));
    expect(ids.size).toBe(FIXTURE_DOC_COUNT);
    for (const s of specs) {
      expect(s.id.startsWith(FIXTURE_ID_PREFIX)).toBe(true);
      expect(s.title.startsWith(FIXTURE_TITLE_PREFIX)).toBe(true);
    }
    expect(specs[0].id).toBe(FIXTURE_5K_ID);
    expect(specs[0].kind).toBe('size5k');
    expect(specs[1].id).toBe(FIXTURE_50K_ID);
    expect(specs[1].kind).toBe('size50k');
    expect(specs.slice(2).every((s) => s.kind === 'list')).toBe(true);
  });
});

describe('NEXT-001 确定性内容', () => {
  it('同 (kind,index) 逐字节一致（改前/改后对比的前提）', () => {
    expect(fixtureContent('size50k', 1)).toBe(fixtureContent('size50k', 1));
    expect(fixtureContent('list', 42)).toBe(fixtureContent('list', 42));
    expect(fixtureContent('list', 42)).not.toBe(fixtureContent('list', 43));
  });

  it('体量达标且不超界：≥目标值，单块上界内', () => {
    const b5k = utf8ByteLength(fixtureContent('size5k', 0));
    const b50k = utf8ByteLength(fixtureContent('size50k', 1));
    const blist = utf8ByteLength(fixtureContent('list', 7));
    expect(b5k).toBeGreaterThanOrEqual(SIZE_5K_BYTES);
    expect(b5k).toBeLessThan(SIZE_5K_BYTES + 1536);
    expect(b50k).toBeGreaterThanOrEqual(SIZE_50K_BYTES);
    expect(b50k).toBeLessThan(SIZE_50K_BYTES + 1536);
    expect(blist).toBeGreaterThanOrEqual(1200);
    expect(blist).toBeLessThan(1200 + 1536);
  });

  it('内容含解析链路要喂的结构：标题/列表/链接', () => {
    const md = fixtureContent('size50k', 1);
    expect(md).toContain('## ');
    expect(md).toContain('- ');
    expect(md).toMatch(/\[\[.+\]\]/);
    expect(md).toMatch(/\[[^\]]+\]\(https:\/\/tauri\.app\)/);
  });
});

describe('NEXT-001 播种规划（幂等）', () => {
  it('空库 → 全量 create', () => {
    const plan = planFixtureSeed([]);
    expect(plan.create).toHaveLength(FIXTURE_DOC_COUNT);
    expect(plan.update).toHaveLength(0);
    expect(plan.unchanged).toBe(0);
  });

  it('内容一致 → unchanged；内容漂移 → update；缺失 → create', () => {
    const specs = fixtureDocSpecs();
    const s0 = specs[0];
    const s1 = specs[1];
    const existing: Article[] = [
      article({ id: s0.id, title: s0.title, content: fixtureContent(s0.kind, s0.index) }),
      article({ id: s1.id, title: s1.title, content: '被用户改坏的正文' }),
    ];
    const plan = planFixtureSeed(existing);
    expect(plan.unchanged).toBe(1);
    expect(plan.update.map((s) => s.id)).toEqual([s1.id]);
    expect(plan.create).toHaveLength(FIXTURE_DOC_COUNT - 2);
  });

  it('SQLite 正文为空但标题一致 → unchanged（文件是真相源）', () => {
    const s0 = fixtureDocSpecs()[0];
    const plan = planFixtureSeed([article({ id: s0.id, title: s0.title, content: '' })]);
    expect(plan.unchanged).toBe(1);
    expect(plan.update).toHaveLength(0);
  });

  it('标题漂移也触发 update（标题是列表扫描成本的一部分）', () => {
    const s0 = fixtureDocSpecs()[0];
    const existing: Article[] = [
      article({ id: s0.id, title: '被改标题', content: fixtureContent(s0.kind, s0.index) }),
    ];
    const plan = planFixtureSeed(existing);
    expect(plan.update.map((s) => s.id)).toEqual([s0.id]);
  });

  it('isFixtureArticle 只认前缀', () => {
    expect(isFixtureArticle({ id: `${FIXTURE_ID_PREFIX}007` })).toBe(true);
    expect(isFixtureArticle({ id: 'a-normal-note' })).toBe(false);
  });
});

describe('NEXT-001 统计口径', () => {
  it('percentile 最近秩法（ceil rank）', () => {
    const values = Array.from({ length: 20 }, (_, i) => i + 1); // 1..20
    expect(percentile(values, 50)).toBe(10); // ceil(20*0.5)=10 → 第 10 个
    expect(percentile(values, 95)).toBe(19); // ceil(20*0.95)=19 → 第 19 个
    expect(percentile([7], 95)).toBe(7);
    expect(percentile([], 95)).toBeNull();
  });

  it('percentile 不受乱序输入影响', () => {
    expect(percentile([9, 1, 5, 3, 7], 50)).toBe(percentile([1, 3, 5, 7, 9], 50));
  });

  it('summarizeSamples 输出 P50/P95/mean/max', () => {
    const stats = summarizeSamples([10, 20, 30, 40]);
    expect(stats).not.toBeNull();
    expect(stats!.n).toBe(4);
    expect(stats!.min).toBe(10);
    expect(stats!.max).toBe(40);
    expect(stats!.mean).toBe(25);
    expect(stats!.p50).toBe(20); // ceil(4*0.5)=2 → 第 2 个
    expect(stats!.p95).toBe(40); // ceil(4*0.95)=4 → 第 4 个
    expect(summarizeSamples([])).toBeNull();
  });
});
