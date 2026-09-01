import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { Crepe, CrepeFeature } from '@milkdown/crepe';
// crepe 主题样式已提升到 main.tsx 入口静态引入（性能债治理，同 MarkdownView 说明）
import { $prose, $shortcut, replaceAll } from '@milkdown/kit/utils';
import { commandsCtx, editorViewCtx, parserCtx, schemaCtx, serializerCtx } from '@milkdown/kit/core';
import { Slice } from '@milkdown/kit/prose/model';
import { closeHistory } from '@milkdown/kit/prose/history';
import { TextSelection } from '@milkdown/kit/prose/state';
import {
  listItemSchema,
  syncListOrderPlugin,
  wrapInBlockTypeCommand,
} from '@milkdown/kit/preset/commonmark';
import { resolveNoteAsset, saveNoteAsset } from '../../services/tauri';
import { inlineCompletionFeature } from '../../editor/inlineCompletionSetup';
import { inlineCompletionKey } from '../../editor/inlineCompletionPlugin';
import { blockPathAt, buildSelectionSnapshot } from '../../editor/selection/capture';
import type { SelectionSnapshot } from '../../editor/selection/types';
import {
  buildDocumentDiffProposal,
  documentDiffKey,
  documentDiffPlugin,
  type DocumentDiffMeta,
  type DocumentDiffSuggestion,
} from '../../editor/documentDiffPlugin';
import { safeSyncListOrderPlugin } from '../../editor/safeListOrderPlugin';
import { editorCodeLanguages } from '../../editor/mermaidCodeMirror';
import type { EditorViewCheckpoint } from '../../editor/viewCheckpoint';
import { registerPerfProbeTarget } from '../../services/perfProbeRegistry';
import type { HunkDecision, HunkDecisionTarget } from '../../services/changeSession';

/**
 * AG-32：编辑器内联建议块（Cursor ⌘K / Codex inline diff 口径的 SophoNote 落点）。
 * 数据源 = propose_document_patch 的 DiffPreviewPayload 子集；呈现层只读，
 * 落盘唯一入口仍是用户侧批准命令（documentApplyPatch），安全契约不变。
 * - mode=block：每个 hunk 在对应原文块后进入文档流，原文红标、新文绿标
 * - mode=inline：短文本纯插入 → 目标块后紧凑绿色建议
 * 多 hunk 作为一次原子提案整体 ✓/×，避免逐个落盘导致版本竞态。
 */
export type InlineSuggestion = DocumentDiffSuggestion;

/**
 * NB-04：任务清单快捷键 ⌘⌥9（延续 preset 的 ⌘⌥7 有序 / ⌘⌥8 无序数字惯例）。
 * 任务清单是笔记核心场景（N5 任务聚合 / NB-03 编辑态勾选），但 preset-gfm 只给了
 * `[ ] ` 输入规则与斜杠菜单入口，无键位。语义与斜杠菜单 Task List 完全一致：
 * wrapInBlockType 包成 checked=false 的 list_item（序列化即 `- [ ]`）。
 * 注意不能带 clearTextInCurrentBlockCommand——那是斜杠菜单清触发残留用的，
 * 快捷键场景调用会清掉用户正在写的正文。
 */
const taskListShortcut = $shortcut((ctx) => {
  const commands = ctx.get(commandsCtx);
  return {
    'Mod-Alt-9': (_state, dispatch) => {
      if (!dispatch) return false;
      commands.call(wrapInBlockTypeCommand.key, {
        nodeType: listItemSchema.type(ctx),
        attrs: { checked: false },
      });
      return true;
    },
  };
});

