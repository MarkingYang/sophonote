/**
 * NB-32 Spike 落地、NB-33 产品化 —— 行内补全编辑器装配层。
 *
 * 把 inlineCompletionPlugin（Decoration/状态机宿主）与 InlineCompletionController
 * （debounce/取消/绑定校验）接进 Crepe 编辑器，并处理 Tab/Esc 键位抢占。
 *
 * 键位冲突策略（完成标准「无建议时 Tab 行为不变」）：
 * Tab/Esc 走 window 捕获阶段监听（NB-04 ⌘E 冲突同款先例）——仅当有可见 ghost 时
 * preventDefault + stopPropagation 抢占，否则完全放行，列表缩进/表格导航等
 * Crepe 内置 Tab 行为零影响，与插件注册顺序解耦。
 *
 * NB-33：provider 缺省 = tauriCompletionProvider（AG-30 真实 CompletionService）；
 * 行为总开关在设置 completion_config.enabled（Rust 侧按请求读取生效），前端不再设常量门。
 * 其余契约（§4.3 请求契约/§4.2 状态机）不变。
 */
import { $prose } from '@milkdown/kit/utils';
import { Plugin } from '@milkdown/kit/prose/state';
import type { Editor } from '@milkdown/kit/core';
import type { EditorState } from '@milkdown/kit/prose/state';
import type { EditorView } from '@milkdown/kit/prose/view';
import {
  InlineCompletionController,
  tauriCompletionProvider,
  type InlineCompletionProvider,
} from '../services/inlineCompletion';
import { completionReportFeedback } from '../services/tauri';
import { useAppStore } from '../stores/appStore';
import {
  inlineCompletionPlugin,
  inlineCompletionKey,
  acceptTransaction,
  visibleGhost,
  docVersionOf,
  caretContext,
  type InlineCompletionMeta,
} from './inlineCompletionPlugin';

export interface InlineCompletionHostOptions {
  articleId: string;
  provider?: InlineCompletionProvider; // 缺省 = AG-30 真实补全（单测可注入替身）
  /** 编辑器保活但不可见时返回 false：暂停新触发并丢弃在途结果。 */
  isActive?: () => boolean;
}

interface UpdateHooks {
  onEditorUpdate?: (view: EditorView, prevState: EditorState) => void;
}

/**
 * 返回可直接传给 crepe.addFeature 的 feature。
 * 生命周期完全跟随编辑器实例：EditorView 创建时挂控制器/监听，销毁时全量拆除。
 */
