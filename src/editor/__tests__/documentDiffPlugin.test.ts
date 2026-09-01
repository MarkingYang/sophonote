import { describe, expect, it, vi } from 'vitest';
import { Schema } from '@milkdown/kit/prose/model';
import { EditorState } from '@milkdown/kit/prose/state';
import { EditorView } from '@milkdown/kit/prose/view';
import {
  applyDocumentDiffHunks,
  buildDocumentDiffProposal,
  documentDiffKey,
  documentDiffPlugin,
  suggestionPreviewLine,
  type DocumentDiffCallbacks,
  type DocumentDiffMeta,
  type DocumentDiffSuggestion,
} from '../documentDiffPlugin';

const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    paragraph: { content: 'text*', group: 'block', toDOM: () => ['p', 0] },
    text: {},
  },
});

function mountSuggestion(
  suggestion: DocumentDiffSuggestion,
  callbacks: DocumentDiffCallbacks
): { host: HTMLDivElement; view: EditorView } {
  const doc = schema.node('doc', null, [
    schema.node('paragraph', null, [schema.text('旧段落一')]),
    schema.node('paragraph', null, [schema.text('旧段落二')]),
  ]);
  const plugin = documentDiffPlugin(callbacks);
  const state = EditorState.create({ schema, doc, plugins: [plugin] });
  const host = document.createElement('div');
  const view = new EditorView(host, { state });
  const meta: DocumentDiffMeta = {
    type: 'show',
    proposal: buildDocumentDiffProposal('旧段落一\n\n旧段落二', suggestion, doc),
  };
  view.dispatch(view.state.tr.setMeta(documentDiffKey, meta));
  return { host, view };
}