export interface MarkdownEditorHandle {
  getMarkdown: () => string;
  /**
   * NB-22：Crepe 实例是否已创建且未销毁。父组件在卸载/切换时机 flush 时，
   * 用它区分「内容合法为空」与「编辑器已不存在」（destroy 后 getMarkdown 返回 ''，
   * 无法单凭返回值判别，误判会把空内容写回覆盖用户正文）
   */
  isReady: () => boolean;
  /** 外部写回正文时原位替换 ProseMirror state，避免销毁并重建整套 Crepe。 */
  replaceMarkdown: (markdown: string, options?: { preserveHistory?: boolean }) => Promise<boolean>;
  /**
   * NB-05：编辑态大纲跳转——滚动到文档中第 index 个标题（0-based，文档顺序）。
   * 编辑态 DOM 是 ProseMirror 渲染（无 hb-line-N 锚点），故走 ProseMirror：
   * 收集全部 heading 节点位置，取第 index 个，coordsAtPos 换算容器内坐标后平滑滚动。
   * 与 DocWorkspace 的 extractOutline 同为「文档顺序第 N 个标题」语义，两边序号天然对齐
   * （代码围栏内的 # 行两侧都不算标题：解析端围栏感知，ProseMirror 端是 code_block 文本）
   */
  scrollToHeadingByIndex: (index: number) => void;
  /**
   * AG-25：捕获当前选区的 SelectionSnapshot（§5.2）。无选区 / 编辑器未就绪 → null。
   * 持久化位置 = selectedMarkdown + hash + 前后文（Rust 侧 TextAnchor 解析），
   * proseFrom/proseTo 仅供高亮。AG-26 的范围 chip 以此为准入。
   * async：editor.action 经 microtask 交付 ctx，无法同步取回。
   */
  captureSelectionSnapshot: (meta: {
    articleId: string;
    projectId?: string;
    baseVersion: number;
  }) => Promise<SelectionSnapshot | null>;
  captureViewCheckpoint: () => Promise<EditorViewCheckpoint | null>;
  restoreViewCheckpoint: (checkpoint: EditorViewCheckpoint) => Promise<boolean>;
  /** NEXT-001 性能夹具：光标处注入文本（等价输入派发）；编辑器未就绪返回 false */
  insertTextAtCursor: (text: string) => Promise<boolean>;
}

interface Props {
  /** 用于重建编辑器的唯一键（文章 id） */
  docKey: string;
  /** 初始 Markdown 内容 */
  defaultValue: string;
  /** 预览态保留 EditorState/history，但暂停编辑、快捷键和补全触发。 */
  active?: boolean;
  /** ProseMirror 文档发生真实变化；不携带 Markdown，避免每次按键全文序列化。 */
  onDocumentChange?: () => void;
  /** 编辑器销毁前同步交付最终 Markdown，保证页签卸载也能进入按文档保存队列。 */
  onBeforeDestroy?: (markdown: string, documentId: string) => void;
  /**
   * 切文档时若草稿已入队且编辑器不脏，跳过 destroy 前的全文 getMarkdown，
   * 避免目标篇首帧被上篇序列化拖住。
   */
  skipSnapshotOnDestroy?: boolean;
  /** AG-31：选区浮动条 / ⌘L 显式「Add to Chat」（宿主捕获快照生成 chip） */
  onAddSelectionToChat?: () => void;
  /** AG-32：内联建议块（null/undefined = 隐藏） */
  suggestion?: InlineSuggestion | null;
  onSuggestionDecision?: (target: HunkDecisionTarget, decision: Exclude<HunkDecision, 'pending'>) => void;
}

const fileToDataUrl = (file: File) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(new Error('图片读取失败'));
    reader.readAsDataURL(file);
  });

/**
 * Milkdown Crepe 编辑器（ProseMirror 内核的 Notion 式块编辑体验）。
 * 数据形态始终是纯 Markdown：保存时父组件调 getMarkdown() 落库 content 单字段，
 * 无双格式同步问题。斜杠菜单 / 悬浮工具栏 / 表格 / KaTeX / mermaid 由 Crepe 特性提供。
 */
