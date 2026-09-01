/**
 * AG-32：文档内 diff 装饰层。
 *
 * 提案只作为 ProseMirror Decoration 呈现，不写入文档模型，因此不会触发 dirty / 自动保存。
 * 每个 hunk 紧跟在原文目标块之后；原块以删除色标出，新增内容在文档流内显示。
 * 每处修改就地 ✓/×；全部区域完成决策后由统一变更会话自动提交接受子集。
 */
import { Plugin, PluginKey } from '@milkdown/kit/prose/state';
import type { Transaction } from '@milkdown/kit/prose/state';
import type { Node as ProseNode } from '@milkdown/kit/prose/model';
import { Decoration, DecorationSet } from '@milkdown/kit/prose/view';
import type { EditorView } from '@milkdown/kit/prose/view';
import { blockIndexAtLine } from './selection/lineRange';
import type { ChangeSessionPhase, HunkDecision } from '../services/changeSession';

export interface DocumentDiffHunk {
  startLine: number;
  contextBefore: string[];
  removed: string[];
  added: string[];
  contextAfter: string[];
}

export interface DocumentDiffSuggestion {
  operationId: string;
  /** 兼容旧调用方；实际呈现按每个 hunk 独立定位，不再用单一浮层锚点 */
  anchorLine: number;
  hunks: DocumentDiffHunk[];
  mode: 'inline' | 'block';
  inlineText?: string;
  phase: ChangeSessionPhase;
  decisions: HunkDecision[];
  pendingAction?: 'apply' | 'reject' | 'undo' | null;
  error?: string | null;
}

export interface DocumentDiffTarget {
  hunkIndex: number;
  pos: number;
  from: number;
  to: number;
}

export interface DocumentDiffProposal {
  suggestion: DocumentDiffSuggestion;
  targets: DocumentDiffTarget[];
}

export interface DocumentDiffCallbacks {
  onDecision?: (hunkIndex: number, decision: Exclude<HunkDecision, 'pending'>) => void;
  onDecisionAll?: (decision: Exclude<HunkDecision, 'pending'>) => void;
}

/**
 * 把后端已经校验并提交的 hunk 应用到当前前端基线。
 * 返回 null 表示本地基线与 hunk 的 removed 行不一致；调用方此时回退数据库刷新，
 * 绝不猜测覆盖。降序应用与 Rust document_apply_patch 的实现保持一致。
 */
export function applyDocumentDiffHunks(
  markdown: string,
  hunks: DocumentDiffHunk[],
  approvedHunks?: number[]
): string | null {
  const selected = approvedHunks ?? hunks.map((_, index) => index);
  const indexes = Array.from(new Set(selected)).sort((a, b) => a - b);
  if (indexes.length === 0 || indexes.some((index) => index < 0 || index >= hunks.length)) {
    return null;
  }
  const lines = markdown.split('\n');
  for (const index of indexes) {
    const hunk = hunks[index];
    const end = hunk.startLine + hunk.removed.length;
    if (hunk.startLine < 0 || end > lines.length) return null;
    if (lines.slice(hunk.startLine, end).some((line, offset) => line !== hunk.removed[offset])) {
      return null;
    }
  }
  for (const index of indexes.slice().reverse()) {
    const hunk = hunks[index];
    lines.splice(hunk.startLine, hunk.removed.length, ...hunk.added);
  }
  return lines.join('\n');
}

/** 每个 hunk 独立映射到当前 Markdown 对应的 ProseMirror 顶层块。 */
export function buildDocumentDiffProposal(
  markdown: string,
  suggestion: DocumentDiffSuggestion,
  doc: ProseNode
): DocumentDiffProposal {
  const blocks: { from: number; to: number }[] = [];
  doc.forEach((node, offset) => {
    blocks.push({ from: offset, to: offset + node.nodeSize });
  });
  const fallback = blocks[blocks.length - 1] ?? { from: 0, to: doc.content.size };
  const targets = suggestion.hunks.map((hunk, hunkIndex) => {
    // PatchHunk.startLine 是 0-based；定位到 removed 区末行（纯插入则落在插入锚点行）。
    const anchorLine = Math.max(
      1,
      hunk.startLine + hunk.contextBefore.length + Math.max(hunk.removed.length, 1)
    );
    const index = blockIndexAtLine(markdown, anchorLine);
    const block = index >= 0 ? blocks[Math.min(index, blocks.length - 1)] ?? fallback : fallback;
    return { hunkIndex, pos: block.to, from: block.from, to: block.to };
  });
  return { suggestion, targets };
}

