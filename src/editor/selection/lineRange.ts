/**
 * AG-31/AG-32：Markdown 源码行号 ↔ 顶层块映射（纯函数，单测覆盖）。
 *
 * - selectionLineRange：选区序列化 Markdown → 源码行号区间（chip「文档名 (16-18)」
 *   的行号来源）。Best-effort：首尾行在源码中逐行命中才返回，否则 null
 *   （UI 回落只显示摘录；真实定位仍走 TextAnchor hash 锚点，安全契约不变）。
 * - blockIndexAtLine：源码行号 → 顶层块下标（AG-32 原文内 diff Decoration
 *   映射到 ProseMirror 第 N 个顶层节点，不依赖 DOM 几何坐标）。
 *
 * 围栏感知与 noteLinks.ts / extractOutline 同口径（``` / ~~~ 行翻转）：
 * 围栏内空行不切块，避免代码围栏被拆成多个「块」导致 DOM 下标错位。
 */

/** 选区 → 源码行号区间（1-based，含首尾）；未命中返回 null */
export function selectionLineRange(
  source: string,
  selectedMarkdown: string
): [number, number] | null {
  const selLines = selectedMarkdown.split('\n');
  let first = 0;
  let last = selLines.length - 1;
  while (first <= last && !selLines[first].trim()) first++;
  while (last >= first && !selLines[last].trim()) last--;
  if (first > last) return null;
  const head = selLines[first].trim();
  const tail = selLines[last].trim();
  const span = last - first;
  const srcLines = source.split('\n');
  for (let i = 0; i < srcLines.length; i++) {
    if (srcLines[i].trim() !== head) continue;
    const j = i + span;
    // 首尾同时命中才认（防「同首行不同尾」误落）
    if (j < srcLines.length && srcLines[j].trim() === tail) {
      return [i + 1, j + 1];
    }
  }
  return null;
}

/** 顶层块起始行（0-based）：空行切块、围栏内不切 */
export function topLevelBlockStartLines(source: string): number[] {
  const starts: number[] = [];
  let inFence = false;
  let blockOpen = false;
  source.split('\n').forEach((ln, i) => {
    if (/^\s*(```|~~~)/.test(ln)) {
      if (!blockOpen) {
        starts.push(i);
        blockOpen = true;
      }
      inFence = !inFence;
      return;
    }
    if (inFence) return;
    if (!ln.trim()) {
      blockOpen = false;
      return;
    }
    if (!blockOpen) {
      starts.push(i);
      blockOpen = true;
    }
  });
  return starts;
}

/** 源码行号（1-based）→ 所属顶层块下标（0-based）；空文档 -1 */
export function blockIndexAtLine(source: string, line: number): number {
  const starts = topLevelBlockStartLines(source);
  if (starts.length === 0) return -1;
  const l = Math.max(0, line - 1);
  let idx = 0;
  for (let i = 0; i < starts.length; i++) {
    if (starts[i] <= l) idx = i;
    else break;
  }
  return idx;
}
