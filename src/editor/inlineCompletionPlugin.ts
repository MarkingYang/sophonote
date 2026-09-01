/**
 * NB-32：Inline Completion Spike —— ProseMirror 插件层。
 *
 * 职责（设计 §3.1 inlineCompletionPlugin）：Decoration 呈现 ghost text、
 * 事务映射（doc 变化/光标移动即失效）、show/accept/dismiss meta 协议。
 * 硬约束：ghost text 只是 Decoration.widget，不成为文档节点、不触发 dirty/save；
 * 接受后由编辑器 dispatch 普通 insertText 事务，等同用户输入进入既有保存链路。
 *
 * Tab/Esc 键位不在本插件内抢——走 MarkdownEditor 的 DOM 捕获阶段监听
 * （NB-04 ⌘E 键位冲突同款先例），保证优先级不受 Crepe 内置键表插件顺序影响；
 * 本插件只导出纯函数供其查询当前是否有可见建议。
 */
import { Plugin, PluginKey, EditorState, TextSelection } from '@milkdown/kit/prose/state';
import type { Transaction } from '@milkdown/kit/prose/state';
import { Decoration, DecorationSet } from '@milkdown/kit/prose/view';
import type { EditorView } from '@milkdown/kit/prose/view';

export interface GhostInfo {
  pos: number;
  text: string;
  anchorHash: string;
  /** show 时的文档版本；事务映射校验用 */
  expectedDocVersion: number;
}

/** 插件状态（显式标注：防止 TS 从 init 把 ghost 推断成恒 null） */
export interface InlineCompletionState {
  docVersion: number;
  ghost: GhostInfo | null;
}

export const inlineCompletionKey = new PluginKey<InlineCompletionState>('nb32-inline-completion');

/** 插件 meta 协议 */
export type InlineCompletionMeta =
  | { type: 'show'; ghost: GhostInfo }
  | { type: 'accept' }
  | { type: 'dismiss' };

/** 装配层桥接钩子类型见 inlineCompletionSetup（update 转发只在驱动插件一处） */

export function inlineCompletionPlugin(): Plugin {
  return new Plugin({
    key: inlineCompletionKey,
    state: {
      init: (): InlineCompletionState => ({ docVersion: 0, ghost: null }),
      apply(tr, prev, _oldState, _newState) {
        const meta = tr.getMeta(inlineCompletionKey) as InlineCompletionMeta | undefined;

        // 文档版本：任何 docChanged +1（accept 插入也算，防止旧 ghost 复活）
        const docVersion = tr.docChanged ? prev.docVersion + 1 : prev.docVersion;

        if (meta?.type === 'show') {
          // 双重校验：show 携带的版本必须等于当前版本，否则丢弃（控制器已校验一次）
          if (meta.ghost.expectedDocVersion === docVersion) {
            return { docVersion, ghost: meta.ghost };
          }
          return { docVersion, ghost: null };
        }

        if (meta?.type === 'accept' || meta?.type === 'dismiss') {
          return { docVersion, ghost: null };
        }

        // 无 meta：任何 doc 变化或光标移动都使建议失效（设计 §4.1 第 5 条）
        if (!prev.ghost) return { docVersion, ghost: null };
        const moved = tr.selection.from !== prev.ghost.pos;
        if (tr.docChanged || moved) return { docVersion, ghost: null };
        return { docVersion, ghost: prev.ghost };
      },
    },
    props: {
      decorations(state) {
        const { ghost } = inlineCompletionKey.getState(state) ?? { ghost: null };
        if (!ghost) return DecorationSet.empty;
        const widget = Decoration.widget(ghost.pos, () => buildGhostDom(ghost.text), {
          side: 1, // 光标在前、ghost 在后
          ignoreSelection: true,
        });
        return DecorationSet.create(state.doc, [widget]);
      },
    },
  });
}

/** ghost text DOM（纯展示，pointer-events none，不可选中） */
export function buildGhostDom(text: string): HTMLElement {
  const span = document.createElement('span');
  span.className = 'hb-ghost-text';
  span.textContent = text;
  span.setAttribute('aria-hidden', 'true');
  return span;
}

// ---------- 供编辑器装配层使用的纯查询函数 ----------

/** 当前是否有可见 ghost（Tab 是否抢占的判据） */
export function visibleGhost(state: EditorState): GhostInfo | null {
  return inlineCompletionKey.getState(state)?.ghost ?? null;
}

/** 当前文档版本（请求绑定用） */
export function docVersionOf(state: EditorState): number {
  return inlineCompletionKey.getState(state)?.docVersion ?? 0;
}

/** 提取光标前后上下文窗口（prefix ≤400 字、suffix ≤200 字，设计 §4.4 近处优先） */
export function caretContext(state: EditorState, pos: number): { prefix: string; suffix: string } {
  const doc = state.doc;
  const from = Math.max(0, pos - 400);
  const to = Math.min(doc.content.size, pos + 200);
  return {
    prefix: doc.textBetween(from, pos, '\n', '￼'),
    suffix: doc.textBetween(pos, to, '\n', '￼'),
  };
}

/** 构造 accept 事务：插入建议文本并把光标置于其后（普通用户编辑，可 undo） */
export function acceptTransaction(view: EditorView, ghost: GhostInfo): Transaction | null {
  const { state } = view;
  // 位置漂移保护：ghost 落点必须仍是当前光标处且文档未变
  if (!state.selection.empty || state.selection.from !== ghost.pos) return null;
  if (ghost.pos > state.doc.content.size) return null;
  const tr = state.tr.insertText(ghost.text, ghost.pos, ghost.pos);
  tr.setSelection(TextSelection.create(tr.doc, ghost.pos + ghost.text.length));
  tr.setMeta(inlineCompletionKey, { type: 'accept' } satisfies InlineCompletionMeta);
  return tr;
}
