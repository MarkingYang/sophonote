/**
 * N5 任务聚合：从 Markdown 文档派生任务清单的纯函数集。
 *
 * 设计：Tasks 页不复制任务数据，实时扫描 articles 派生「笔记任务」；
 * 勾选直接写回源文档 .md（真相源唯一），与预览态勾选（MarkdownView）共用同一规则。
 * 围栏感知：代码块内的 `- [ ]` 不算任务（与 scanTaskLines 规则完全一致，防两边漂移）。
 */

export interface NoteTask {
  /** 1-based 源码行号（行级回链锚点，对齐 MarkdownView hb-line-N） */
  line: number;
  done: boolean;
  /** 去掉列表标记与复选框后的任务文本（保留行内 markdown 原文） */
  text: string;
}

const TASK_LINE = /^\s*(?:[-*+]|\d+[.)])\s+\[([ xX])\]\s*(.*)$/;

/** 按文档顺序收集任务（忽略代码围栏内） */
export function parseNoteTasks(md: string): NoteTask[] {
  const out: NoteTask[] = [];
  let inFence = false;
  md.split('\n').forEach((ln, i) => {
    if (/^\s*```/.test(ln)) {
      inFence = !inFence;
      return;
    }
    if (inFence) return;
    const m = TASK_LINE.exec(ln);
    if (m) out.push({ line: i + 1, done: m[1] !== ' ', text: m[2].trim() });
  });
  return out;
}

/** 与 MarkdownView 预览勾选共用：文档顺序第 N 个任务的源码行号（1-based） */
export function scanTaskLines(md: string): number[] {
  return parseNoteTasks(md).map((t) => t.line);
}

/** 翻转指定行（1-based）的复选框；该行不是任务行时原样返回 */
export function toggleTaskLine(md: string, line: number): string {
  const lines = md.split('\n');
  const idx = line - 1;
  if (idx < 0 || idx >= lines.length) return md;
  const m = TASK_LINE.exec(lines[idx]);
  if (!m) return md;
  lines[idx] = lines[idx].replace(/\[[ xX]\]/, (s) => (s === '[ ]' ? '[x]' : '[ ]'));
  return lines.join('\n');
}

/**
 * NB-10 段落级嵌入（![[笔记#标题]]）勾选回写：把「段落内」的勾选变化映射回全文档。
 * section/newSection 行数必须一致（勾选只翻转单行复选框，不增删行），
 * 找到首个差异行 → 绝对行号 = startLine（段落起始 1-based）+ 行内偏移 → toggleTaskLine 全文档。
 * 形态异常（行数变化/无差异/目标行非任务）返回 null，调用方不写库。
 */
export function remapSectionToggle(
  fullMd: string,
  section: string,
  newSection: string,
  startLine: number
): string | null {
  const s = section.split('\n');
  const n = newSection.split('\n');
  if (s.length !== n.length) return null;
  for (let i = 0; i < s.length; i++) {
    if (s[i] !== n[i]) {
      const out = toggleTaskLine(fullMd, startLine + i);
      return out === fullMd ? null : out;
    }
  }
  return null;
}
