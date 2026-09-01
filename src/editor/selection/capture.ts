/**
 * AG-25：选区捕获（SelectionSnapshot Spike 的纯函数层）。
 *
 * 设计基线：docs/architecture.md——「将 ProseMirror 选区稳定映射到
 * Markdown 真相源」是先决难点。本模块只做**可单测的映射步骤**：
 * - blockPathAt / selectedTextOf：从 ProseMirror 状态取结构路径与选中文本；
 * - locateContexts：在 Markdown 源中定位选区并截取前后文——**唯一命中才采用**，
 *   0/多处命中一律空上下文交给 Rust 侧判冲突，绝不猜测位置（同 resolve_anchor 口径）；
 * - buildSelectionSnapshot：组装快照（hash 计算）。
 *
 * 运行时粘合（Crepe editorViewCtx + serializerCtx）在 MarkdownEditor 的
 * captureSelectionSnapshot handle 中，仅做薄封装。
 */
import type { Node as ProseNode } from '@milkdown/kit/prose/model';
import type { EditorState } from '@milkdown/kit/prose/state';
import { fnv1aHex } from './fnv1a';
import { SELECTION_CONTEXT_CHARS, type SelectionSnapshot } from './types';

/** pos 所在块的块路径（各层祖先索引；临时结构路径，重捕即失效） */
export function blockPathAt(doc: ProseNode, pos: number): number[] {
  const $pos = doc.resolve(pos);
  const path: number[] = [];
  for (let depth = 1; depth <= $pos.depth; depth += 1) {
    path.push($pos.index(depth - 1));
  }
  return path;
}

/** 当前选中文本（块间以换行连接）；空选区返回 null */
export function selectedTextOf(state: EditorState): string | null {
  const { from, to } = state.selection;
  if (from === to) return null;
  return state.doc.textBetween(from, to, '\n');
}

/**
 * 在 Markdown 真相源中定位选中文本并截取前后文窗口。
 * 铁律：唯一命中才采用；0 命中（序列化差异）或多命中（重复文本）→ 空上下文，
 * 由 Rust 侧 TextAnchor 解析判冲突——捕获端同样不猜测位置。
 */
export function locateContexts(
  markdown: string,
  selected: string,
  maxChars: number
): { beforeContext: string; afterContext: string } {
  if (!selected) return { beforeContext: '', afterContext: '' };
  const first = markdown.indexOf(selected);
  if (first < 0) return { beforeContext: '', afterContext: '' };
  if (markdown.indexOf(selected, first + 1) >= 0) {
    return { beforeContext: '', afterContext: '' };
  }
  return {
    beforeContext: markdown.slice(Math.max(0, first - maxChars), first),
    afterContext: markdown.slice(
      first + selected.length,
      first + selected.length + maxChars
    ),
  };
}

export interface SnapshotInput {
  articleId: string;
  projectId?: string;
  baseVersion: number;
  /** 当前 Markdown 真相源（crepe.getMarkdown()） */
  markdown: string;
  proseFrom: number;
  proseTo: number;
  blockPath: number[];
  /** 选区序列化 Markdown（Milkdown serializer 输出；降级 = 选中文本） */
  selectedMarkdown: string;
  capturedAt?: number;
}

/** 纯构建器：空选区/空选中文本 → null；否则组装带 hash 的完整快照 */
export function buildSelectionSnapshot(input: SnapshotInput): SelectionSnapshot | null {
  if (input.proseFrom === input.proseTo) return null;
  if (!input.selectedMarkdown.trim()) return null;
  const capturedAt = input.capturedAt ?? Date.now();
  const { beforeContext, afterContext } = locateContexts(
    input.markdown,
    input.selectedMarkdown,
    SELECTION_CONTEXT_CHARS
  );
  return {
    selectionId: `sel-${input.articleId}-${input.proseFrom}-${input.proseTo}-${capturedAt}`,
    articleId: input.articleId,
    projectId: input.projectId,
    baseVersion: input.baseVersion,
    proseFrom: input.proseFrom,
    proseTo: input.proseTo,
    selectedMarkdown: input.selectedMarkdown,
    selectedTextHash: fnv1aHex(input.selectedMarkdown),
    blockPath: input.blockPath,
    beforeContext,
    afterContext,
    beforeHash: fnv1aHex(beforeContext),
    afterHash: fnv1aHex(afterContext),
    capturedAt,
  };
}
