/**
 * Milkdown 7.22.0 syncListOrderPlugin 的安全替代。
 *
 * 上游在 bullet_list -> ordered_list 兼容分支中，对 `node.descendants` 返回的
 * 相对位置直接调用 `tr.setNodeMarkup`。列表不位于文档开头时，这个位置可能指向
 * text 节点，从而抛出 `NodeType.create can't construct text nodes`。
 * 这里保留原同步语义，只把子节点位置转换成文档绝对位置。
 */
import { $prose } from '@milkdown/kit/utils';
import {
  bulletListSchema,
  listItemSchema,
  orderedListSchema,
} from '@milkdown/kit/preset/commonmark';
import { Plugin } from '@milkdown/kit/prose/state';
import type { NodeType } from '@milkdown/kit/prose/model';

export function createSafeListOrderPlugin(
  orderedListType: NodeType,
  bulletListType: NodeType,
  listItemType: NodeType
): Plugin {
  return new Plugin({
    appendTransaction(transactions, _oldState, newState) {
      if (
        !newState.selection ||
        transactions.some((transaction) =>
          transaction.getMeta('addToHistory') === false || !transaction.isGeneric
        )
      ) {
        return null;
      }

      const updateLabel = (attrs: Record<string, unknown>, index: number, order = 1) => {
        const expected = `${index + order}.`;
        if (attrs.label === expected) return false;
        attrs.label = expected;
        return true;
      };

      let transaction = newState.tr;
      let changed = false;
      newState.doc.descendants((node, pos, parent, index) => {
        if (node.type === bulletListType) {
          const first = node.maybeChild(0);
          if (first?.type === listItemType && first.attrs.listType === 'ordered') {
            changed = true;
            transaction = transaction.setNodeMarkup(pos, orderedListType, { spread: true });
            node.descendants((child, relativePos, _childParent, childIndex) => {
              if (child.type === listItemType) {
                const attrs = { ...child.attrs };
                if (updateLabel(attrs, childIndex)) {
                  // Node.descendants 的位置相对当前 list 节点内容；+1 跨过 list 起始 token。
                  const absolutePos = pos + 1 + relativePos;
                  transaction = transaction.setNodeMarkup(absolutePos, undefined, attrs);
                }
              }
              return false;
            });
          }
        } else if (node.type === listItemType && parent?.type === orderedListType) {
          const attrs = { ...node.attrs };
          let itemChanged = false;
          if (attrs.listType !== 'ordered') {
            attrs.listType = 'ordered';
            itemChanged = true;
          }
          if (parent.maybeChild(0)) {
            itemChanged = updateLabel(attrs, index, parent.attrs.order ?? 1) || itemChanged;
          }
          if (itemChanged) {
            transaction = transaction.setNodeMarkup(pos, undefined, attrs);
            changed = true;
          }
        }
      });

      return changed ? transaction.setMeta('addToHistory', false) : null;
    },
  });
}

export const safeSyncListOrderPlugin = $prose((ctx) =>
  createSafeListOrderPlugin(
    orderedListSchema.type(ctx),
    bulletListSchema.type(ctx),
    listItemSchema.type(ctx)
  )
);