describe('document diff interactions', () => {
  it('single hunk check/cross directly records the local decision', () => {
    const onDecision = vi.fn();
    const suggestion: DocumentDiffSuggestion = {
      operationId: 'op-single',
      anchorLine: 1,
      mode: 'inline',
      phase: 'proposed',
      decisions: ['pending'],
      hunks: [
        { startLine: 0, contextBefore: [], removed: ['旧段落一'], added: ['新段落一'], contextAfter: [] },
      ],
    };
    const { host, view } = mountSuggestion(suggestion, { onDecision });

    expect(host.querySelector('.hb-document-diff-reviewbar')).toBeNull();
    expect(host.textContent).not.toContain('− 旧段落一');
    const accept = host.querySelector<HTMLButtonElement>('.hb-document-diff-button-primary')!;
    const reject = host.querySelector<HTMLButtonElement>('.hb-document-diff-button-danger')!;
    expect(accept.textContent).toBe('✓');
    expect(accept.getAttribute('aria-label')).toBe('使用第 1 处修改');
    expect(reject.textContent).toBe('×');
    expect(reject.getAttribute('aria-label')).toBe('不使用第 1 处修改');
    expect(host.textContent).not.toContain('采纳');
    expect(host.textContent).not.toContain('保留原文');
    const compact = host.querySelector('.hb-document-diff-compact')!;
    expect(compact).not.toBeNull();
    expect(compact.children[0].classList.contains('hb-document-diff-body')).toBe(true);
    expect(compact.children[1].classList.contains('hb-document-diff-choice')).toBe(true);
    accept.click();
    expect(onDecision).toHaveBeenCalledWith(0, 'accepted');
    view.destroy();

    const second = mountSuggestion(suggestion, { onDecision });
    second.host.querySelector<HTMLButtonElement>('.hb-document-diff-button-danger')!.click();
    expect(onDecision).toHaveBeenCalledWith(0, 'rejected');
    second.view.destroy();
  });

  it('multiple hunks expose an independent check/cross at every region', () => {
    const onDecision = vi.fn();
    const onDecisionAll = vi.fn();
    const suggestion: DocumentDiffSuggestion = {
      operationId: 'op-multi',
      anchorLine: 3,
      mode: 'block',
      phase: 'proposed',
      decisions: ['pending', 'pending'],
      hunks: [
        { startLine: 0, contextBefore: [], removed: ['旧段落一'], added: ['新段落一'], contextAfter: [] },
        { startLine: 2, contextBefore: [], removed: ['旧段落二'], added: ['新段落二'], contextAfter: [] },
      ],
    };
    const { host, view } = mountSuggestion(suggestion, { onDecision, onDecisionAll });

    expect(host.querySelectorAll('.hb-document-diff')).toHaveLength(2);
    const review = host.querySelector('.hb-document-diff-reviewbar')!;
    expect(review.textContent).toContain('AI 修改 · 2 处未确认');
    expect(review.textContent).not.toContain('已决定');
    expect(host.querySelectorAll('.hb-document-diff .hb-document-diff-choice')).toHaveLength(2);

    const acceptAll = review.querySelector<HTMLButtonElement>('[data-diff-bulk-action="accept"]')!;
    const rejectAll = review.querySelector<HTMLButtonElement>('[data-diff-bulk-action="reject"]')!;
    expect(acceptAll.textContent).toBe('✓');
    expect(acceptAll.getAttribute('aria-label')).toBe('使用全部未确认修改');
    expect(rejectAll.textContent).toBe('×');
    expect(rejectAll.getAttribute('aria-label')).toBe('放弃全部未确认修改');
    acceptAll.click();
    expect(onDecisionAll).toHaveBeenCalledWith('accepted');

    const accept = host.querySelector<HTMLButtonElement>('.hb-document-diff[data-hunk-index="1"] .hb-document-diff-button-primary')!;
    accept.click();
    expect(onDecision).toHaveBeenCalledWith(1, 'accepted');
    view.destroy();
  });

  it('removes pending colors and controls as soon as a region is decided', () => {
    const acceptedSuggestion: DocumentDiffSuggestion = {
      operationId: 'op-decided',
      anchorLine: 3,
      mode: 'block',
      phase: 'proposed',
      decisions: ['accepted', 'pending'],
      hunks: [
        { startLine: 0, contextBefore: [], removed: ['旧段落一'], added: ['新段落一'], contextAfter: [] },
        { startLine: 2, contextBefore: [], removed: ['旧段落二'], added: ['新段落二'], contextAfter: [] },
      ],
    };
    const accepted = mountSuggestion(acceptedSuggestion, {});
    const acceptedHunk = accepted.host.querySelector<HTMLElement>('[data-hunk-index="0"]')!;
    expect(acceptedHunk.classList.contains('hb-document-diff-hunk-accepted')).toBe(true);
    expect(acceptedHunk.querySelector('.hb-document-diff-choice')).toBeNull();
    expect(accepted.host.querySelectorAll('.hb-document-diff-target')).toHaveLength(1);
    expect(accepted.host.querySelector('.hb-document-diff-reviewbar')?.textContent)
      .toContain('AI 修改 · 1 处未确认');
    accepted.view.destroy();

    const rejected = mountSuggestion({
      ...acceptedSuggestion,
      operationId: 'op-rejected',
      decisions: ['rejected', 'pending'],
    }, {});
    expect(rejected.host.querySelector('[data-hunk-index="0"]')).toBeNull();
    expect(rejected.host.querySelectorAll('.hb-document-diff-target')).toHaveLength(1);
    rejected.view.destroy();
  });

  it('applies committed hunks locally in descending order and rejects a stale baseline', () => {
    const hunks = [
      { startLine: 0, contextBefore: [], removed: ['a'], added: ['A', 'A2'], contextAfter: [] },
      { startLine: 2, contextBefore: [], removed: ['c'], added: ['C'], contextAfter: [] },
    ];
    expect(applyDocumentDiffHunks('a\nb\nc', hunks)).toBe('A\nA2\nb\nC');
    expect(applyDocumentDiffHunks('a\nb\nc', hunks, [1])).toBe('a\nb\nC');
    expect(applyDocumentDiffHunks('changed\nb\nc', hunks)).toBeNull();
  });

  it('renders Markdown suggestions as reading text without changing the source payload', () => {
    const source = '> **本文档总结**\\';
    expect(suggestionPreviewLine(source)).toBe('本文档总结');
    expect(suggestionPreviewLine('- **第一点**：`AG_UI`')).toBe('• 第一点：AG_UI');
    expect(source).toBe('> **本文档总结**\\');
  });

  it('keeps long suggestion controls in a separate decision column', () => {
    const suggestion: DocumentDiffSuggestion = {
      operationId: 'op-long',
      anchorLine: 1,
      mode: 'block',
      phase: 'proposed',
      decisions: ['pending'],
      hunks: [{
        startLine: 0,
        contextBefore: [],
        removed: ['旧段落一'],
        added: ['这是一段明显超过七十二个字符的长建议，用于验证操作按钮进入独立决策列，不会覆盖生成文字或挤压正文阅读区域。这里继续补充长度。'],
        contextAfter: [],
      }],
    };
    const { host, view } = mountSuggestion(suggestion, {});
    const long = host.querySelector('.hb-document-diff-long')!;
    expect(long).not.toBeNull();
    expect(long.querySelector('.hb-document-diff-choice')).not.toBeNull();
    expect(long.querySelector('.hb-document-diff-body')).not.toBeNull();
    view.destroy();
  });
});
