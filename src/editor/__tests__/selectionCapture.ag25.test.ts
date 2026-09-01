/**
 * AG-25：选区捕获（SelectionSnapshot）纯函数单测。
 * 零模型、零 DOM：真实 prosemirror-state（最小 schema，同 NB-32 口径）。
 */
import { describe, it, expect } from 'vitest';
import { Schema } from '@milkdown/kit/prose/model';
import { EditorState, TextSelection } from '@milkdown/kit/prose/state';
import { fnv1aHex } from '../selection/fnv1a';
import {
  blockPathAt,
  buildSelectionSnapshot,
  locateContexts,
  selectedTextOf,
} from '../selection/capture';
import { SELECTION_CONTEXT_CHARS } from '../selection/types';

const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    paragraph: { group: 'block', content: 'text*' },
    text: { group: 'inline' },
  },
});

function makeState(paragraphs: string[], selFrom?: number, selTo?: number) {
  const doc = schema.node(
    'doc',
    null,
    paragraphs.map((t) =>
      schema.node('paragraph', null, t.length > 0 ? [schema.text(t)] : [])
    )
  );
  return EditorState.create({
    doc,
    ...(selFrom != null && selTo != null
      ? { selection: TextSelection.create(doc, selFrom, selTo) }
      : {}),
  });
}

describe('fnv1aHex（与 Rust content_hash 同口径）', () => {
  it('空串 = offset basis（双侧共测锚点向量）', () => {
    expect(fnv1aHex('')).toBe('cbf29ce484222325');
  });

  it('稳定、敏感、16 位 hex', () => {
    expect(fnv1aHex('abc')).toBe(fnv1aHex('abc'));
    expect(fnv1aHex('abc')).not.toBe(fnv1aHex('abd'));
    expect(fnv1aHex('选区文本')).toMatch(/^[0-9a-f]{16}$/);
  });
});

describe('blockPathAt / selectedTextOf', () => {
  it('块路径 = 各层祖先索引', () => {
    const state = makeState(['甲段文字', '乙段文字']);
    // 位置模型：p1 占 0..6（内容 1..5），p2 占 6..12（内容 7..11）
    expect(blockPathAt(state.doc, 2)).toEqual([0]);
    expect(blockPathAt(state.doc, 8)).toEqual([1]);
  });

  it('空选区 → null；范围选区 → 选中文本', () => {
    const caret = makeState(['甲段文字']);
    expect(selectedTextOf(caret)).toBeNull();
    const ranged = makeState(['甲段文字', '乙段文字'], 7, 11);
    expect(selectedTextOf(ranged)).toBe('乙段文字');
  });
});

describe('locateContexts（唯一命中才采用，不猜测）', () => {
  it('唯一命中 → 前后文窗口', () => {
    const md = 'aaaaTARGETbbbb';
    const ctx = locateContexts(md, 'TARGET', 4);
    expect(ctx.beforeContext).toBe('aaaa');
    expect(ctx.afterContext).toBe('bbbb');
  });

  it('多命中 → 空上下文（交给 Rust 判冲突）', () => {
    const md = '重复句。\n重复句。';
    expect(locateContexts(md, '重复句。', SELECTION_CONTEXT_CHARS)).toEqual({
      beforeContext: '',
      afterContext: '',
    });
  });

  it('0 命中（序列化差异）→ 空上下文', () => {
    expect(locateContexts('正文甲', '不存在的选区', 80)).toEqual({
      beforeContext: '',
      afterContext: '',
    });
  });

  it('窗口截断不越界', () => {
    const ctx = locateContexts('abTARGETcd', 'TARGET', 100);
    expect(ctx.beforeContext).toBe('ab');
    expect(ctx.afterContext).toBe('cd');
  });
});

describe('buildSelectionSnapshot', () => {
  const base = {
    articleId: 'a1',
    projectId: 'p1',
    baseVersion: 3,
    markdown: '第一段内容。\n\n第二段内容。\n\n第三段内容。',
    proseFrom: 7,
    proseTo: 13,
    blockPath: [1],
  };

  it('完整快照：hash + 前后文 + 版本绑定', () => {
    const snap = buildSelectionSnapshot({
      ...base,
      selectedMarkdown: '第二段内容。',
      capturedAt: 123456,
    });
    expect(snap).not.toBeNull();
    expect(snap!.articleId).toBe('a1');
    expect(snap!.baseVersion).toBe(3);
    expect(snap!.selectedTextHash).toBe(fnv1aHex('第二段内容。'));
    expect(snap!.beforeContext.endsWith('\n\n')).toBe(true);
    expect(snap!.afterContext.startsWith('\n\n')).toBe(true);
    expect(snap!.beforeHash).toBe(fnv1aHex(snap!.beforeContext));
    expect(snap!.afterHash).toBe(fnv1aHex(snap!.afterContext));
    expect(snap!.capturedAt).toBe(123456);
    expect(snap!.selectionId).toContain('a1');
  });

  it('空选区（from === to）→ null', () => {
    expect(
      buildSelectionSnapshot({ ...base, proseFrom: 5, proseTo: 5, selectedMarkdown: 'x' })
    ).toBeNull();
  });

  it('空选中文本 → null', () => {
    expect(buildSelectionSnapshot({ ...base, selectedMarkdown: '   ' })).toBeNull();
  });

  it('选中文本在源中多命中 → 上下文为空（不猜测）', () => {
    const snap = buildSelectionSnapshot({
      ...base,
      markdown: '重复段落。\n\n重复段落。',
      selectedMarkdown: '重复段落。',
    });
    expect(snap!.beforeContext).toBe('');
    expect(snap!.afterContext).toBe('');
    expect(snap!.beforeHash).toBe(fnv1aHex(''));
  });
});