interface DocumentDiffState {
  proposal: DocumentDiffProposal | null;
  activeHunk: number;
}

export type DocumentDiffMeta =
  | { type: 'show'; proposal: DocumentDiffProposal }
  | { type: 'dismiss' }
  | { type: 'navigate'; hunkIndex: number };

export const documentDiffKey = new PluginKey<DocumentDiffState>('ag32-document-diff');

export function documentDiffPlugin(callbacks: DocumentDiffCallbacks): Plugin {
  // ProseMirror 会在光标移动、选区变化等每个 state 更新上读取 decorations。
  // diff state 与 doc 均未变化时复用同一 DecorationSet，避免长文里反复分配全部 widget。
  let cachedDiffState: DocumentDiffState | null = null;
  let cachedDoc: ProseNode | null = null;
  let cachedDecorations = DecorationSet.empty;
  return new Plugin({
    key: documentDiffKey,
    state: {
      init: (): DocumentDiffState => ({ proposal: null, activeHunk: 0 }),
      apply(tr: Transaction, prev: DocumentDiffState): DocumentDiffState {
        const meta = tr.getMeta(documentDiffKey) as DocumentDiffMeta | undefined;
        if (meta?.type === 'dismiss') {
          return { proposal: null, activeHunk: 0 };
        }
        if (meta?.type === 'show') {
          const sameOperation = prev.proposal?.suggestion.operationId === meta.proposal.suggestion.operationId;
          const decisions = meta.proposal.suggestion.decisions;
          const previousActive = Math.min(prev.activeHunk, Math.max(0, decisions.length - 1));
          const firstPending = decisions.findIndex((decision) => decision === 'pending');
          return {
            proposal: meta.proposal,
            activeHunk: sameOperation && decisions[previousActive] === 'pending'
              ? previousActive
              : Math.max(0, firstPending),
          };
        }
        if (meta?.type === 'navigate' && prev.proposal) {
          return {
            ...prev,
            activeHunk: clamp(meta.hunkIndex, 0, Math.max(0, prev.proposal.suggestion.hunks.length - 1)),
          };
        }
        if (!tr.docChanged || !prev.proposal) return prev;

        // 用户在审批前继续编辑时让装饰跟随事务映射；真正批准仍由后端版本/锚点复检，
        // 位置漂移只影响展示，绝不会绕过冲突保护。
        const max = tr.doc.content.size;
        return {
          ...prev,
          proposal: {
            ...prev.proposal,
            targets: prev.proposal.targets.map((target) => ({
              ...target,
              pos: clamp(tr.mapping.map(target.pos, 1), 0, max),
              from: clamp(tr.mapping.map(target.from, -1), 0, max),
              to: clamp(tr.mapping.map(target.to, 1), 0, max),
            })),
          },
        };
      },
    },
    props: {
      decorations(state) {
        const diffState = documentDiffKey.getState(state);
        if (!diffState?.proposal) return DecorationSet.empty;
        if (diffState === cachedDiffState && state.doc === cachedDoc) return cachedDecorations;
        const { proposal, activeHunk } = diffState;
        const decorations: Decoration[] = [];
        if (proposal.suggestion.hunks.length > 1) {
          decorations.push(
            Decoration.widget(
              0,
              (view) => buildReviewBar(view, proposal, activeHunk, callbacks),
              {
                key: `${proposal.suggestion.operationId}:review:${activeHunk}:${proposal.suggestion.phase}:${proposal.suggestion.decisions.join(',')}`,
                side: -1,
                ignoreSelection: true,
                stopEvent: () => true,
              }
            )
          );
        }
        for (const target of proposal.targets) {
          const hunk = proposal.suggestion.hunks[target.hunkIndex];
          if (!hunk) continue;
          const decision = proposal.suggestion.decisions[target.hunkIndex] ?? 'pending';
          if (
            hunk.removed.length > 0 &&
            target.from < target.to &&
            decision === 'pending'
          ) {
            decorations.push(
              Decoration.node(target.from, target.to, {
                class: 'hb-document-diff-target',
              })
            );
          }
          // 拒绝后原文已是最终视觉结果，不再保留灰色建议块。
          if (decision === 'rejected') continue;
          decorations.push(
            Decoration.widget(
              target.pos,
              () => buildHunkDom(proposal, target.hunkIndex, callbacks),
              {
                key: `${proposal.suggestion.operationId}:${target.hunkIndex}:${proposal.suggestion.phase}:${proposal.suggestion.decisions[target.hunkIndex] ?? 'pending'}`,
                side: 1,
                ignoreSelection: true,
                stopEvent: () => true,
              }
            )
          );
        }
        try {
          cachedDiffState = diffState;
          cachedDoc = state.doc;
          cachedDecorations = DecorationSet.create(state.doc, decorations);
          return cachedDecorations;
        } catch {
          // 极端事务映射后节点边界失效时宁可隐藏建议，也不能让编辑器渲染崩溃。
          return DecorationSet.empty;
        }
      },
    },
  });
}

