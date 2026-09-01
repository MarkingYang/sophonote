/**
 * NB-32：Inline Completion Spike —— ProseMirror 插件层单测。
 * 用真实 prosemirror-state（最小 schema，无 DOM 渲染）验证：
 * ghost 状态机事务映射（show/accept/dismiss/文档变化/光标移动）、
 * 文档版本绑定校验、accept 事务构造、上下文窗口提取。
 */
import { describe, it, expect } from 'vitest';
import { Schema } from '@milkdown/kit/prose/model';
import { EditorState, TextSelection } from '@milkdown/kit/prose/state';
import type { EditorView } from '@milkdown/kit/prose/view';
import {
  inlineCompletionPlugin,
  inlineCompletionKey,
  acceptTransaction,
  visibleGhost,
  docVersionOf,
  caretContext,
  type GhostInfo,
} from '../inlineCompletionPlugin';

const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    paragraph: { group: 'block', content: 'text*' },
    text: { group: 'inline' },
  },
});

function makeDoc(text: string) {
  return schema.node('doc', null, [
    schema.node('paragraph', null, text.length > 0 ? [schema.text(text)] : []),
  ]);
}

function makeState(text = '你好世界') {
  const doc = makeDoc(text);
  return EditorState.create({
    doc,
    plugins: [inlineCompletionPlugin()],
    selection: TextSelection.atEnd(doc),
  });
}

function ghost(pos: number, expectedDocVersion = 0): GhostInfo {
  return { pos, text: '（建议文本）', anchorHash: 'h1', expectedDocVersion };
}

/**
 * doc=「你好世界」时的位置模型（paragraph 开闭各占 1 位）：
 * 0=doc 级段前 · 1=段内首字前 · 2..4=字间 · 5=段内末字后（=atEnd 光标位） · 6=doc 级段后
 */
const END_POS = 5;

describe('inlineCompletionPlugin 状态机', () => {
  it('初始：无 ghost、版本 0', () => {
    const state = makeState();
    expect(visibleGhost(state)).toBeNull();
    expect(docVersionOf(state)).toBe(0);
  });

  it('show meta 版本匹配 → ghost 可见', () => {
    const state = makeState();
    const next = state.apply(
      state.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) })
    );
    const g = visibleGhost(next);
    expect(g).not.toBeNull();
    expect(g!.text).toBe('（建议文本）');
    expect(g!.pos).toBe(END_POS);
  });

  it('show meta 版本不匹配 → 丢弃（旧请求不得呈现）', () => {
    const state = makeState();
    const next = state.apply(
      state.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 5) })
    );
    expect(visibleGhost(next)).toBeNull();
  });

  it('文档变化（继续输入）→ ghost 清除且版本 +1', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    expect(visibleGhost(s1)).not.toBeNull();
    const s2 = s1.apply(s1.tr.insertText('新', END_POS, END_POS));
    expect(visibleGhost(s2)).toBeNull();
    expect(docVersionOf(s2)).toBe(1);
  });

  it('光标移动（无文档变化）→ ghost 清除', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    const s2 = s1.apply(s1.tr.setSelection(TextSelection.create(s1.doc, 3)));
    expect(visibleGhost(s2)).toBeNull();
    expect(docVersionOf(s2)).toBe(0);
  });

  it('纯 meta 事务（dismiss）→ ghost 清除、版本不变', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    const s2 = s1.apply(s1.tr.setMeta(inlineCompletionKey, { type: 'dismiss' }));
    expect(visibleGhost(s2)).toBeNull();
    expect(docVersionOf(s2)).toBe(0);
  });

  it('光标未动且无变化 → ghost 保留', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    // scroll/forceUpdate 类事务：选择不变（仍在原位）、文档不变
    const s2 = s1.apply(s1.tr.setSelection(TextSelection.atEnd(s1.doc)));
    expect(visibleGhost(s2)).not.toBeNull();
  });
});

describe('acceptTransaction', () => {
  it('插入建议文本、光标置于其后（普通编辑事务，可进既有保存链路）', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    const g = visibleGhost(s1)!;
    const fakeView = { state: s1 } as unknown as EditorView;
    const tr = acceptTransaction(fakeView, g);
    expect(tr).not.toBeNull();
    const s2 = s1.apply(tr!);
    expect(s2.doc.textContent).toBe('你好世界（建议文本）');
    expect(s2.selection.from).toBe(END_POS + g.text.length);
    expect(visibleGhost(s2)).toBeNull(); // accept meta 已清 ghost
    expect(docVersionOf(s2)).toBe(1); // 插入算文档变化
  });

  it('位置漂移保护：光标已离开 ghost 落点 → 拒绝构造事务', () => {
    const base = makeState();
    const s1 = base.apply(base.tr.setMeta(inlineCompletionKey, { type: 'show', ghost: ghost(END_POS, 0) }));
    const s2 = s1.apply(s1.tr.setSelection(TextSelection.create(s1.doc, 3)));
    // 假设 ghost 仍被外部持有（实际插件状态已清，此处只测纯函数防护）
    const fakeView = { state: s2 } as unknown as EditorView;
    expect(acceptTransaction(fakeView, ghost(END_POS, 0))).toBeNull();
  });
});

describe('caretContext', () => {
  it('提取光标前后窗口', () => {
    const state = makeState('前半段文本后半段');
    // 段内位置：1=首字前；光标在「段/文」之间 = 1 + 3 = 4
    const { prefix, suffix } = caretContext(state, 4);
    expect(prefix).toBe('前半段');
    expect(suffix).toBe('文本后半段');
  });

  it('文档尾光标：suffix 为空', () => {
    const state = makeState('结尾');
    const end = 1 + 2; // doc(0) + paragraph 开(1) + 2 字 → 段尾 = 3
    const { prefix, suffix } = caretContext(state, end);
    expect(prefix).toBe('结尾');
    expect(suffix).toBe('');
  });
});