export function inlineCompletionFeature(
  options: InlineCompletionHostOptions
): (editor: Editor) => void {
  return (editor: Editor) => {
    // 状态插件：ghost/文档版本的事务映射（Decoration 呈现）
    editor.use($prose(() => inlineCompletionPlugin()));

    // 驱动插件：ProseMirror view 钩子管理控制器与键位监听的生灭
    // （hooks 仅在本插件内使用——update 转发只此一处，避免双插件重复调用）
    const hooks: UpdateHooks = {};
    editor.use(
      $prose(
        () =>
          new Plugin({
            view(view: EditorView) {
              const controller = new InlineCompletionController(
                options.provider ?? tauriCompletionProvider(),
                {
                  onVisible: (result, binding) => {
                    if (options.isActive && !options.isActive()) return;
                    dispatchMeta(view, {
                      type: 'show',
                      ghost: {
                        pos: binding.prosePos,
                        text: result.text,
                        anchorHash: binding.anchorHash,
                        expectedDocVersion: binding.documentVersion,
                      },
                    });
                  },
                  onDismiss: () => dispatchMeta(view, { type: 'dismiss' }),
                }
              );

              const onKeyDown = (e: KeyboardEvent) => {
                if (e.defaultPrevented) return;
                if (options.isActive && !options.isActive()) return;
                const target = e.target as Node | null;
                if (!target || !view.dom.contains(target)) return;

                if (e.key === 'Tab') {
                  const ghost = visibleGhost(view.state);
                  if (!ghost) return; // 无建议 → 放行原 Tab 行为
                  e.preventDefault();
                  e.stopPropagation();
                  // 先收状态机再 dispatch：防止 update 钩子在 dispatch 期间看到 visible 又触发 dismiss
                  const text = controller.accept();
                  const tr = text != null ? acceptTransaction(view, ghost) : null;
                  if (tr) {
                    view.dispatch(tr);
                    // NB-33：显式接受反馈（§4.5 聚合计数，fire-and-forget，失败不影响编辑）
                    void completionReportFeedback(true).catch(() => {});
                  }
                  return;
                }

                if (e.key === 'Escape') {
                  if (!visibleGhost(view.state)) return; // 无建议 → 放行（菜单等原行为）
                  e.preventDefault();
                  e.stopPropagation();
                  controller.dismiss(); // 回调内 dispatch dismiss meta
                  // NB-33：显式拒绝反馈（隐式失效——继续输入/光标移动——不计数，避免噪音）
                  void completionReportFeedback(false).catch(() => {});
                }
              };
              window.addEventListener('keydown', onKeyDown, true);

              hooks.onEditorUpdate = (v, prevState) => {
                if (v.state === prevState) return;
                if (options.isActive && !options.isActive()) {
                  controller.cancel();
                  return;
                }
                // 选区态（非折叠）取消建议（设计 §4.1 第 5 条）
                if (!v.state.selection.empty) {
                  controller.cancel();
                  return;
                }
                const docChanged = v.state.doc !== prevState.doc;
                const selChanged = !v.state.selection.eq(prevState.selection);
                if (!docChanged && !selChanged) return; // 纯 meta 事务（show/dismiss）不触发
                const pos = v.state.selection.from;
                const { prefix, suffix } = caretContext(v.state, pos);
                // NB-33：上下文求值器在此捕获最新 state，fire 时（debounce 落定）惰性求值
                const stateAtSchedule = v.state;
                controller.scheduleTrigger({
                  articleId: options.articleId,
                  documentVersion: docVersionOf(v.state),
                  prosePos: pos,
                  prefix,
                  suffix,
                  getTitle: () => resolveArticleTitle(options.articleId),
                  getOutline: () => headingOutline(stateAtSchedule),
                });
              };

              return {
                update: (v: EditorView, prevState: EditorState) => {
                  hooks.onEditorUpdate?.(v, prevState);
                },
                destroy() {
                  window.removeEventListener('keydown', onKeyDown, true);
                  hooks.onEditorUpdate = undefined;
                  controller.dispose();
                },
              };
            },
          })
      )
    );
  };
}

// ---------- NB-33：真实上下文（设计 §4.4 上下文预算） ----------

/** 按 articleId 查笔记标题；未命中返回空串——绝不编造上下文 */
function resolveArticleTitle(articleId: string): string {
  return useAppStore.getState().articles.find((a) => a.id === articleId)?.title ?? '';
}

/**
 * 大纲 = 全文档标题文本（按文档顺序，上限 20 条防长文超预算）。
 * 与 MarkdownEditor.scrollToHeadingByIndex / noteOutline.extractOutline 同语义：
 * 只认真实 heading 节点（代码围栏内的 # 行是 code_block 文本，天然不算）。
 */
function headingOutline(state: EditorState): string[] {
  const outline: string[] = [];
  state.doc.descendants((node) => {
    if (outline.length >= 20) return false; // 预算上限：停止收集（不再深入该节点）
    if (node.type.name === 'heading') {
      const text = node.textContent.trim();
      if (text) outline.push(text);
    }
    return undefined;
  });
  return outline;
}

function dispatchMeta(view: EditorView, meta: InlineCompletionMeta): void {
  // 延迟到微任务：onDismiss 可能在 plugin view update 钩子（即 dispatch 过程中）被触发，
  // ProseMirror 禁止 dispatch 内再 dispatch；微任务时外层 dispatch 已完成，
  // 且 tr 基于最新 state 构造——若期间文档已变，show 的 expectedDocVersion
  // 校验会自然拒绝旧建议（正是状态机要的 stale 丢弃语义）
  queueMicrotask(() => {
    try {
      // 编辑器可能已销毁（异步建议到达时）：view 无公开 isDestroyed，try 兜底
      view.dispatch(view.state.tr.setMeta(inlineCompletionKey, meta));
    } catch {
      /* 已销毁：静默丢弃，建议随编辑器消失 */
    }
  });
}
