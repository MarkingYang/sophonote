/**
 * NEXT-001 统一性能夹具：确定性语料 + 统计口径（PRD §3.1 / §11.4 / §16 指标）。
 *
 * 设计约束：
 * - 语料必须「改前/改后同一份」：内容由固定种子 LCG 生成，跨运行/跨设备逐字节一致；
 * - 走现网写入路径（insertArticle/updateArticle → 先 .md 真相源后 DB 索引），
 *   不引入旁路写库；
 * - 全部文档带稳定前缀（perf-fix- / [Perf] ），一键整批清理；
 * - 本文件的可测纯函数（specs/content/plan/percentile）不触碰 Tauri，
 *   沙箱 vitest 直接覆盖；IO 编排（seed/clear）是纯规划函数之外的薄壳。
 */

import type { Article } from '../types';
import { deleteArticles, insertArticle, renameArticle, updateArticle } from './tauri';

// ==================== 常量 ====================

export const FIXTURE_ID_PREFIX = 'perf-fix-';
export const FIXTURE_TITLE_PREFIX = '[Perf] ';
export const FIXTURE_DOC_COUNT = 200;
export const FIXTURE_5K_ID = `${FIXTURE_ID_PREFIX}5k`;
export const FIXTURE_50K_ID = `${FIXTURE_ID_PREFIX}50k`;
/** PRD §3.1 / §11.4 固定分桶：5 KB、50 KB、200 篇库 */
export const SIZE_5K_BYTES = 5 * 1024;
export const SIZE_50K_BYTES = 50 * 1024;
export const SIZE_LIST_BYTES = 1200;

export type FixtureKind = 'size5k' | 'size50k' | 'list';

export interface FixtureDocSpec {
  id: string;
  title: string;
  kind: FixtureKind;
  index: number;
}

// ==================== 确定性内容生成 ====================

/** 固定种子 LCG（Numerical Recipes 参数）：同 (kind,index) 永远同内容 */
function lcg(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 4294967296;
  };
}

const SENTENCES = [
  '写作性能的目标是让输入与切换不打断思路，而不是追求跑分上的极限数字。',
  'ProseMirror 的 transaction 模型要求每次派发都尽快返回，避免主线程长任务。',
  'Serialization cost grows with document size, so debounce and max-wait matter.',
  '分屏预览按 Markdown hash 缓存，重复渲染只应在正文真正变化后发生。',
  '页签切换的体感由首帧与可交互两个时点共同决定，二者都需要采样。',
  'Long Task 超过 50ms 就会阻塞输入响应，是 P0 门禁的硬指标之一。',
  '保存队列按 documentId 隔离后，快速 A/B 切换不再互相等待 flush。',
  'The fixture corpus must stay byte-stable across runs to keep comparisons honest.',
  '提及扫描在空闲时执行，且跳过仅当前文档保存的场景，避免抢占写作主链路。',
  '列表插件只在列表结构变化时扫描，经 transaction mapping 映射回安全范围。',
];

const HEADINGS = ['背景与目标', '设计与取舍', '测量口径', '回归与风险', '参考资料', '附录'];

const KEYWORDS = ['性能', '夹具', '切换', '序列化', '预览', '队列', '基线', '采样'];

function makeParagraph(rnd: () => number, docIndex: number): string {
  const count = 3 + Math.floor(rnd() * 3); // 3~5 句
  const parts: string[] = [];
  for (let i = 0; i < count; i++) {
    parts.push(SENTENCES[Math.floor(rnd() * SENTENCES.length)]);
  }
  // 偶发双链/外链/关键词，喂给提及扫描与解析链路（贴近真实笔记结构）
  if (rnd() < 0.5) {
    const linkTarget = Math.floor(rnd() * FIXTURE_DOC_COUNT);
    parts.push(`参见 [[${FIXTURE_TITLE_PREFIX.trim()} 列表样本 ${String(linkTarget).padStart(3, '0')}]] 的 ${KEYWORDS[Math.floor(rnd() * KEYWORDS.length)]} 部分。`);
  }
  if (rnd() < 0.3) {
    parts.push(`更多定义见 [Tauri 文档](https://tauri.app) 与文档 ${docIndex} 的注记。`);
  }
  return parts.join('');
}

function makeListBlock(rnd: () => number): string {
  const n = 3 + Math.floor(rnd() * 3);
  const lines: string[] = [];
  for (let i = 0; i < n; i++) {
    const task = rnd() < 0.3 ? '- [ ] ' : '- ';
    lines.push(`${task}${KEYWORDS[Math.floor(rnd() * KEYWORDS.length)]}样本项 ${i + 1}（种子值 ${Math.floor(rnd() * 10000)}）`);
  }
  return lines.join('\n');
}

function makeCodeFence(rnd: () => number): string {
  return ['```ts', `const seed = ${Math.floor(rnd() * 100000)};`, `const p95 = samples.sort((a, b) => a - b)[Math.floor(samples.length * 0.95)];`, '```'].join('\n');
}

export function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/**
 * 生成目标体量的确定性 Markdown。
 * 体量语义：结果 ≥ target 且超出部分不超过最后一个块（测试给出上界）。
 */
