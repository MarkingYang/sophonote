/**
 * NB-08 搜索命中上下文（对标 Obsidian 搜索面板：命中片段 + 高亮 + 命中数）。
 *
 * 纯函数集，供 ⌘K 快速切换器与笔记本列表搜索共用；
 * 全部大小写不敏感（与 Latin 标题/正文的实际使用一致），命中片段保留原文形态。
 */

export interface SnippetSeg {
  text: string;
  hit: boolean;
}

/** 统计 query 在 md 中的命中次数（大小写不敏感；空查询返回 0） */
export function countMatches(md: string, query: string): number {
  const q = query.trim().toLowerCase();
  if (!q) return 0;
  const lower = md.toLowerCase();
  let count = 0;
  let pos = lower.indexOf(q);
  while (pos !== -1) {
    count++;
    pos = lower.indexOf(q, pos + q.length);
  }
  return count;
}

/**
 * 把 text 按 query 的所有命中切成段（hit=true 的段为命中原文）。
 * 大小写不敏感匹配、保留原文形态；无命中或空查询返回整段 hit=false。
 */
export function highlightSegments(text: string, query: string): SnippetSeg[] {
  const q = query.trim().toLowerCase();
  if (!q) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  const segs: SnippetSeg[] = [];
  let cursor = 0;
  let pos = lower.indexOf(q);
  while (pos !== -1) {
    if (pos > cursor) segs.push({ text: text.slice(cursor, pos), hit: false });
    segs.push({ text: text.slice(pos, pos + q.length), hit: true });
    cursor = pos + q.length;
    pos = lower.indexOf(q, cursor);
  }
  if (cursor < text.length) segs.push({ text: text.slice(cursor), hit: false });
  if (segs.length === 0) segs.push({ text, hit: false });
  return segs;
}

export interface MatchContext {
  /** 首处命中前的上下文（换行压平、超长截断时前置 …） */
  before: string;
  /** 命中原文（保留原大小写） */
  match: string;
  /** 命中后的上下文（换行压平、超长截断时后置 …） */
  after: string;
  /** 命中所在行（1-based） */
  line: number;
}

/** 首处命中的上下文片段；无命中或空查询返回 null */
export function firstMatchContext(
  md: string,
  query: string,
  radius = 36
): MatchContext | null {
  const q = query.trim().toLowerCase();
  if (!q) return null;
  const idx = md.toLowerCase().indexOf(q);
  if (idx === -1) return null;
  const line = md.slice(0, idx).split('\n').length;
  const start = Math.max(0, idx - radius);
  const end = Math.min(md.length, idx + q.length + radius);
  const flat = (s: string) => s.replace(/\s+/g, ' ').trim();
  return {
    before: (start > 0 ? '…' : '') + flat(md.slice(start, idx)),
    match: md.slice(idx, idx + q.length),
    after: flat(md.slice(idx + q.length, end)) + (end < md.length ? '…' : ''),
    line,
  };
}
