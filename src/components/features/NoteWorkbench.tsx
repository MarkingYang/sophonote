import {
  forwardRef,
  startTransition,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { useShallow } from 'zustand/react/shallow';
import MarkdownView from './MarkdownView';
import MarkdownEditor, { type InlineSuggestion, type MarkdownEditorHandle } from '../editor/MarkdownEditor';
import type { HunkDecision, HunkDecisionTarget } from '../../services/changeSession';
import { useAppStore } from '../../stores/appStore';
import { scanTaskLines, toggleTaskLine } from '../../services/noteTasks';
import { findUnlinkedMentions, linkifyMention, containsWikilinkTo } from '../../services/noteLinks';
import { perfTime, perfMark } from '../../services/notePerf';
import { registerPerfProbeTarget } from '../../services/perfProbeRegistry';
import { usePageSurfaceActive } from '../layout/KeptAlivePage';
import { extractOutline } from '../../services/noteOutline';
import { messageOf } from '../../services/noteSave';
import {
  documentDraftQueue,
  type DocumentDraftSnapshot,
} from '../../services/documentDraftQueue';
import type { Article } from '../../types';
import {
  Pencil,
  Eye,
  Columns2,
  Check,
  Loader2,
  AlertCircle,
  Trash2,
  Link2,
  AtSign,
  ListTree,
  ChevronDown,
  MoreHorizontal,
} from 'lucide-react';

type Mode = 'edit' | 'split' | 'preview';

const SAVE_DEBOUNCE_MS = 800;
const SAVE_MAX_WAIT_MS = 5000;
const SPLIT_PREVIEW_DEBOUNCE_MS = 400;

const draftWriters = {
  writeContent: (documentId: string, markdown: string) =>
    useAppStore.getState().updateArticleContent(documentId, markdown),
  writeTitle: (documentId: string, title: string) =>
    useAppStore.getState().updateArticleTitle(documentId, title),
};

type MentionGroup = {
  doc: Article;
  hits: ReturnType<typeof findUnlinkedMentions>;
};

interface RelationshipSnapshot {
  articleId: string;
  backlinks: Article[];
  mentions: MentionGroup[];
}

export interface NoteWorkbenchMenuItem {
  id: string;
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
}

/**
 * NB-11 分屏渲染探针：liveMd 变化引发的 React 提交完成后（useLayoutEffect 时机）
 * 落点「快照采集 → DOM 提交」全链路耗时。起点由分屏快照定时器在 setLiveMd 前写入 startRef；
 * md 未变不触发重渲染、不产点，无噪音。
 */
function SplitRenderProbe({ md, startRef }: { md: string; startRef: { current: number } }) {
  useLayoutEffect(() => {
    if (startRef.current > 0) {
      perfMark('分屏·更新全链路', performance.now() - startRef.current);
      startRef.current = 0;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [md]);
  return null;
}

/**
 * NB-23：单文档书写工作台（共享唯一实现，禁止复制）——
 * 笔记本（DocWorkspace）与 AI 工作室项目模式（ProjectMode）嵌入同一组件，
 * 编辑/预览/双链/目录等功能两空间完全一致，两边数据天然同步（同一份 articles 记录、
 * 同一条写路径 updateArticleContent/updateArticleTitle）。原 DocWorkspace 内联工作台整体搬移而来。
 *
 * 宿主职责（工作台不关心文档列表/导航形态）：
 * - onOpenDocument：双链/反链跳转已有文档（heading 为标题链接目标段，如有；blockId 为块引用目标块，如有）
 * - onCreateDocument：[[双链]] 指向不存在标题 → 新建笔记（笔记本进列表；项目归入当前项目）
 * - onDelete：⋯ 菜单两步确认后的删除动作
 * - 各插槽：笔记本注入快速捕获栏/更多菜单/性能探针；项目注入类型徽标与移出按钮
 */
export interface NoteWorkbenchHandle {
  /** 落盘未保存内容（切换/离开前的主保障；destroy 后的编辑器经 isReady 守卫自动跳过） */
  flush: () => Promise<boolean>;
  /** 当前文档是否有未入队/未落盘的脏草稿——干净时导航可跳过 flush */
  needsFlush: () => boolean;
  /** 编辑/分屏态取编辑器实时内容；预览态取基线（N2 捕获等外部写入的基线来源） */
  getLiveMarkdown: () => string;
  /** 外部写库后同步基线 + 原位替换编辑器正文（防心跳用旧快照覆盖） */
  applyExternalContent: (md: string, options?: { addToHistory?: boolean }) => void;
  /** 进入编辑态（新建文档后宿主调用，配合 focusTitle 引导命名） */
  enterEdit: () => void;
  /** 回到预览态（⌘K 锚点跳转同文档时确保 hb-line-N 锚点 DOM 存在） */
  enterPreview: () => void;
  /** 聚焦标题输入框并全选 */
  focusTitle: () => void;
  /** AG-26：转发编辑器选区快照捕获（ProjectChatPanel 范围 chip 的准入）。
   *  尚未进入过编辑态或无有效选区 → null。签名与 MarkdownEditorHandle 同源，不重复声明 */
  captureSelectionSnapshot: MarkdownEditorHandle['captureSelectionSnapshot'];
  captureViewCheckpoint: MarkdownEditorHandle['captureViewCheckpoint'];
  restoreViewCheckpoint: MarkdownEditorHandle['restoreViewCheckpoint'];
}

interface NoteWorkbenchProps {
  article: Article;
  /** [[双链]]/反链/痕迹跳转：目标是已有文档。heading = 标题链接目标段；blockId = 块引用目标块（NB-29） */
  onOpenDocument: (doc: Article, heading?: string, blockId?: string) => void;
  /** [[双链]] 指向不存在的标题 → 新建笔记（落点由宿主决定） */
  onCreateDocument: (title: string) => void;
  /** ⋯ 菜单两步确认删除的第二次点击；不传则菜单无删除项 */
  onDelete?: () => void;
  /** 顶栏之下插槽（N2 快速捕获栏） */
  belowHeader?: ReactNode;
  /** 底部状态栏附加项（快捷键卡片 / 性能探针开关由宿主自管） */
  statusExtra?: ReactNode;
  /** 顶栏右端附加元素（项目：类型徽标 + ×移出） */
  headerExtra?: ReactNode;
  /** 顶栏 ⋯ 菜单附加项（笔记本：今日痕迹入口） */
  moreMenuItems?: NoteWorkbenchMenuItem[];
  /** AG-31：选区「Add to Chat ⌘L」→ 宿主捕获快照生成 chip（未传则编辑器不显示浮动条） */
  onAddSelectionToChat?: () => void;
  /** AG-32：内联建议块（project chat 的 propose_document_patch 结果；null = 隐藏） */
  suggestion?: InlineSuggestion | null;
  onSuggestionDecision?: (target: HunkDecisionTarget, decision: Exclude<HunkDecision, 'pending'>) => void;
}

const NoteWorkbench = forwardRef<NoteWorkbenchHandle, NoteWorkbenchProps>(function NoteWorkbench(
  {
    article,
    onOpenDocument,
    onCreateDocument,
    onDelete,
    belowHeader,
    statusExtra,
    headerExtra,
    moreMenuItems = [],
    onAddSelectionToChat,
    suggestion,
    onSuggestionDecision,
  },
  ref
) {
  const { articles, updateArticleContent, openArticleAtLine, setSelectedItemId } = useAppStore(
    useShallow((state) => ({
      articles: state.articles,
      updateArticleContent: state.updateArticleContent,
      openArticleAtLine: state.openArticleAtLine,
      setSelectedItemId: state.setSelectedItemId,
    }))
  );

  const pageActive = usePageSurfaceActive();
  const pageActiveRef = useRef(pageActive);
  pageActiveRef.current = pageActive;

  // 模式与文章 ID 绑定。文章刚切换的首帧同步视为 preview，防止上一文章的 edit
  // 状态让新 Crepe 实例先创建、随后又被 effect 销毁。
  const currentArticleIdRef = useRef(article.id);
  currentArticleIdRef.current = article.id;
  const [modeState, setModeState] = useState<{ articleId: string; value: Mode }>({
    articleId: article.id,
    value: 'preview',
  });
  // 纯预览文档不提前创建重型 Crepe；本篇一旦进入过编辑态，预览时只隐藏不销毁，保留 undo。
  // 切走时延后清空 editorArticleId，避免与目标篇 lite 首帧同 commit 同步 destroy。
  const [editorArticleId, setEditorArticleId] = useState<string | null>(null);
  const editorArticleIdRef = useRef<string | null>(null);
  editorArticleIdRef.current = editorArticleId;
  const skipDestroySnapshotRef = useRef(true);
  const previewGenRef = useRef(0);
  const mode = modeState.articleId === article.id ? modeState.value : 'preview';
  const setMode = useCallback((value: Mode) => {
    if (value !== 'preview') setEditorArticleId(currentArticleIdRef.current);
    setModeState({ articleId: currentArticleIdRef.current, value });
  }, []);
  // 仍挂着上篇实例时保持 mounted，仅在归属当前篇且非预览时展示交互面。
  const editorMounted = editorArticleId != null;
  const editorIsCurrent = editorArticleId === article.id;
  const seededDraft = documentDraftQueue.seed({
    documentId: article.id,
    markdown: article.content ?? '',
    title: article.title,
  });
  const [draftTitle, setDraftTitle] = useState(seededDraft.title);
  const [liveMd, setLiveMd] = useState('');
  // 预览正文：切文档时与 article.id 同步对齐，避免骨架空窗（原先 rAF+setTimeout 会造成「像加载失败」的卡顿感）
  const [previewSource, setPreviewSource] = useState({
    articleId: article.id,
    content: seededDraft.markdown,
  });
  const [relationships, setRelationships] = useState<RelationshipSnapshot>({
    articleId: '',
    backlinks: [],
    mentions: [],
  });
  const splitRenderStartRef = useRef(0); // NB-11：分屏「快照→DOM 提交」全链路起点
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [moreMenuOpen, setMoreMenuOpen] = useState(false); // 顶栏 ⋯ 更多菜单（分屏/删除）
  const [outlineOpen, setOutlineOpen] = useState(true);
  const [activeLine, setActiveLine] = useState<number | null>(null); // NB-05：scrollspy 当前所在标题行
  const [editorBump, setEditorBump] = useState(0); // 外部写回后强制编辑器 remount，避免心跳用旧内容覆盖
  // 切文档 / 首次打开：先轻量纯文本，idle 后再跑 ReactMarkdown/KaTeX/highlight。
  // 与 mode 相同：articleId 未对齐时强制 lite，避免首帧沿用上篇 rich。
  const [previewPhaseState, setPreviewPhaseState] = useState<{ articleId: string; value: 'lite' | 'rich' }>({
    articleId: article.id,
    value: 'lite',
  });
  const previewPhase = previewPhaseState.articleId === article.id ? previewPhaseState.value : 'lite';
  const setPreviewPhase = useCallback((value: 'lite' | 'rich') => {
    setPreviewPhaseState({ articleId: currentArticleIdRef.current, value });
  }, []);
  const editorRef = useRef<MarkdownEditorHandle>(null);
  /** NEXT-001：性能夹具探针句柄（useImperativeHandle 提交期写入，mount effect 注册） */
  const perfWbHandleRef = useRef<NoteWorkbenchHandle | null>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);
  const previewScrollRef = useRef<HTMLDivElement>(null); // NB-05：预览滚动容器（大纲 scrollspy 与锚点跳转的作用域）
  const editorDirtyRef = useRef(false);
  const saveTimerRef = useRef<number | null>(null);
  const maxWaitTimerRef = useRef<number | null>(null);
  const previewTimerRef = useRef<number | null>(null);
  const relationshipInputsRef = useRef<{ title: string; candidates: Article[] } | null>(null);

  // 外部正文/标题变更：同步预览基线（dirty 草稿由 queue.seed 守卫，不被干净写回覆盖）
  useEffect(() => {
    const draft = documentDraftQueue.seed({
      documentId: article.id,
      markdown: article.content ?? '',
      title: article.title,
    });
    setPreviewSource({ articleId: article.id, content: draft.markdown });
  }, [article.id, article.content, article.title]);

  // 反链与未链接提及原本在文章切换 render 内同步全库扫描（O(篇数 × 文长)）。
  // 移到空闲片段执行，文章内容先出现，关系面板随后补齐；当前文档自动保存只会替换
  // articles 中自身对象，其余候选引用未变时跳过扫描，避免每次落盘重复全文正则。
  // 页签保活隐藏时不扫描，避免 hidden 子树 mutation 拖住其它页的 settle。
  useEffect(() => {
    if (!pageActive) return;
    const title = article.title.trim();
    // 「未命名文档」与过短标题全库子串命中极多，跳过未链接提及扫描以免顶主线程。
    const skipMentions = title === '未命名文档' || title.length < 4;
    const candidates = articles.filter((candidate) => candidate.id !== article.id);
    const previous = relationshipInputsRef.current;
    if (
      previous?.title === article.title &&
      previous.candidates.length === candidates.length &&
      previous.candidates.every((candidate, index) => candidate === candidates[index])
    ) return;
    relationshipInputsRef.current = { title: article.title, candidates };

    const scan = () => {
      const backlinks = candidates.filter((candidate) => containsWikilinkTo(candidate.content, article.title));
      const mentions = skipMentions
        ? []
        : perfTime('提及·全库扫描', () =>
            candidates
              .map((candidate) => ({
                doc: candidate,
                hits: findUnlinkedMentions(candidate.content, article.title),
              }))
              .filter((group) => group.hits.length > 0)
          );
      setRelationships({ articleId: article.id, backlinks, mentions });
    };
    let idleId: number | null = null;
    let timerId: number | null = null;
    if ('requestIdleCallback' in window) idleId = window.requestIdleCallback(scan, { timeout: 500 });
    else timerId = globalThis.setTimeout(scan, 0);
    return () => {
      if (idleId != null && 'cancelIdleCallback' in window) window.cancelIdleCallback(idleId);
      if (timerId != null) window.clearTimeout(timerId);
    };
  }, [articles, article.id, article.title, pageActive]);

  // 最新值实时镜像到 ref：心跳 / 快捷键 / 卸载 flush / handle 共用，避免闭包过期
  const stateRef = useRef({ article, mode, draftTitle });
  stateRef.current = { article, mode, draftTitle };
  const modeRef = useRef(mode);
  modeRef.current = mode;
  const flushRef = useRef<() => Promise<boolean>>(async () => true);
  // 导航回调经 ref 转发：⌘S/⌘E 等常驻监听与 openArticleByTitle 不随宿主闭包重建
  const onOpenDocumentRef = useRef(onOpenDocument);
  onOpenDocumentRef.current = onOpenDocument;
  const onCreateDocumentRef = useRef(onCreateDocument);
  onCreateDocumentRef.current = onCreateDocument;

  const timeNow = () => new Date().toLocaleTimeString('zh-CN', { hour12: false });

  const clearSaveTimers = () => {
    if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
    if (maxWaitTimerRef.current != null) window.clearTimeout(maxWaitTimerRef.current);
    saveTimerRef.current = null;
    maxWaitTimerRef.current = null;
  };

  const captureDraft = (markdownOverride?: string): DocumentDraftSnapshot => {
    const art = stateRef.current.article;
    const existing = documentDraftQueue.seed({
      documentId: art.id,
      markdown: art.content ?? '',
      title: art.title,
    });
    const markdown = markdownOverride ?? (
      editorRef.current?.isReady()
        ? perfTime('草稿·序列化', () => editorRef.current!.getMarkdown())
        : existing.markdown
    );
    editorDirtyRef.current = false;
    return documentDraftQueue.update({
      documentId: art.id,
      markdown,
      title: stateRef.current.draftTitle.trim() || '未命名文档',
    });
  };

  const persistDraft = async (snapshot: DocumentDraftSnapshot): Promise<boolean> => {
    const { documentId } = snapshot;
    if (currentArticleIdRef.current === documentId && snapshot.dirty) {
      setSaving(true);
      setSaveError(null);
    }
    const ok = await documentDraftQueue.flush(documentId, draftWriters);
    const latest = documentDraftQueue.get(documentId);
    if (currentArticleIdRef.current === documentId) {
      setSaving(false);
      setDirty(editorDirtyRef.current || !!latest?.dirty);
      setSaveError(latest?.error?.slice(0, 60) ?? null);
      if (ok && snapshot.dirty && !latest?.dirty) setSavedAt(timeNow());
    }
    return ok;
  };

  // getMarkdown 只在输入停止、max-wait、显式保存或导航快照时执行；同文档保存由全局
  // DocumentDraftQueue 串行合并，不同文档的基线/inFlight 完全隔离。
  // 导航切换：干净文档跳过序列化，避免每次点文档都卡主线程。
  const flushSave = (): Promise<boolean> => {
    clearSaveTimers();
    const art = stateRef.current.article;
    const queued = documentDraftQueue.get(art.id);
    if (!editorDirtyRef.current && !queued?.dirty && !queued?.saving) {
      return Promise.resolve(true);
    }
    return persistDraft(captureDraft());
  };
  flushRef.current = flushSave;

  // 切换文档时重置草稿态（回到预览 = 笔记本 selectArticle 语义；宿主新建流再经 handle.enterEdit 进编辑）
  useEffect(() => {
    clearSaveTimers();
    if (previewTimerRef.current != null) window.clearTimeout(previewTimerRef.current);
    previewTimerRef.current = null;
    const leavingId = editorArticleIdRef.current;
    if (leavingId && leavingId !== article.id) {
      const queued = documentDraftQueue.get(leavingId);
      // 脏草稿应已由点击路径 flush 入队；不脏则 destroy 跳过 getMarkdown。
      skipDestroySnapshotRef.current = !editorDirtyRef.current && !queued?.dirty;
    } else {
      skipDestroySnapshotRef.current = true;
    }
    editorDirtyRef.current = false;
    const draft = documentDraftQueue.seed({
      documentId: article.id,
      markdown: article.content ?? '',
      title: article.title,
    });
    setDraftTitle(draft.title);
    setDirty(draft.dirty);
    setSavedAt(null);
    setSaveError(draft.error?.slice(0, 60) ?? null);
    setConfirmDelete(false);
    setPreviewSource({ articleId: article.id, content: draft.markdown });
    setPreviewPhase('lite');
    setMode('preview');
    setActiveLine(null);
    // 延后卸上篇 Crepe，让目标篇 lite 先 paint
    const targetId = article.id;
    const raf = window.requestAnimationFrame(() => {
      setEditorArticleId((current) => (current != null && current !== targetId ? null : current));
    });
    return () => window.cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [article.id]);

  // 切文档后的下一空闲帧再升级富文本预览（体量越大越延后，减轻切走长文时的卸载成本）。
  // 保活隐藏时不 promote，避免 ReactMarkdown/KaTeX 在 hidden 子树里拖住其它页签 settle。
  useEffect(() => {
    if (!pageActive) return;
    if (previewPhase !== 'lite') return;
    const gen = ++previewGenRef.current;
    const mdLen =
      previewSource.articleId === article.id
        ? previewSource.content.length
        : (article.content ?? '').length;
    const timeoutMs = mdLen < 8_000 ? 120 : mdLen < 48_000 ? 400 : 800;
    let idleId: number | null = null;
    let timerId: number | null = null;
    const promote = () => {
      if (previewGenRef.current !== gen) return;
      startTransition(() => {
        if (previewGenRef.current !== gen) return;
        setPreviewPhase('rich');
      });
    };
    const ric = (window as Window & {
      requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number;
      cancelIdleCallback?: (id: number) => void;
    }).requestIdleCallback;
    if (typeof ric === 'function') {
      idleId = ric(promote, { timeout: timeoutMs });
    } else {
      timerId = window.setTimeout(promote, timeoutMs);
    }
    return () => {
      previewGenRef.current += 1; // 取消在途 promote
      const cic = (window as Window & { cancelIdleCallback?: (id: number) => void }).cancelIdleCallback;
      if (idleId != null && typeof cic === 'function') cic(idleId);
      if (timerId != null) window.clearTimeout(timerId);
    };
  }, [article.id, previewPhase, previewSource.articleId, previewSource.content, article.content, pageActive]);

  // 外部改名（列表行内重命名 / 其它空间同源改名）同步标题草稿，防 flush 用陈旧草稿覆盖
  useEffect(() => {
    const draft = documentDraftQueue.seed({
      documentId: article.id,
      markdown: article.content ?? '',
      title: article.title,
    });
    if (!draft.dirty) setDraftTitle(draft.title);
  }, [article.id, article.title]);

  const scheduleDraftWork = () => {
    editorDirtyRef.current = true;
    setDirty(true);
    setSaveError(null);

    if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      if (maxWaitTimerRef.current != null) window.clearTimeout(maxWaitTimerRef.current);
      maxWaitTimerRef.current = null;
      perfMark('事件驱动·写库触发', 0);
      void flushRef.current();
    }, SAVE_DEBOUNCE_MS);

    if (maxWaitTimerRef.current == null) {
      maxWaitTimerRef.current = window.setTimeout(() => {
        maxWaitTimerRef.current = null;
        if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
        saveTimerRef.current = null;
        perfMark('事件驱动·max-wait', 0);
        void flushRef.current();
      }, SAVE_MAX_WAIT_MS);
    }

    if (modeRef.current === 'split') {
      if (previewTimerRef.current != null) window.clearTimeout(previewTimerRef.current);
      previewTimerRef.current = window.setTimeout(() => {
        previewTimerRef.current = null;
        const snapshot = captureDraft();
        splitRenderStartRef.current = performance.now();
        setLiveMd((prev) => (prev === snapshot.markdown ? prev : snapshot.markdown));
      }, SPLIT_PREVIEW_DEBOUNCE_MS);
    }
  };

  // 模式切换即时完成：预览使用同步捕获的草稿，持久化在按文档队列后台继续。
  // 编辑器实例不卸载，因此 Edit → Preview → Edit 不会清空 ProseMirror history。
  // 进入编辑/分屏用 startTransition，让顶栏高亮先响应，再挂载重型 Crepe。
  const switchMode = (m: Mode) => {
    if (m === mode) return;
    if (m === 'preview') {
      const snapshot = captureDraft();
      setPreviewSource({ articleId: snapshot.documentId, content: snapshot.markdown });
      void persistDraft(snapshot);
      setMode(m);
      return;
    }
    if (m === 'split') {
      const snapshot = captureDraft();
      setLiveMd(snapshot.markdown);
    }
    startTransition(() => setMode(m));
  };

  const toggleMode = () => {
    if (stateRef.current.mode === 'preview') {
      startTransition(() => setMode('edit'));
    } else {
      const snapshot = captureDraft();
      setPreviewSource({ articleId: snapshot.documentId, content: snapshot.markdown });
      void persistDraft(snapshot);
      setMode('preview');
    }
  };
  const toggleModeRef = useRef(toggleMode);
  toggleModeRef.current = toggleMode;

  // ⌘S / Ctrl+S 立即落盘；⌘E / Ctrl+E 切换编辑↔预览（Obsidian 肌肉记忆）
  // NB-04：改捕获阶段拦截并 stopPropagation——Milkdown preset 的 ⌘E 是内联代码，
  // 冒泡阶段两者会先后触发（插入反引号 + 切模式）；⌘E 固定为模式切换（与状态栏公示一致），
  // 内联代码保留工具栏/斜杠菜单入口
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!pageActiveRef.current) return;
      if (!(e.metaKey || e.ctrlKey)) return;
      const k = e.key.toLowerCase();
      if (k !== 's' && k !== 'e') return;
      e.preventDefault();
      e.stopPropagation();
      if (k === 's') void flushRef.current();
      else toggleModeRef.current();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, []);

  // 离开页面兜底落盘；MarkdownEditor 自身还会在 destroy 前同步交付最终 Markdown。
  useEffect(() => () => {
    clearSaveTimers();
    if (previewTimerRef.current != null) window.clearTimeout(previewTimerRef.current);
    previewTimerRef.current = null;
    void flushRef.current();
  }, []);
  useEffect(() => {
    const onBeforeUnload = () => {
      void flushRef.current();
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, []);

  useImperativeHandle(
    ref,
    () => {
      const handle: NoteWorkbenchHandle = {
      flush: () => flushRef.current(),
      needsFlush: () => {
        const art = stateRef.current.article;
        const queued = documentDraftQueue.get(art.id);
        return editorDirtyRef.current || !!queued?.dirty || !!queued?.saving;
      },
      getLiveMarkdown: () =>
        editorRef.current?.isReady()
          ? editorRef.current.getMarkdown()
          : documentDraftQueue.get(stateRef.current.article.id)?.markdown ?? stateRef.current.article.content,
      applyExternalContent: (md: string, options) => {
        const art = stateRef.current.article;
        documentDraftQueue.markPersisted({
          documentId: art.id,
          markdown: md,
          title: stateRef.current.draftTitle.trim() || art.title,
        });
        editorDirtyRef.current = false;
        setDirty(false);
        setSavedAt(timeNow());
        setLiveMd(md); // 分屏态预览立即反映
        if (modeRef.current !== 'preview') {
          void editorRef.current?.replaceMarkdown(md, { preserveHistory: options?.addToHistory }).then((replaced) => {
            if (!replaced) setEditorBump((v) => v + 1);
          });
        }
      },
      enterEdit: () => startTransition(() => setMode('edit')),
      enterPreview: () => {
        const snapshot = captureDraft();
        setPreviewSource({ articleId: snapshot.documentId, content: snapshot.markdown });
        void persistDraft(snapshot);
        setMode('preview');
      },
      focusTitle: () => {
        titleInputRef.current?.focus();
        titleInputRef.current?.select();
      },
      // AG-26：尚未创建编辑器 → null；编辑后切到预览时实例仍保留但 readonly/inert。
      captureSelectionSnapshot: (meta) =>
        editorRef.current
          ? editorRef.current.captureSelectionSnapshot(meta)
          : Promise.resolve(null),
      captureViewCheckpoint: () =>
        editorRef.current
          ? editorRef.current.captureViewCheckpoint()
          : Promise.resolve(null),
      restoreViewCheckpoint: (checkpoint) =>
        editorRef.current
          ? editorRef.current.restoreViewCheckpoint(checkpoint)
          : Promise.resolve(false),
      };
      perfWbHandleRef.current = handle;
      return handle;
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );

  // NEXT-001：注册性能夹具探针（perfRunner 的 A/B/输入场景从这里取 enterEdit/编辑器句柄）
  useEffect(() => {
    const handle = perfWbHandleRef.current;
    if (!handle) return;
    return registerPerfProbeTarget('workbench', handle);
  }, []);

  // [[双链]] 点击：存在则跳转（跨空间），不存在则按标题新建笔记
  // NB-10：heading 为标题链接（[[笔记#标题]]）目标段，交给宿主定位；创建分支忽略 heading
  const openArticleByTitle = useCallback((title: string, heading?: string, blockId?: string) => {
    const target = articles.find((a) => a.title === title);
    // flush 会在返回 Promise 前同步捕获当前草稿，后端写入可在新文档首帧之后完成。
    void flushRef.current();
    if (target) {
      onOpenDocumentRef.current(target, heading, blockId);
      return;
    }
    onCreateDocumentRef.current(title);
  }, [articles]);

  const backlinks = relationships.articleId === article.id ? relationships.backlinks : [];
  const mentions = relationships.articleId === article.id ? relationships.mentions : [];
  const mentionCount = mentions.reduce((n, x) => n + x.hits.length, 0);

  // 切文档首帧：previewSource 可能尚属上一篇，立刻用 seed，杜绝骨架空窗
  const previewMd =
    mode === 'split'
      ? liveMd
      : previewSource.articleId === article.id
        ? previewSource.content
        : seededDraft.markdown;
  const charCount = useMemo(() => previewMd.replace(/\s/g, '').length, [previewMd]);
  const outline = useMemo(
    () => (previewPhase === 'rich' ? extractOutline(previewMd) : []),
    [previewMd, previewPhase]
  );

  const statusNode = saving ? (
    <span className="flex items-center gap-1 text-[12px] text-[var(--text-tertiary)] shrink-0">
      <Loader2 size={11} className="animate-spin" /> 保存中…
    </span>
  ) : saveError ? (
    <span className="flex items-center gap-1 text-[12px] text-[var(--danger)] shrink-0" title={saveError}>
      <AlertCircle size={11} /> 保存失败
    </span>
  ) : dirty ? (
    <span className="flex items-center gap-1.5 text-[12px] text-[var(--warning)] shrink-0">
      <span className="w-1.5 h-1.5 rounded-full bg-[var(--warning)]" /> 未保存更改
    </span>
  ) : savedAt ? (
    <span className="flex items-center gap-1 text-[12px] text-[var(--success)] shrink-0">
      <Check size={11} /> 已保存 {savedAt}
    </span>
  ) : (
    <span className="text-[12px] text-[var(--text-tertiary)] shrink-0">自动保存已开启</span>
  );

  const renderBacklinks = () =>
    backlinks.length > 0 && (
      <div className="mt-8 pt-4 border-t border-[var(--border-default)]">
        <p className="flex items-center gap-1.5 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider mb-2">
          <Link2 size={12} /> 反向链接 · {backlinks.length}
        </p>
        <div className="space-y-1.5">
          {backlinks.map((b) => (
            <button
              key={b.id}
              onClick={() => onOpenDocumentRef.current(b)}
              className="w-full text-left px-3 py-2 rounded-[var(--radius-md)] border border-[var(--border-default)] bg-[var(--bg-surface)] hover:border-[var(--accent-border)] transition-colors"
            >
              <p className="text-xs font-medium text-[var(--text-secondary)]">{b.title}</p>
              <p className="text-[12px] text-[var(--text-tertiary)] line-clamp-1 mt-0.5">
                {b.content.replace(/\s+/g, ' ').slice(0, 80)}
              </p>
            </button>
          ))}
        </div>
      </div>
    );

  // NB-06 一键转链：把目标文档第 ordinal 个裸提及包成 [[规范标题]]。
  // 写规范标题而非原文变体——双链解析（openArticleByTitle）按标题精确匹配，变体会成死链。
  // 目标文档此刻未挂在编辑器（同工作台仅选中文档挂载），无心跳冲突；写后 mentions 自动重算
  // NB-31：转链写失败不再静默——状态栏给出可见错误；内存已乐观更新，
  // 目标文档下次挂载并成功落盘时随之持久化
  const handleConvertMention = async (docId: string, ordinal: number) => {
    const doc = articles.find((a) => a.id === docId);
    if (!doc) return;
    const next = linkifyMention(doc.content, article.title, ordinal);
    if (next === doc.content) return;
    try {
      await updateArticleContent(docId, next);
    } catch (e) {
      setSaveError(`转链失败：${messageOf(e)}`.slice(0, 60));
    }
  };

  // NB-06 未链接提及面板（预览态，紧随反链）：上下文行可跳回来源行，单条一键转 [[链接]]
  const renderMentions = () =>
    mentionCount > 0 && (
      <div className="mt-4 pt-4 border-t border-[var(--border-default)]">
        <p className="flex items-center gap-1.5 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider mb-2">
          <AtSign size={12} /> 提及未链接 · {mentionCount}
        </p>
        <div className="space-y-1.5">
          {mentions.map(({ doc, hits }) => (
            <div
              key={doc.id}
              className="px-3 py-2 rounded-[var(--radius-md)] border border-[var(--border-default)] bg-[var(--bg-surface)]"
            >
              <p className="text-xs font-medium text-[var(--text-secondary)]">{doc.title}</p>
              <div className="mt-1 space-y-1">
                {hits.map((h) => (
                  <div key={h.ordinal} className="flex items-center gap-2">
                    <button
                      onClick={() => openArticleAtLine(doc.id, h.line)}
                      title="跳转到提及处"
                      className="flex-1 min-w-0 text-left text-[12px] text-[var(--text-tertiary)] truncate hover:text-[var(--accent)] transition-colors"
                    >
                      <span className="font-mono text-[12px] px-1 rounded-[6px] bg-[var(--bg-sunken)] text-[var(--text-tertiary)] mr-1">L{h.line}</span>
                      {h.snippet}
                    </button>
                    <button
                      onClick={() => void handleConvertMention(doc.id, h.ordinal)}
                      title={`转为 [[${article.title}]] 双链`}
                      className="flex items-center gap-0.5 text-[12px] px-1.5 py-0.5 rounded-[6px] border border-[var(--border-default)] text-[var(--text-tertiary)] hover:border-[var(--accent-border)] hover:text-[var(--accent)] transition-colors shrink-0"
                    >
                      <Link2 size={9} /> 转链
                    </button>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </div>
    );

  // NB-05：锚点跳转限定在预览容器内，且排除嵌入卡片（![[转引]] 内的标题也带 hb-line-N id，
  // 全局 getElementById 可能命中嵌入副本而非本文标题）
  const scrollToHeading = (line: number) => {
    const scope = previewScrollRef.current;
    if (!scope) {
      document.getElementById(`hb-line-${line}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      return;
    }
    const el = Array.from(scope.querySelectorAll(`[id="hb-line-${line}"]`)).find((n) => !n.closest('.md-embed'));
    el?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  };

  // NB-05：大纲跳转分发——预览/分屏走 DOM 锚点；编辑态 DOM 是 ProseMirror（无 hb-line 锚点），
  // 走编辑器 handle 按「文档顺序第 index 个标题」滚动（两侧序号语义一致，见 MarkdownEditor 注释）
  const handleOutlineJump = (line: number, index: number) => {
    if (mode === 'edit') editorRef.current?.scrollToHeadingByIndex(index);
    else scrollToHeading(line);
  };

  // NB-05：scrollspy——滚动预览容器时定位「视口上方最近的标题」，高亮大纲当前项。
  // 仅预览/分屏态（编辑态无锚点 DOM，定位交给 ProseMirror 跳转，高亮暂缺省）
  useEffect(() => {
    if (mode === 'edit') {
      setActiveLine(null);
      return;
    }
    const el = previewScrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const containerTop = el.getBoundingClientRect().top;
      const headings = Array.from(el.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')).filter(
        (h) => h.id.startsWith('hb-line-') && !h.closest('.md-embed')
      );
      let current: number | null = null;
      for (const h of headings) {
        if (h.getBoundingClientRect().top - containerTop <= 96) {
          current = Number(h.id.slice('hb-line-'.length));
        } else break;
      }
      setActiveLine(current);
    };
    onScroll();
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => el.removeEventListener('scroll', onScroll);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, article.id, previewMd]);

  // 预览态任务勾选/嵌入编辑写回：直接落库（预览态编辑器未挂载，无心跳冲突）；
  // 编辑态不传勾选通道（Crepe 原生支持编辑态勾选，经心跳自然落库）
  // NB-31：先写盘成功再推进基线；失败保留 dirty 并展示错误，防"假保存"
  const handlePreviewContentChange = useCallback(async (newMd: string) => {
    try {
      await updateArticleContent(article.id, newMd);
    } catch (e) {
      setDirty(true);
      setSaveError(messageOf(e).slice(0, 60));
      return;
    }
    documentDraftQueue.markPersisted({
      documentId: article.id,
      markdown: newMd,
      title: draftTitle.trim() || article.title,
    });
    editorDirtyRef.current = false;
    setDirty(false);
    setSavedAt(timeNow());
  }, [article.id, updateArticleContent]);

  // NB-03：分屏态预览勾选——预览快照可能滞后编辑器 ≤400ms，故基线取编辑器实时内容
  // （同 N2 快速捕获防心跳覆盖方案）：序号→行号在最新内容上重定位，写库后原位替换编辑器正文
  // NB-31：写盘失败则不替换、不推进基线——编辑器与磁盘保持一致的未勾选态，错误可见可重试
  const handleToggleInSplit = useCallback(async (taskOrdinal: number) => {
    const base = editorRef.current?.isReady() ? editorRef.current.getMarkdown() : article.content;
    const line = scanTaskLines(base)[taskOrdinal];
    if (line == null) return;
    const newMd = toggleTaskLine(base, line);
    if (newMd === base) return;
    try {
      await updateArticleContent(article.id, newMd);
    } catch (e) {
      setDirty(true);
      setSaveError(messageOf(e).slice(0, 60));
      return;
    }
    documentDraftQueue.markPersisted({
      documentId: article.id,
      markdown: newMd,
      title: draftTitle.trim() || article.title,
    });
    editorDirtyRef.current = false;
    setDirty(false);
    setSavedAt(timeNow());
    setLiveMd(newMd); // 预览立即反映，不等 400ms 快照
    const replaced = await editorRef.current?.replaceMarkdown(newMd);
    if (!replaced) setEditorBump((v) => v + 1); // 极端替换失败才回退 remount
  }, [article.content, article.id, updateArticleContent]);

  const renderPreview = (md: string) => (
    <>
      {previewPhase === 'lite' ? (
        <pre className="whitespace-pre-wrap break-words text-[13px] leading-6 text-[var(--text-secondary)] font-sans m-0">
          {md.length > 48_000 ? `${md.slice(0, 48_000)}\n…` : md}
        </pre>
      ) : (
        <MarkdownView
          content={md}
          onOpenArticle={openArticleByTitle}
          onOpenItem={setSelectedItemId}
          onContentChange={mode === 'preview' ? handlePreviewContentChange : undefined}
          onToggleTask={mode === 'split' ? handleToggleInSplit : undefined}
          hoverPreview={mode === 'preview'}
        />
      )}
      {/* NB-11 分屏探针：md 提交后落点「快照→DOM」全链路耗时 */}
      {mode === 'split' && previewPhase === 'rich' && (
        <SplitRenderProbe md={md} startRef={splitRenderStartRef} />
      )}
      {previewPhase === 'rich' && renderBacklinks()}
      {previewPhase === 'rich' && renderMentions()}
    </>
  );

  return (
    <div className="flex-1 min-w-0 flex flex-col overflow-hidden">
      {/* 顶栏：行内标题 + 模式切换 + 保存状态 + ⋯ 菜单。
          NB-20 原意：整行空白可拖窗。
          NB-28 修正：Tauri 的 data-tauri-drag-region 按最近祖先匹配——
          原先属性挂在整行容器（+ 预览态 h2 也挂），导致在标题 input/h2 上
          「按住拖动以圈选文本」被劫持为拖动整个客户端窗口（点击按钮仍可，
          因点击无位移不触发拖窗；但圈选需要位移 → 被吞）。
          改法：属性从容器与 h2 移除，改挂到标题与右侧控件之间一块独立空白 spacer
          （与 Sidebar.tsx:128 同款「flex-1 空白 div 挂属性」范式）；
          标题两态均可圈选复制，空白区仍可拖窗。 */}
      <div className="px-6 h-10 border-b border-[var(--border-default)] bg-[var(--bg-surface)] flex items-center gap-3 shrink-0">
        {mode !== 'preview' ? (
          <input
            ref={titleInputRef}
            value={draftTitle}
            onChange={(e) => {
              const title = e.target.value;
              setDraftTitle(title);
              const queued = documentDraftQueue.get(article.id);
              documentDraftQueue.update({
                documentId: article.id,
                markdown: queued?.markdown ?? article.content ?? '',
                title,
              });
              scheduleDraftWork();
            }}
            placeholder="未命名文档"
            className="flex-1 min-w-0 bg-transparent text-sm font-semibold text-[var(--text-primary)] focus:outline-none border-b border-transparent focus:border-[var(--accent)] transition-colors"
          />
        ) : (
          <h2 className="flex-1 min-w-0 text-sm font-semibold text-[var(--text-primary)] truncate cursor-text select-text">
            {article.title}
          </h2>
        )}
        {/* 独立空白拖拽区：挂 data-tauri-drag-region，不包裹任何可交互/可选中文本，
            故不会吞掉标题圈选；aria-hidden 对辅助技术隐去纯布局空白 */}
        <div className="flex-1 min-w-0 self-stretch" data-tauri-drag-region aria-hidden="true" />
        {/* 编辑 ↔ 预览：只显示「可切到的对方」图标（预览态→铅笔，编辑/分屏态→眼睛），无文字 */}
        <button
          onClick={() => void switchMode(mode === 'preview' ? 'edit' : 'preview')}
          title={mode === 'preview' ? '切换到编辑（⌘E）' : '切换到预览（⌘E）'}
          aria-label={mode === 'preview' ? '切换到编辑' : '切换到预览'}
          className="p-1.5 rounded-lg shrink-0 text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-sunken)] transition-colors"
        >
          {mode === 'preview' ? <Pencil size={14} /> : <Eye size={14} />}
        </button>
        {/* 分屏：图标旁仅图标，无文字；再点一次回编辑 */}
        <button
          onClick={() => void switchMode(mode === 'split' ? 'edit' : 'split')}
          title={mode === 'split' ? '退出分屏，回到编辑' : '分屏：左编辑右预览'}
          aria-label={mode === 'split' ? '退出分屏' : '分屏'}
          className={`p-1.5 rounded-lg shrink-0 transition-colors ${
            mode === 'split'
              ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
              : 'text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-sunken)]'
          }`}
        >
          <Columns2 size={14} />
        </button>
        {statusNode}
        {/* ⋯ 更多菜单：宿主快捷入口 + 删除（两步确认语义保留） */}
        {(onDelete || moreMenuItems.length > 0) && (
        <div className="relative shrink-0">
          <button
            onClick={() => setMoreMenuOpen((v) => !v)}
            title="更多"
            aria-label="更多"
            className={`p-1.5 rounded-lg transition-colors ${
              moreMenuOpen
                ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                : 'text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-sunken)]'
            }`}
          >
            <MoreHorizontal size={14} />
          </button>
          {moreMenuOpen && (
            <>
              {/* 透明遮罩：点击菜单外任意处关闭 */}
              <div
                className="fixed inset-0 z-40"
                onClick={() => {
                  setMoreMenuOpen(false);
                  setConfirmDelete(false);
                }}
              />
              <div className="absolute right-0 top-full mt-1 z-50 w-40 rounded-[var(--radius-md)] border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] py-1">
                {moreMenuItems.map((item) => (
                  <button
                    key={item.id}
                    onClick={() => {
                      setMoreMenuOpen(false);
                      setConfirmDelete(false);
                      item.onSelect();
                    }}
                    className="w-full flex items-center gap-2 text-left px-3 py-2 text-[13px] font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] transition-colors"
                  >
                    {item.icon}
                    <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  </button>
                ))}
                {onDelete && (
                  <>
                    {moreMenuItems.length > 0 && <div className="my-1 border-t border-[var(--border-default)]" />}
                    <button
                      onClick={() => {
                        if (confirmDelete) {
                          setMoreMenuOpen(false);
                          onDelete();
                        } else {
                          setConfirmDelete(true);
                          setTimeout(() => setConfirmDelete(false), 3000);
                        }
                      }}
                      className={`w-full flex items-center gap-1.5 text-left px-3 py-2 text-[13px] font-medium transition-colors ${
                        confirmDelete
                          ? 'bg-[var(--danger)] text-white hover:opacity-90'
                          : 'text-[var(--danger)] hover:bg-[var(--danger-subtle)]'
                      }`}
                    >
                      <Trash2 size={12} /> {confirmDelete ? '确认删除？' : '删除文档'}
                    </button>
                  </>
                )}
              </div>
            </>
          )}
        </div>
        )}
        {/* 宿主顶栏右端附加元素（项目：类型徽标 + ×移出） */}
        {headerExtra}
      </div>

      {/* 宿主顶栏之下插槽（笔记本：N2 快速捕获栏） */}
      {belowHeader}

      {/* 内容区：编辑 / 分屏 / 预览 + 常驻大纲侧栏（NB-05，三模式可用）。
          NB-26：滚动容器高度链修复——内容行补 min-h-0，预览滚动容器从「h-full 百分比取高」
          改为「列 flex + flex-1 min-h-0」标准滚动范式：WebKit 下 flex 子项 h-full 对
          flex 计算出的父高解析不稳（解析为 auto → 容器随内容撑高 → 无 overflow →
          滚轮失效 + scrollIntoView 无目标可滚 → 大纲跳转失效），改用主轴 flex-1 取高后确定。
          NB-27：编辑面板同因修复——原结构面板与内衬两层 h-full 百分比（对 flex 计算的
          父高解析），WebKit 下同样失效（编辑区随内容撑高、滚轮与大纲跳转无效）；
          改「列 flex + flex-1 min-h-0」直达 MarkdownEditor（其高度契约同轮改为 flex-1）。 */}
      <div className="relative flex flex-1 min-h-0 overflow-hidden">
        {/* 预览态只隐藏并冻结编辑器，不卸载 Crepe。由此保留同文档的 EditorState、选区与
            undo history；切换文档时延后清空 editorArticleId 再销毁，避免挡目标篇首帧。 */}
        {editorMounted && editorArticleId && (
          <div
            aria-hidden={!editorIsCurrent || mode === 'preview' || !pageActive}
            inert={!editorIsCurrent || mode === 'preview' || !pageActive ? true : undefined}
            className={`${
              editorIsCurrent && mode !== 'preview'
                ? 'relative flex-1'
                : 'invisible pointer-events-none absolute inset-0'
            } min-w-0 min-h-0 flex flex-col p-6 ${editorIsCurrent && mode === 'split' ? 'border-r border-[var(--border-default)]' : ''}`}
          >
            <div className="mx-auto flex w-full max-w-3xl flex-1 min-h-0 flex-col">
              <MarkdownEditor
                key={`${editorArticleId}:${editorIsCurrent ? editorBump : 0}`}
                ref={editorIsCurrent ? editorRef : undefined}
                docKey={editorArticleId}
                defaultValue={
                  editorIsCurrent
                    ? seededDraft.markdown
                    : (documentDraftQueue.get(editorArticleId)?.markdown ?? '')
                }
                active={editorIsCurrent && mode !== 'preview' && pageActive}
                skipSnapshotOnDestroy={skipDestroySnapshotRef.current}
                onDocumentChange={scheduleDraftWork}
                onBeforeDestroy={(markdown, documentId) => {
                  const queued = documentDraftQueue.get(documentId);
                  const snapshot = documentDraftQueue.update({
                    documentId,
                    markdown,
                    title: queued?.title.trim() || '未命名文档',
                  });
                  if (snapshot.dirty) void documentDraftQueue.flush(documentId, draftWriters);
                }}
                onAddSelectionToChat={editorIsCurrent ? onAddSelectionToChat : undefined}
                suggestion={editorIsCurrent ? suggestion : null}
                onSuggestionDecision={editorIsCurrent ? onSuggestionDecision : undefined}
              />
            </div>
          </div>
        )}
        {mode !== 'edit' && (
          <div className="flex-1 min-w-0 flex flex-col min-h-0">
            <div ref={previewScrollRef} className="flex-1 min-h-0 overflow-y-auto p-6">
              <div className="max-w-2xl">
                {renderPreview(previewMd)}
              </div>
            </div>
          </div>
        )}
        {/* NB-05 大纲侧栏：预览/分屏走 hb-line-N 锚点跳转 + scrollspy 高亮当前标题；
            编辑态经 MarkdownEditor handle 按标题序号滚动 ProseMirror；![[嵌入]] 内标题已排除。
            NB-26：高度从 h-full 改为行内 stretch（去掉百分比依赖，与滚动容器同因） */}
        {outline.length > 0 && (
          <div
            className={`${outlineOpen ? 'w-44' : 'w-8'} border-l border-[var(--border-default)] bg-[var(--bg-sunken)] shrink-0 flex flex-col overflow-hidden`}
          >
            <button
              onClick={() => setOutlineOpen((v) => !v)}
              title={outlineOpen ? '收起大纲' : '展开大纲'}
              className="flex items-center gap-1.5 px-2.5 py-2.5 text-[12px] font-medium text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] transition-colors shrink-0"
            >
              {outlineOpen ? <ChevronDown size={12} /> : <ListTree size={12} />}
              {outlineOpen && <span className="truncate">大纲 · {outline.length}</span>}
            </button>
            {outlineOpen && (
              <div className="flex-1 overflow-y-auto px-1.5 pb-2 space-y-px">
                {outline.map((h, i) => (
                  <button
                    key={h.line}
                    onClick={() => handleOutlineJump(h.line, i)}
                    style={{ paddingLeft: 8 + (h.level - 1) * 10 }}
                    className={`w-full text-left pr-2 py-1 rounded-md truncate transition-colors ${
                      h.level === 1 ? 'text-[12px] font-semibold' : 'text-[12px]'
                    } ${
                      activeLine === h.line
                        ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                        : 'text-[var(--text-secondary)] hover:bg-[var(--bg-surface)] hover:text-[var(--accent)]'
                    }`}
                    title={h.text}
                  >
                    {h.text}
                  </button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* 底部状态栏：字数 / 反链 / 格式 / 宿主附加项 / 模式与快捷键 */}
      <div className="px-6 py-1.5 border-t border-[var(--border-default)] bg-[var(--bg-surface)] flex items-center gap-3 text-[12px] text-[var(--text-tertiary)] shrink-0">
        <span>字数 {charCount}</span>
        <span>Markdown</span>
        {backlinks.length > 0 && (
          <span className="flex items-center gap-1">
            <Link2 size={11} /> 反链 {backlinks.length}
          </span>
        )}
        {outline.length > 0 && (
          <button
            onClick={() => setOutlineOpen((v) => !v)}
            className="flex items-center gap-1 hover:text-[var(--accent)] transition-colors"
            title="显示/隐藏大纲侧栏"
          >
            <ListTree size={11} /> 大纲 {outline.length}
          </button>
        )}
        {/* 宿主附加项（笔记本：NB-04 快捷键参考卡 + NB-11 性能探针开关） */}
        {statusExtra}
        <span className="ml-auto">
          {mode === 'edit' && '编辑 · 输入停顿后自动保存'}
          {mode === 'split' && '分屏 · 变更后更新预览'}
          {mode === 'preview' && '预览'}
          {' · ⌘E 切换 · ⌘S 保存'}
        </span>
      </div>
    </div>
  );
});

export default NoteWorkbench;