export function fixtureContent(kind: FixtureKind, index: number): string {
  const target =
    kind === 'size5k' ? SIZE_5K_BYTES : kind === 'size50k' ? SIZE_50K_BYTES : SIZE_LIST_BYTES;
  const seedBase = index * 7919 + (kind === 'size5k' ? 11 : kind === 'size50k' ? 13 : 17);
  const rnd = lcg(seedBase);

  const blocks: string[] = [makeParagraph(rnd, index)];
  let section = 0;
  const joined = () => blocks.join('\n\n');
  while (utf8ByteLength(joined()) < target) {
    const heading = `## ${HEADINGS[section % HEADINGS.length]} · ${section + 1}`;
    const body: string[] = [makeParagraph(rnd, index)];
    if (rnd() < 0.6) body.push(makeListBlock(rnd));
    if (kind === 'size50k' && section % 7 === 3) body.push(makeCodeFence(rnd));
    blocks.push(`${heading}\n\n${body.join('\n\n')}`);
    section++;
  }
  return joined();
}

// ==================== 语料清单与播种规划（纯函数） ====================

/** 200 篇固定清单：0 号 = 5KB 基准长文，1 号 = 50KB 基准长文，其余为列表样本 */
export function fixtureDocSpecs(): FixtureDocSpec[] {
  const specs: FixtureDocSpec[] = [];
  for (let i = 0; i < FIXTURE_DOC_COUNT; i++) {
    const id = i === 0 ? FIXTURE_5K_ID : i === 1 ? FIXTURE_50K_ID : `${FIXTURE_ID_PREFIX}${String(i).padStart(3, '0')}`;
    const title =
      i === 0
        ? `${FIXTURE_TITLE_PREFIX}5KB 基准长文`
        : i === 1
          ? `${FIXTURE_TITLE_PREFIX}50KB 基准长文`
          : `${FIXTURE_TITLE_PREFIX}列表样本 ${String(i).padStart(3, '0')}`;
    specs.push({ id, title, kind: i === 0 ? 'size5k' : i === 1 ? 'size50k' : 'list', index: i });
  }
  return specs;
}

export interface FixtureSeedPlan {
  create: FixtureDocSpec[];
  update: FixtureDocSpec[];
  unchanged: number;
}

/** 幂等播种规划：缺则建、漂移则更新（内容或标题）、一致则跳过 */
export function planFixtureSeed(existing: Article[]): FixtureSeedPlan {
  const byId = new Map(existing.map((a) => [a.id, a]));
  const plan: FixtureSeedPlan = { create: [], update: [], unchanged: 0 };
  for (const spec of fixtureDocSpecs()) {
    const cur = byId.get(spec.id);
    if (!cur) {
      plan.create.push(spec);
      continue;
    }
    const content = fixtureContent(spec.kind, spec.index);
    // SQLite `articles.content` 可按文件真相源策略为空；空正文不能当成漂移，
    // 否则每次门禁都会串行重写 200 篇，拖死 NEXT-001。
    const body = cur.content.trim();
    const drifted = cur.title !== spec.title || (body.length > 0 && body !== content.trim());
    if (drifted) {
      plan.update.push(spec);
    } else {
      plan.unchanged++;
    }
  }
  return plan;
}

export function isFixtureArticle(article: Pick<Article, 'id'>): boolean {
  return article.id.startsWith(FIXTURE_ID_PREFIX);
}

// ==================== IO 编排（薄壳，宿主执行） ====================

export interface FixtureSeedResult {
  created: number;
  updated: number;
  unchanged: number;
}

/** 播种 200 篇固定语料（串行写入，避免打爆索引防抖与保存队列） */
export async function seedFixtureCorpus(
  existing: Article[],
  now: () => Date = () => new Date(),
): Promise<FixtureSeedResult> {
  const plan = planFixtureSeed(existing);
  for (const spec of plan.create) {
    await insertArticle({
      id: spec.id,
      title: spec.title,
      content: fixtureContent(spec.kind, spec.index),
      articleType: 'manual',
      edited: false,
      createdAt: now().toISOString(),
    });
  }
  for (const spec of plan.update) {
    await updateArticle(spec.id, fixtureContent(spec.kind, spec.index));
    const cur = existing.find((a) => a.id === spec.id);
    if (cur && cur.title !== spec.title) {
      await renameArticle(spec.id, spec.title);
    }
  }
  return { created: plan.create.length, updated: plan.update.length, unchanged: plan.unchanged };
}

/** 一键清理全部夹具文档（单事务批量删，现网 db_delete_articles 路径） */
export async function clearFixtureCorpus(existing: Article[]): Promise<number> {
  const ids = existing.filter(isFixtureArticle).map((a) => a.id);
  if (ids.length === 0) return 0;
  await deleteArticles(ids);
  return ids.length;
}

// ==================== 统计口径（P50/P95 写入台账用） ====================

export interface SampleStats {
  n: number;
  min: number;
  max: number;
  mean: number;
  p50: number;
  p95: number;
}

/** 最近秩法（ceil rank）：小样本下保守、可复算，台账口径固定用它 */
export function percentile(values: number[], p: number): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.max(0, Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[rank];
}

export function summarizeSamples(values: number[]): SampleStats | null {
  if (values.length === 0) return null;
  const p50 = percentile(values, 50);
  const p95 = percentile(values, 95);
  if (p50 === null || p95 === null) return null;
  const sum = values.reduce((acc, v) => acc + v, 0);
  return {
    n: values.length,
    min: Math.min(...values),
    max: Math.max(...values),
    mean: Math.round((sum / values.length) * 10) / 10,
    p50,
    p95,
  };
}