function buildHunkDom(
  proposal: DocumentDiffProposal,
  hunkIndex: number,
  callbacks: DocumentDiffCallbacks
): HTMLElement {
  const { suggestion } = proposal;
  const hunk = suggestion.hunks[hunkIndex];
  const compact =
    suggestion.hunks.length === 1 &&
    hunk.added.length === 1 &&
    suggestionPreviewLine(hunk.added[0] ?? '').length <= 48;
  const root = document.createElement('section');
  root.className = `hb-document-diff${
    suggestion.mode === 'inline' && suggestion.hunks.length === 1 ? ' hb-document-diff-inline' : ''
  }${suggestion.hunks.length > 1 ? ' hb-document-diff-batch' : ''}${
    compact ? ' hb-document-diff-compact' : ' hb-document-diff-long'
  }`;
  root.contentEditable = 'false';
  root.dataset.hunkIndex = String(hunkIndex);
  root.dataset.diffOperation = suggestion.operationId;
  const decision = suggestion.decisions[hunkIndex] ?? 'pending';
  root.dataset.hunkDecision = decision;
  if (decision !== 'pending') root.classList.add(`hb-document-diff-hunk-${decision}`);
  root.setAttribute('aria-label', `AI 修改建议 ${hunkIndex + 1}/${suggestion.hunks.length}`);

  const body = document.createElement('div');
  body.className = 'hb-document-diff-body';
  // 原文已在上方原位置标红，不在建议区重复一遍；这里只呈现预期新增/替换文本。
  for (const line of hunk.added) body.append(diffLine('add', suggestionPreviewLine(line)));
  if (hunk.added.length === 0 && hunk.removed.length > 0) {
    body.append(diffLine('context', '删除上方内容'));
  } else if (hunk.removed.length === 0 && hunk.added.length === 0) {
    body.append(diffLine('context', '此处没有可显示的文本变化'));
  }
  root.append(body);
  if (decision === 'pending') {
    const controls = document.createElement('div');
    controls.className = 'hb-document-diff-choice';
    const accept = actionButton('✓', `使用第 ${hunkIndex + 1} 处修改`, 'primary', () =>
      callbacks.onDecision?.(hunkIndex, 'accepted')
    );
    const reject = actionButton('×', `不使用第 ${hunkIndex + 1} 处修改`, 'danger', () =>
      callbacks.onDecision?.(hunkIndex, 'rejected')
    );
    const locked = suggestion.phase !== 'proposed';
    accept.disabled = locked;
    reject.disabled = locked;
    controls.append(accept, reject);
    root.append(controls);
  }
  if (suggestion.error) {
    const error = document.createElement('div');
    error.className = 'hb-document-diff-error';
    error.dataset.diffRuntimeError = 'true';
    error.textContent = suggestion.error;
    error.setAttribute('role', 'alert');
    root.append(error);
  }

  return root;
}

