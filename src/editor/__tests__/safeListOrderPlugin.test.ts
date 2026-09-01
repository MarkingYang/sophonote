import { describe, expect, it } from 'vitest';
import { Schema } from '@milkdown/kit/prose/model';
import { EditorState } from '@milkdown/kit/prose/state';
import { createSafeListOrderPlugin } from '../safeListOrderPlugin';

const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    paragraph: { content: 'text*', group: 'block' },
    bullet_list: { content: 'list_item+', group: 'block', attrs: { spread: { default: false } } },
    ordered_list: {
      content: 'list_item+',
      group: 'block',
      attrs: { order: { default: 1 }, spread: { default: false } },
    },
    list_item: {
      content: 'paragraph+',
      attrs: {
        listType: { default: 'bullet' },
        label: { default: '' },
      },
    },
    text: {},
  },
});

describe('safe list order synchronization', () => {
  it('uses absolute positions for an ordered list after another block', () => {
    const paragraph = (text: string) => schema.node('paragraph', null, [schema.text(text)]);
    const orderedItem = (text: string) => schema.node(
      'list_item',
      { listType: 'ordered', label: '' },
      [paragraph(text)]
    );
    const legacyList = schema.node('bullet_list', null, [
      orderedItem('第一项'),
      orderedItem('第二项'),
    ]);
    const doc = schema.node('doc', null, [paragraph('列表前的正文'), legacyList]);
    const plugin = createSafeListOrderPlugin(
      schema.nodes.ordered_list,
      schema.nodes.bullet_list,
      schema.nodes.list_item
    );
    const state = EditorState.create({ schema, doc, plugins: [plugin] });

    // 上游实现会把 list 内的 relativePos=0 当作文档 pos=0，随后在错误节点上
    // setNodeMarkup；多项列表时更可能直接命中 text 并抛异常。
    const result = state.applyTransaction(state.tr.insertText('！', 2));

    const list = result.state.doc.child(1);
    expect(list.type).toBe(schema.nodes.ordered_list);
    expect(list.firstChild?.attrs).toMatchObject({ listType: 'ordered', label: '1.' });
    expect(list.lastChild?.attrs).toMatchObject({ listType: 'ordered', label: '2.' });
  });
});