const MarkdownEditor = forwardRef<MarkdownEditorHandle, Props>(function MarkdownEditor(
  {
    docKey,
    defaultValue,
    active = true,
    onDocumentChange,
    onBeforeDestroy,
    skipSnapshotOnDestroy = false,
    onAddSelectionToChat,
    suggestion = null,
    onSuggestionDecision,
  },
  ref
) {
  const outerRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const crepeRef = useRef<Crepe | null>(null);
  /** NEXT-001：注册进探针表的句柄实例（Crepe 就绪后注册，销毁时注销） */
  const perfHandleRef = useRef<MarkdownEditorHandle | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;
  const suppressDocumentChangeRef = useRef(false);
  const lifecycleCallbacksRef = useRef({ onDocumentChange, onBeforeDestroy, skipSnapshotOnDestroy });
  lifecycleCallbacksRef.current = { onDocumentChange, onBeforeDestroy, skipSnapshotOnDestroy };
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  // AG-31：选区浮动条坐标（相对 outer；null = 隐藏）
  const [toolbar, setToolbar] = useState<{ top: number; left: number } | null>(null);
  // AG-32：Decoration 插件生命周期随 Crepe；回调经 ref 读取宿主最新闭包，
  // 不因审批状态变化重建整个编辑器。
  const suggestionCallbacksRef = useRef({ onSuggestionDecision });
  suggestionCallbacksRef.current = {
    onSuggestionDecision,
  };

  // AG-31：选区非空 → 浮动「Add to Chat ⌘L」。selectionchange 全量覆盖鼠标/键盘选区；
  // 滚动时选区 viewport 坐标变化 → 同函数重算（坐标相对 outer，天然跟随滚动）。
  useEffect(() => {
    const scroller = containerRef.current;
    const outer = outerRef.current;
    if (!scroller || !outer) return;
    const update = () => {
      try {
        const sel = window.getSelection();
        if (!activeRef.current || !onAddSelectionToChat || !sel || sel.isCollapsed || sel.rangeCount === 0) {
          setToolbar(null);
          return;
        }
        const range = sel.getRangeAt(0);
        if (!scroller.isConnected || !scroller.contains(range.commonAncestorContainer)) {
          setToolbar(null);
          return;
        }
        const rect = range.getBoundingClientRect();
        if (!Number.isFinite(rect.top) || (rect.width === 0 && rect.height === 0)) {
          setToolbar(null);
          return;
        }
        const host = outer.getBoundingClientRect();
        const topSpace = rect.top - host.top;
        setToolbar({
          // 顶部空间不足时挂选区下方（首行选区不遮挡）
          top: topSpace > 40 ? topSpace - 34 : rect.bottom - host.top + 6,
          left: Math.min(Math.max(4, rect.left - host.left), Math.max(4, host.width - 132)),
        });
      } catch {
        // selectionchange 可能命中刚卸载的 Range；几何读取失败不应升级成全局致命错误。
        setToolbar(null);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (!onAddSelectionToChat) return;
      if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== 'l') return;
      const sel = window.getSelection();
      if (sel && !sel.isCollapsed && scroller.contains(sel.anchorNode)) {
        e.preventDefault();
        onAddSelectionToChat();
        setToolbar(null);
      }
    };
    document.addEventListener('selectionchange', update);
    scroller.addEventListener('scroll', update, { passive: true });
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('selectionchange', update);
      scroller.removeEventListener('scroll', update);
      document.removeEventListener('keydown', onKey);
    };
  }, [onAddSelectionToChat, active]);

  // 预览不销毁 Crepe：EditorState/history 原位保留，返回编辑后 ⌘Z 仍指向切换前输入。
  // readonly + inert（宿主设置）共同阻断隐藏态输入；补全装配层通过 activeRef 丢弃隐藏态触发。
  useEffect(() => {
    const crepe = crepeRef.current;
    if (!crepe || loading) return;
    crepe.setReadonly(!active);
    if (!active) {
      setToolbar(null);
      try {
        crepe.editor.action((ctx) => {
          const view = ctx.get(editorViewCtx);
          view.dispatch(view.state.tr.setMeta(inlineCompletionKey, { type: 'dismiss' }));
        });
      } catch {
        /* 编辑器正处于销毁边界，隐藏态无需升级错误。 */
      }
    } else {
      // 从不可见态恢复后让 WebView/编辑器重新计算尺寸，不改变 EditorState。
      requestAnimationFrame(() => window.dispatchEvent(new Event('resize')));
    }
  }, [active, loading]);

  // AG-32：提案进入 ProseMirror Decoration；不再查询 DOM 坐标或创建 absolute 浮层。
  // 每个 hunk 是文档流中的 widget，滚动/缩放/长文/多区域修改都由编辑器原生布局承载。
  useEffect(() => {
    const crepe = crepeRef.current;
    if (!crepe || loading) return;
    void (async () => {
      try {
        await crepe.editor.action((ctx) => {
          const view = ctx.get(editorViewCtx);
          const meta: DocumentDiffMeta = suggestion
            ? {
                type: 'show',
                proposal: buildDocumentDiffProposal(crepe.getMarkdown(), suggestion, view.state.doc),
              }
            : { type: 'dismiss' };
          view.dispatch(view.state.tr.setMeta(documentDiffKey, meta));
        });
      } catch (e) {
        console.warn('[notes] document diff decoration failed:', e);
      }
    })();
  }, [suggestion, loading]);

  useImperativeHandle(
    ref,
    () => {
      const handle: MarkdownEditorHandle = {
      getMarkdown: () => crepeRef.current?.getMarkdown() ?? '',
      isReady: () => crepeRef.current != null,
      replaceMarkdown: async (markdown: string, options) => {
        const crepe = crepeRef.current;
        if (!crepe) return false;
        try {
          // 普通外部同步用 flush=true 重建基线；已经由用户批准的一次 AI operation
          // 用单个 replace transaction 进入 ProseMirror history，使一次 ⌘Z 精确撤回整次应用。
          suppressDocumentChangeRef.current = true;
          if (options?.preserveHistory) {
            await crepe.editor.action((ctx) => {
              const view = ctx.get(editorViewCtx);
              const doc = ctx.get(parserCtx)(markdown);
              if (!doc) return;
              const transaction = closeHistory(
                view.state.tr.replace(0, view.state.doc.content.size, new Slice(doc.content, 0, 0))
              );
              view.dispatch(transaction);
              // 再关闭一次刚创建的历史事件，防止随后人工输入在时间窗口内与 AI 应用合并。
              view.dispatch(closeHistory(view.state.tr.setMeta('addToHistory', false)));
            });
          } else {
            await crepe.editor.action(replaceAll(markdown, true));
          }
          return true;
        } catch (e) {
          console.warn('[notes] replaceMarkdown failed:', e);
          return false;
        } finally {
          suppressDocumentChangeRef.current = false;
        }
      },
      scrollToHeadingByIndex: (index: number) => {
        const crepe = crepeRef.current;
        const container = containerRef.current;
        if (!crepe || !container) return;
        try {
          crepe.editor.action((ctx) => {
            const view = ctx.get(editorViewCtx);
            const positions: number[] = [];
            view.state.doc.descendants((node, pos) => {
              if (node.type.name === 'heading') positions.push(pos);
            });
            const targetPos = positions[index];
            if (targetPos == null) return;
            const coords = view.coordsAtPos(targetPos);
            const containerRect = container.getBoundingClientRect();
            // 72px 顶部留白，避免标题贴住容器上沿
            container.scrollTo({
              top: container.scrollTop + (coords.top - containerRect.top) - 72,
              behavior: 'smooth',
            });
          });
        } catch (e) {
          console.warn('[notes] scrollToHeadingByIndex failed:', e);
        }
      },
      captureSelectionSnapshot: async (meta) => {
        const crepe = crepeRef.current;
        if (!crepe) return null;
        try {
          return await crepe.editor.action((ctx) => {
            const view = ctx.get(editorViewCtx);
            const { from, to } = view.state.selection;
            if (from === to) return null;
            // 优先序列化选中 Slice 为 Markdown（同 Milkdown getMarkdown(range) 宏口径：
            // slice(from,to,true) → topNodeType.createAndFill 包成临时 doc → serializer 函数；
            // serializer 是 (content: Node) => string，无 serializeFragment 方法）。
            // 序列化失败/为空时退回纯文本（锚点仍可用，唯命中率略降）
            let selectedMarkdown = '';
            try {
              const schema = ctx.get(schemaCtx);
              const serializer = ctx.get(serializerCtx);
              const slice = view.state.doc.slice(from, to, true);
              const tempDoc = schema.topNodeType.createAndFill(null, slice.content);
              selectedMarkdown = tempDoc
                ? serializer(tempDoc).replace(/\n+$/, '')
                : '';
            } catch {
              selectedMarkdown = '';
            }
            if (!selectedMarkdown.trim()) {
              selectedMarkdown = view.state.doc.textBetween(from, to, '\n');
            }
            return buildSelectionSnapshot({
              articleId: meta.articleId,
              projectId: meta.projectId,
              baseVersion: meta.baseVersion,
              markdown: crepe.getMarkdown(),
              proseFrom: from,
              proseTo: to,
              blockPath: blockPathAt(view.state.doc, from),
              selectedMarkdown,
            });
          });
        } catch (e) {
          console.warn('[notes] captureSelectionSnapshot failed:', e);
          return null;
        }
      },
      captureViewCheckpoint: async () => {
        const crepe = crepeRef.current;
        const container = containerRef.current;
        if (!crepe || !container) return null;
        try {
          return await crepe.editor.action((ctx) => {
            const view = ctx.get(editorViewCtx);
            return {
              anchor: view.state.selection.anchor,
              head: view.state.selection.head,
              scrollTop: container.scrollTop,
              scrollLeft: container.scrollLeft,
              focused: view.hasFocus(),
            };
          });
        } catch {
          return null;
        }
      },
      restoreViewCheckpoint: async (checkpoint) => {
        const crepe = crepeRef.current;
        const container = containerRef.current;
        if (!crepe || !container) return false;
        try {
          await crepe.editor.action((ctx) => {
            const view = ctx.get(editorViewCtx);
            const max = view.state.doc.content.size;
            const anchor = Math.min(Math.max(checkpoint.anchor, 0), max);
            const head = Math.min(Math.max(checkpoint.head, 0), max);
            try {
              view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, anchor, head)));
            } catch {
              // 替换后旧位置可能落在非文本节点边界；滚动仍恢复，选择保持安全默认值。
            }
            if (checkpoint.focused) view.focus();
          });
          requestAnimationFrame(() => requestAnimationFrame(() => {
            container.scrollTo({ top: checkpoint.scrollTop, left: checkpoint.scrollLeft, behavior: 'auto' });
          }));
          return true;
        } catch {
          return false;
        }
      },
      // NEXT-001 性能夹具：ProseMirror insertText 等价输入派发，供输入延迟场景测
      // 「派发→上屏」。走已就绪的 ctx 同步 dispatch，避免 editor.action 在 50KB
      // 文档上挂起 Promise、拖死整份宿主门禁。
      insertTextAtCursor: async (text: string) => {
        const crepe = crepeRef.current;
        if (!crepe || !text) return false;
        try {
          const view = crepe.editor.ctx.get(editorViewCtx);
          const { from } = view.state.selection;
          view.dispatch(view.state.tr.insertText(text, from));
          return true;
        } catch (e) {
          console.warn('[perf] insertTextAtCursor failed:', e);
          return false;
        }
      },
      };
      perfHandleRef.current = handle;
      return handle;
    },
    []
  );

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    // effect 创建时冻结当前文档的销毁回调与 docKey；docKey 切换的 render 会先更新 callback ref，
    // 若 cleanup 再读 ref，可能把旧编辑器 Markdown 误交给新 documentId。
    const deliverBeforeDestroy = lifecycleCallbacksRef.current.onBeforeDestroy;
    const destroyDocKey = docKey;
    // skip 标志在 cleanup 时再读 ref：切文档后父组件可能把「已入队/不脏」更新上来。
    let cancelled = false;
    // NEXT-001：性能夹具探针注册句柄（cleanup 注销）
    let unregisterPerfEditor: (() => void) | null = null;
    setLoading(true);
    setError('');

    const crepe = new Crepe({
      root: el,
      defaultValue,
      featureConfigs: {
        [CrepeFeature.CodeMirror]: {
          languages: editorCodeLanguages,
          searchPlaceholder: '搜索代码语言',
          noResultText: '未找到语言',
        },
        [CrepeFeature.ImageBlock]: {
          // N0：图片落盘 notes/assets/，文档状态只存相对路径 `assets/<name>`；
          // 命令失败时退回 data URL（保旧行为，不丢图）
          onUpload: async (file) => {
            const dataUrl = await fileToDataUrl(file);
            try {
              return await saveNoteAsset(dataUrl);
            } catch (e) {
              console.error('[notes] saveNoteAsset failed, fallback to data URL:', e);
              return dataUrl;
            }
          },
        },
      },
    });

    // 只监听 ProseMirror doc identity，不启用 markdownUpdated：后者会在每次按键后全文
    // 序列化。父级收到事件后做 800ms trailing / 5s max-wait 的有界快照。
    crepe.on((listener) => {
      listener.updated((_ctx, doc, prevDoc) => {
        if (
          prevDoc &&
          doc !== prevDoc &&
          !suppressDocumentChangeRef.current &&
          activeRef.current
        ) {
          lifecycleCallbacksRef.current.onDocumentChange?.();
        }
      });
    });

    // NB-04：补任务清单键位 ⌘⌥9（preset 只给了 `[ ] ` 输入规则与斜杠菜单，无快捷键；
    // 任务清单是笔记核心场景，N5/NB-03 均围绕它）。在 create 前 use 进编辑器
    crepe.addFeature((editor) => {
      editor.use(taskListShortcut);
    });

    // Milkdown 7.22.0 的列表同步插件会把 list 内相对位置当成文档绝对位置，
    // 在正文重载且有序列表不位于首块时可能对 text 节点 setNodeMarkup 并闪退。
    // create 前移除上游实现，换成仅修正位置计算的等价插件。
    crepe.addFeature((editor) => {
      editor.use(safeSyncListOrderPlugin);
    });

    // AG-32：文档内 diff 插件常驻，提案通过事务 meta 显示/隐藏。
    // 回调只读取 ref，审批状态变化不会重建 Crepe 或遗留旧 DOM 监听器。
    crepe.addFeature((editor) => {
      editor.use(
        $prose(() =>
          documentDiffPlugin({
            onDecision: (hunkIndex, decision) =>
              suggestionCallbacksRef.current.onSuggestionDecision?.(hunkIndex, decision),
            onDecisionAll: (decision) =>
              suggestionCallbacksRef.current.onSuggestionDecision?.('all', decision),
          })
        )
      );
    });

    // NB-33：行内补全接入 AG-30 真实 CompletionService（NB-32 Spike 装配层产品化）。
    // 行为开关在设置 completion_config.enabled（Rust 侧按请求读取）；
    // ghost text 仅 Decoration，不进 Markdown/dirty/历史，生命周期跟随本编辑器实例
    crepe.addFeature(inlineCompletionFeature({ articleId: docKey, isActive: () => activeRef.current }));

    // 图片显示解析：文档/序列化保持 `assets/` 相对路径（getMarkdown 干净），
    // 仅把 DOM 层 img.src 换成 data URL。改写后 src 以 data: 开头，不会再命中选择器，天然幂等；
    // 解析失败的打 data-asset-failed 标记防重试风暴。
    const resolveImages = () => {
      const imgs = el.querySelectorAll<HTMLImageElement>(
        'img[src^="assets/"]:not([data-asset-failed]):not([data-asset-resolving])'
      );
      imgs.forEach((img) => {
        const rel = img.getAttribute('src')!;
        img.setAttribute('data-asset-resolving', '1');
        void resolveNoteAsset(rel).then((url) => {
          img.removeAttribute('data-asset-resolving');
          if (!img.isConnected) return;
          if (url) {
            img.src = url;
          } else {
            img.setAttribute('data-asset-failed', '1');
            img.alt = `${img.alt || ''}（图片缺失：${rel}）`.trim();
          }
        });
      });
    };
    const observer = new MutationObserver(resolveImages);

    crepe.editor
      .remove(syncListOrderPlugin)
      .then(() => crepe.create())
      .then(() => {
        if (cancelled) {
          void crepe.destroy();
          return;
        }
        crepeRef.current = crepe;
        setLoading(false);
        // NEXT-001：Crepe 就绪即注册探针（perfRunner 输入场景用）；
        // handle 在 useImperativeHandle 提交期已写入 ref。
        if (perfHandleRef.current) {
          unregisterPerfEditor = registerPerfProbeTarget('editor', perfHandleRef.current);
        }
        resolveImages();
        observer.observe(el, {
          subtree: true,
          childList: true,
          attributes: true,
          attributeFilter: ['src'],
        });
      })
      .catch((e) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setLoading(false);
        }
      });

    return () => {
      cancelled = true;
      observer.disconnect();
      if (!lifecycleCallbacksRef.current.skipSnapshotOnDestroy) {
        try {
          deliverBeforeDestroy?.(crepe.getMarkdown(), destroyDocKey);
        } catch {
          /* 未完成 create 或已销毁时没有可交付快照。 */
        }
      }
      crepeRef.current = null;
      unregisterPerfEditor?.();
      unregisterPerfEditor = null;
      void crepe.destroy().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey]);

  return (
    /* NB-27：高度契约从「h-full 百分比」改为「flex-1 min-h-0 列 flex」。
       宿主要么是列 flex 容器（NoteWorkbench 编辑面板 / ItemDetail 编辑框），
       要么给出显式高度——百分比对 flex 计算出的父高在 WebKit 下解析不稳
       （NB-26 同因），改主轴 flex-1 取高后确定。两个宿主已同步改造。 */
    <div ref={outerRef} className="relative flex min-h-0 flex-1 flex-col">
      {loading && !error && (
        <p className="absolute inset-0 z-10 flex items-center justify-center text-xs text-[var(--text-tertiary)]">
          正在打开编辑器…
        </p>
      )}
      {error && (
        <p className="absolute inset-0 z-10 flex items-center justify-center px-6 text-center text-xs text-[var(--danger)]">
          编辑器初始化失败：{error}
        </p>
      )}
      <div ref={containerRef} className="flex-1 min-h-0 overflow-y-auto" />
      {/* AG-31：选区浮动「Add to Chat ⌘L」（显式捕获入口，替代隐式 focus 捕获） */}
      {toolbar && onAddSelectionToChat && (
        <button
          type="button"
          className="absolute z-30 flex items-center gap-1.5 rounded-md border border-[var(--border-default)] bg-[var(--bg-surface)] px-2.5 py-1.5 text-xs font-medium text-[var(--text-secondary)] shadow-[var(--shadow-md)] hover:bg-[var(--bg-sunken)]"
          style={{ top: toolbar.top, left: toolbar.left }}
          // mousedown 抢先于选区塌陷（click 前先捕获，避免失焦清选区）
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => {
            onAddSelectionToChat();
            setToolbar(null);
          }}
        >
          Add to Chat
          <kbd className="rounded-[6px] bg-[var(--bg-sunken)] px-1 text-xs text-[var(--text-tertiary)]">⌘L</kbd>
        </button>
      )}
    </div>
  );
});

export default MarkdownEditor;