/** 文档内常驻审阅条：大批量修改时无需滚到最后一个 hunk 才能提交。 */
function buildReviewBar(
  view: EditorView,
  proposal: DocumentDiffProposal,
  activeHunk: number,
  callbacks: DocumentDiffCallbacks
): HTMLElement {
  const pendingHunks = proposal.suggestion.decisions
    .map((decision, index) => decision === 'pending' ? index : -1)
    .filter((index) => index >= 0);
  const pendingCount = pendingHunks.length;
  const bar = document.createElement('nav');
  bar.className = 'hb-document-diff-reviewbar';
  bar.contentEditable = 'false';
  bar.dataset.diffOperation = proposal.suggestion.operationId;
  bar.setAttribute('aria-label', 'AI 修改审阅');

  const summary = document.createElement('strong');
  summary.textContent = `AI 修改 · ${pendingCount} 处未确认`;
  const position = document.createElement('span');
  position.className = 'hb-document-diff-position';
  const activePendingPosition = Math.max(0, pendingHunks.indexOf(activeHunk));
  position.textContent = pendingCount > 0 ? `第 ${activePendingPosition + 1}/${pendingCount} 处` : '';

  const navigate = (delta: number) => {
    if (pendingCount === 0) return;
    const nextPosition = (activePendingPosition + delta + pendingCount) % pendingCount;
    const next = pendingHunks[nextPosition];
    view.dispatch(
      view.state.tr.setMeta(documentDiffKey, { type: 'navigate', hunkIndex: next } satisfies DocumentDiffMeta)
    );
    requestAnimationFrame(() => {
      view.dom
        .querySelector<HTMLElement>(`.hb-document-diff[data-hunk-index="${next}"]`)
        ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    });
  };
  const acceptAll = actionButton('✓', '使用全部未确认修改', 'primary', () =>
    callbacks.onDecisionAll?.('accepted')
  );
  const rejectAll = actionButton('×', '放弃全部未确认修改', 'danger', () =>
    callbacks.onDecisionAll?.('rejected')
  );
  acceptAll.dataset.diffBulkAction = 'accept';
  rejectAll.dataset.diffBulkAction = 'reject';
  const previous = actionButton('‹', '上一处修改', 'secondary', () => navigate(-1));
  const next = actionButton('›', '下一处修改', 'secondary', () => navigate(1));
  const locked = pendingCount === 0 || proposal.suggestion.phase !== 'proposed';
  acceptAll.disabled = locked;
  rejectAll.disabled = locked;
  previous.disabled = pendingCount < 2 || proposal.suggestion.phase === 'applying';
  next.disabled = pendingCount < 2 || proposal.suggestion.phase === 'applying';

  bar.append(summary, position, acceptAll, rejectAll, previous, next);
  if (proposal.suggestion.error) {
    const error = document.createElement('span');
    error.className = 'hb-document-diff-error';
    error.dataset.diffRuntimeError = 'true';
    error.textContent = proposal.suggestion.error;
    error.setAttribute('role', 'alert');
    bar.append(error);
  }
  return bar;
}

function diffLine(kind: 'remove' | 'add' | 'context', text: string): HTMLDivElement {
  const line = document.createElement('div');
  line.className = `hb-document-diff-line hb-document-diff-${kind}`;
  line.textContent = text;
  return line;
}

function actionButton(
  text: string,
  label: string,
  tone: 'primary' | 'secondary' | 'danger',
  action: () => void
): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = `hb-document-diff-button hb-document-diff-button-${tone}`;
  button.textContent = text;
  button.title = label;
  button.setAttribute('aria-label', label);
  button.addEventListener('mousedown', (event) => {
    event.preventDefault();
    event.stopPropagation();
  });
  button.addEventListener('click', (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (button.disabled) return;
    action();
  });
  return button;
}

/**
 * 建议预览使用正文阅读形态，不把 Markdown 转义符直接暴露给用户。
 * 这里只改变 Decoration 的显示文本；真正写盘仍使用原始 replacementMarkdown。
 */
export function suggestionPreviewLine(markdown: string): string {
  return markdown
    .replace(/^\s{0,3}>\s?/, '')
    .replace(/^\s{0,3}#{1,6}\s+/, '')
    .replace(/^\s*[-+*]\s+/, '• ')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/\*\*([^*]+)\*\*/g, '$1')
    .replace(/__([^_]+)__/g, '$1')
    .replace(/~~([^~]+)~~/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/\\([\\`*_[\]{}()#+.!<>-])/g, '$1')
    .replace(/\s*\\\s*$/, '');
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}
