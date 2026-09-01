import { startTransition, useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import NoteWorkbench, { type NoteWorkbenchHandle } from './NoteWorkbench';
import ShortcutHelp from './ShortcutHelp';
import PerfOverlay from './PerfOverlay';
import ContextMenu, { type CtxMenuItem, type CtxMenuState } from '../ui/ContextMenu';
import VerticalResizeHandle from '../ui/VerticalResizeHandle';
import EmptyState from '../ui/EmptyState';
import ScoreBadge from '../ui/ScoreBadge';
import { useAppStore } from '../../stores/appStore';
import { useProjectStore } from '../../stores/projectStore';
import { getDailyPicks, exportArticle, getDataDir, getSetting, updateSetting, deleteArticles as tauriDeleteArticles, documentCurrentVersion } from '../../services/tauri';
import { todayStr, appendCaptureLine } from '../../services/journal';
import { articleActivityCounts, firstNotebookArticleIdForDate, notebookDateKey } from '../../services/noteActivityCalendar';
import { findHeadingLine, findBlockLine } from '../../services/noteLinks';
import { instantiateTemplate, isTemplateContent } from '../../services/noteTemplates';
import { missingNoteExamples, noteExampleArticles, noteExamples } from '../../services/noteExamples';
import { countMatches, firstMatchContext } from '../../services/noteSearch';
import SearchHighlight from './SearchHighlight';
import MonthlyActivityCalendar from './MonthlyActivityCalendar';
import ProjectChatPanel from '../aistudio/ProjectChatPanel';
import type { RunContext } from '../../stores/agentStore';
import { selectionLineRange } from '../../editor/selection/lineRange';
import { applyDocumentDiffHunks } from '../../editor/documentDiffPlugin';
import type { InlineSuggestion } from '../editor/MarkdownEditor';
import { activeChangeSession, useChangeSessionStore } from '../../stores/changeSessionStore';
import {
  resolveHunkReview,
  type HunkDecision,
  type HunkDecisionTarget,
} from '../../services/changeSession';
import type { Article, DailyPick } from '../../types';
import {
  BookOpen,
  BookOpenText,
  Pencil,
  Loader2,
  FilePlus2,
  Search,
  Hash,
  Trash2,
  ChevronDown,
  Zap,
  Sparkles,
  NotebookPen,
  FolderDown,
  FolderOpen,
  Activity,
  Gauge,
  PanelLeftClose,
  PanelLeftOpen,
  AlertTriangle,
  ArrowLeft,
} from 'lucide-react';

function afterNextPaint(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

const typeBadge: Record<string, { text: string; cls: string }> = {
  nightly: { text: '夜间生成', cls: 'bg-[var(--accent-subtle)] text-[var(--accent)]' },
  'deep-dive': { text: '深度解读', cls: 'bg-[var(--success-subtle)] text-[var(--success)]' },
  manual: { text: '笔记', cls: 'bg-[var(--accent-subtle)] text-[var(--accent)]' },
  journal: { text: '日记', cls: 'bg-[var(--warning-subtle)] text-[var(--warning)]' },
};

const NOTES_AGENT_WIDTH_KEY = 'ui:notes-agent-width';
const DEFAULT_NOTES_AGENT_WIDTH = 400;
const MIN_NOTES_AGENT_WIDTH = 280;
const MIN_NOTES_EDITOR_WIDTH = 320;

/** 走查修正：今日痕迹面板 Top5 类别 id → 短标签。
 *  原实现固定 w-9 裸显原始 id，producthunt/huggingface 等长 id 溢出压到标题上造成文字重叠 */
const pickCategoryLabel: Record<string, string> = {
  github: 'GitHub',
  arxiv: 'arXiv',
  hackernews: 'HN',
  producthunt: 'PH',
  huggingface: 'HF',
  aihot: 'AH',
};

/** 提取笔记中的 #标签（# 后直接跟字符，天然避开「# 标题」语法） */
function collectTags(a: Article): string[] {
  return Array.from(a.content.matchAll(/#[\p{L}\p{N}_-]+/gu)).map((m) => m[0].slice(1));
}

interface DocWorkspaceProps {
  /** 本空间管理哪些文档（AI 解读 / 个人笔记） */
  scope: (a: Article) => boolean;
  listTitle: string;
  newDocLabel?: string;
  emptyHint: string;
  /** 笔记本页：显示 #标签 过滤 */
  showTags?: boolean;
  /** 笔记本页：N2 Journals——只读定位已有日期笔记，显式新建后再落地；顶栏快速捕获 + 今日痕迹聚合 */
  journal?: boolean;
  /** 笔记本页：NB-07 模板体系——新建按钮带模板下拉（#template 标签的笔记即模板） */
  enableTemplates?: boolean;
  /** 笔记本专用：公开功能范例只在用户点击导入后创建。 */
  enableStarterExamples?: boolean;
  /** 笔记本专用：右侧 Hermes Agent；窄窗口覆盖、宽窗口并列。 */
  agentCollapsed?: boolean;
  onRequestAgent?: () => void;
}

/**
 * 文档工作台（深度解读 / 个人笔记本共用）：
 * - 左栏列表 + 笔记本特性（N2 journals 捕获/痕迹、NB-07 模板、搜索、#标签）
 * - NB-23：单文档书写工作台抽取为共享组件 NoteWorkbench（编辑/分屏/预览、双链、反链、
 *   大纲、快捷键、自动保存全在其内），AI 工作室项目模式嵌入同一实现，两空间书写体验同源
 */
export default function DocWorkspace({
  scope,
  listTitle,
  newDocLabel,
  emptyHint,
  showTags,
  journal,
  enableTemplates,
  enableStarterExamples,
  agentCollapsed = true,
  onRequestAgent,
}: DocWorkspaceProps) {
  const {
    articles,
    saveArticle,
    updateArticleContent,
    updateArticleTitle,
    deleteArticle,
    loadArticles,
    pendingArticleId,
    clearPendingArticle,
    pendingAnchorLine,
    clearPendingAnchor,
    setSelectedItemId,
    requestOpenArticle,
    openArticleAtLine,
  } = useAppStore(useShallow((state) => ({
    articles: state.articles,
    saveArticle: state.saveArticle,
    updateArticleContent: state.updateArticleContent,
    updateArticleTitle: state.updateArticleTitle,
    deleteArticle: state.deleteArticle,
    loadArticles: state.loadArticles,
    pendingArticleId: state.pendingArticleId,
    clearPendingArticle: state.clearPendingArticle,
    pendingAnchorLine: state.pendingAnchorLine,
    clearPendingAnchor: state.clearPendingAnchor,
    setSelectedItemId: state.setSelectedItemId,
    requestOpenArticle: state.requestOpenArticle,
    openArticleAtLine: state.openArticleAtLine,
  })));
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // NB-23：共享书写工作台句柄（flush / 捕获写回 / 新建进编辑态等跨组件通道）
  const wbRef = useRef<NoteWorkbenchHandle>(null);
  // NB-11 性能探针面板开关（状态栏「性能」；打点逻辑随工作台抽入 NoteWorkbench）
  const [perfOpen, setPerfOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [selectedTag, setSelectedTag] = useState<string | null>(null);
  const [genMessage, setGenMessage] = useState<string | null>(null);
  const [ctxMenu, setCtxMenu] = useState<CtxMenuState | null>(null); // NB-13 列表右键上下文菜单
  // 列表行内重命名（用户指令：点哪儿改哪儿，不用弹窗；笔记本/深度解读/AI 工作室同源）
  const [renamingDocId, setRenamingDocId] = useState<string | null>(null);
  const [renameDocDraft, setRenameDocDraft] = useState('');
  const renameDocDoneRef = useRef(false); // Enter→blur 双触发防重
  const [listCollapsed, setListCollapsed] = useState(false); // NB-14 列表栏折叠（持久化）
  useEffect(() => {
    getSetting('ui:doclist-collapsed')
      .then((v) => setListCollapsed(v === '1'))
      .catch(() => {});
  }, []);
  const toggleListCollapsed = () => {
    setListCollapsed((v) => {
      const next = !v;
      updateSetting('ui:doclist-collapsed', next ? '1' : '0').catch(() => {});
      return next;
    });
  };
  const [agentWidth, setAgentWidth] = useState(DEFAULT_NOTES_AGENT_WIDTH);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const [workspaceWidth, setWorkspaceWidth] = useState(() => window.innerWidth);
  useEffect(() => {
    const host = workspaceRef.current;
    if (!host) return;
    let frame = 0;
    const observer = new ResizeObserver(([entry]) => {
      const next = Math.round(entry.contentRect.width);
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        setWorkspaceWidth((current) => Math.abs(current - next) > 1 ? next : current);
      });
    });
    observer.observe(host);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, []);
  useEffect(() => {
    getSetting(NOTES_AGENT_WIDTH_KEY)
      .then((raw) => {
        const saved = Number(raw);
        if (Number.isFinite(saved)) {
          setAgentWidth(Math.max(MIN_NOTES_AGENT_WIDTH, Math.min(1100, saved)));
        }
      })
      .catch(() => {});
  }, []);
  const visibleListWidth = listCollapsed ? 40 : 288;
  const maxAgentWidth = Math.max(
    MIN_NOTES_AGENT_WIDTH,
    Math.min(1100, workspaceWidth - visibleListWidth - MIN_NOTES_EDITOR_WIDTH)
  );
  const visibleAgentWidth = Math.min(agentWidth, maxAgentWidth);
  // 与工作室一致：窄工作区使用覆盖面板，宽工作区才开放左右拖动。
  const agentOverlay = workspaceWidth < 1120;
  const [templateMenuOpen, setTemplateMenuOpen] = useState(false); // NB-07：新建模板下拉
  const [importingExamples, setImportingExamples] = useState(false);
  // NB-30：Shift+范围选 + 批量删除（macOS Finder 范式）
  const [batchIds, setBatchIds] = useState<Set<string>>(new Set());
  const [batchAnchor, setBatchAnchor] = useState<string | null>(null);
  const [batchConfirm, setBatchConfirm] = useState(false); // 批量删除两步确认

  // —— N2 Journals ——
  const today = todayStr();
  const [selectedDate, setSelectedDate] = useState<string | null>(() => todayStr());
  const [calendarMonth, setCalendarMonth] = useState(
    () => new Date(`${todayStr()}T12:00:00`)
  );
  const [captureText, setCaptureText] = useState('');
  const [tracesOpen, setTracesOpen] = useState(false);
  const [todayPicks, setTodayPicks] = useState<DailyPick[]>([]);

  // 本空间文档（scope 过滤）；双链/反链仍基于全量 articles 跨空间互通
  const docs = articles.filter(scope);
  const dateFilteredDocs = journal && selectedDate
    ? docs.filter((article) => notebookDateKey(article) === selectedDate)
    : docs;
  const selected = dateFilteredDocs.find((a) => a.id === selectedId) || dateFilteredDocs[0];
  const activityCounts = useMemo(() => articleActivityCounts(articles), [articles]);
  const revealArticleDate = useCallback((article: Article) => {
    if (!journal) return;
    const dateKey = notebookDateKey(article);
    if (!dateKey) return;
    setSelectedDate(dateKey);
    setCalendarMonth(new Date(`${dateKey}T12:00:00`));
  }, [journal]);
  const [chatSelection, setChatSelection] = useState<RunContext | null>(null);
  const [chatSelectionLines, setChatSelectionLines] = useState<[number, number] | null>(null);
  const selectedDocIdRef = useRef<string | null>(selected?.id ?? null);
  selectedDocIdRef.current = selected?.id ?? null;

  // 笔记本虽然没有 projectId，仍与工作室共用同一个 Change Session 状态机。
  // ProjectChatPanel 以 `notebook` 作为仅前端审阅作用域采纳本轮 Host Patch。
  const activeChange = useChangeSessionStore((state) => activeChangeSession(state, selected?.id));
  const decideChangeHunk = useChangeSessionStore((state) => state.decideHunk);

  useEffect(() => {
    setChatSelection(null);
    setChatSelectionLines(null);
    setTracesOpen(false);
  }, [selected?.id]);

  useEffect(() => {
    if (!tracesOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setTracesOpen(false);
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [tracesOpen]);

  const suggestion = useMemo<InlineSuggestion | null>(() => {
    if (!activeChange || !selected || activeChange.documentId !== selected.id) return null;
    if (
      activeChange.preview.hunks.length === 0 ||
      !(
        activeChange.phase === 'proposed' ||
        activeChange.phase === 'conflict' ||
        (activeChange.phase === 'applying' && activeChange.pendingAction !== 'undo')
      )
    ) return null;
    const diff = activeChange.preview;
    const anchorLine = diff.hunks.reduce(
      (max, hunk) => Math.max(
        max,
        hunk.startLine + hunk.contextBefore.length + Math.max(hunk.removed.length, 1)
      ),
      1
    );
    const single = diff.hunks.length === 1 ? diff.hunks[0] : null;
    const inlineText = single ? single.added.join(' ').trim() : '';
    return {
      operationId: diff.operationId,
      anchorLine,
      hunks: diff.hunks,
      mode: single && single.removed.length === 0 && inlineText.length > 0 && inlineText.length <= 40
        ? 'inline'
        : 'block',
      inlineText: inlineText || undefined,
      phase: activeChange.phase,
      decisions: activeChange.decisions,
      pendingAction: activeChange.pendingAction,
      error: activeChange.error,
    };
  }, [activeChange, selected]);

  useEffect(() => {
    if (!suggestion) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      if (!cancelled) startTransition(() => wbRef.current?.enterEdit());
    }, 0);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [suggestion?.operationId]);

  const refreshEditorFromDb = useCallback(async (documentId: string) => {
    try {
      await loadArticles();
      const fresh = useAppStore.getState().articles.find((article) => article.id === documentId);
      if (fresh && selectedDocIdRef.current === documentId) {
        wbRef.current?.applyExternalContent(fresh.content || '');
      }
    } catch (error) {
      console.warn('[notebook] refreshEditorFromDb failed:', error);
    }
  }, [loadArticles]);

  const decideSuggestion = useCallback(async (
    target: HunkDecisionTarget,
    decision: Exclude<HunkDecision, 'pending'>
  ) => {
    const session = activeChange;
    if (!session || session.phase !== 'proposed') return;
    const review = resolveHunkReview(session.decisions, target, decision);
    const checkpoint = await wbRef.current?.captureViewCheckpoint() ?? null;
    let baseline: string | null = null;
    if (review.requiresDraftFlush) {
      if (!wbRef.current || !(await wbRef.current.flush())) return;
      baseline = wbRef.current.getLiveMarkdown();
      await afterNextPaint();
    }
    const resolution = await decideChangeHunk(session.operationId, target, decision, checkpoint);
    if (!resolution || resolution.kind !== 'applied') return;

    const nextMarkdown = baseline != null
      ? applyDocumentDiffHunks(baseline, session.preview.hunks, resolution.approvedHunks)
      : null;
    // NEXT-042：整块批准时标题随正文一次写盘；快路径同步侧边栏标题。
    const appliedTitle = resolution.result.appliedTitle ?? null;
    await afterNextPaint();
    if (nextMarkdown != null) {
      useAppStore.setState((state) => ({
        articles: state.articles.map((article) =>
          article.id === session.documentId
            ? {
                ...article,
                content: nextMarkdown,
                ...(appliedTitle ? { title: appliedTitle } : {}),
                edited: true,
              }
            : article
        ),
      }));
      if (selectedDocIdRef.current === session.documentId) {
        wbRef.current?.applyExternalContent(nextMarkdown, { addToHistory: true });
        if (checkpoint) void wbRef.current?.restoreViewCheckpoint(checkpoint);
      }
    } else {
      await refreshEditorFromDb(session.documentId);
      if (checkpoint) void wbRef.current?.restoreViewCheckpoint(checkpoint);
    }
  }, [activeChange, decideChangeHunk, refreshEditorFromDb]);

  const changeStatusBar = activeChange?.phase === 'proposed' && activeChange.preview.proposedTitle ? (
    <div className="hb-change-session-bar">
      <span>全部批准变更块时，标题将同步改为《{activeChange.preview.proposedTitle}》</span>
    </div>
  ) : activeChange?.phase === 'applying' ? (
    <div className="hb-change-session-bar">
      <Loader2 size={12} className="animate-spin" />
      <span>{activeChange.pendingAction === 'reject' ? '正在关闭建议…' : '正在应用已选择的修改…'}</span>
    </div>
  ) : activeChange?.phase === 'conflict' ? (
    <div className="hb-change-session-bar hb-change-session-bar-error">
      <AlertTriangle size={12} />
      <span>{activeChange.error ?? '修改与当前文档发生冲突，请重新下达指令。'}</span>
    </div>
  ) : null;

  const captureChatSelection = () => {
    const doc = selected;
    if (!doc) return;
    onRequestAgent?.();
    void (async () => {
      try {
        const baseVersion = await documentCurrentVersion(doc.id);
        const snapshot = await wbRef.current?.captureSelectionSnapshot({
          articleId: doc.id,
          projectId: 'notebook',
          baseVersion,
        });
        if (!snapshot || selectedDocIdRef.current !== doc.id) return;
        const liveMarkdown = wbRef.current?.getLiveMarkdown() || doc.content || '';
        setChatSelectionLines(selectionLineRange(liveMarkdown, snapshot.selectedMarkdown));
        setChatSelection({
          articleId: snapshot.articleId,
          title: doc.title,
          baseVersion: snapshot.baseVersion,
          selectedMarkdown: snapshot.selectedMarkdown,
          selectedTextHash: snapshot.selectedTextHash,
          beforeContext: snapshot.beforeContext,
          afterContext: snapshot.afterContext,
        });
      } catch {
        // 捕获失败不打断写作；用户仍可直接在 Agent 面板提问。
      }
    })();
  };

  // 标签云（按频次取前 12）
  const tagCounts = new Map<string, number>();
  if (showTags) {
    for (const d of docs) {
      for (const t of collectTags(d)) tagCounts.set(t, (tagCounts.get(t) ?? 0) + 1);
    }
  }
  const topTags = Array.from(tagCounts.entries())
    .sort((a, b) => b[1] - a[1])
    .slice(0, 12);

  // 列表展示 = scope + 日期 + 标签 + 搜索四重过滤
  const listDocs = dateFilteredDocs.filter((d) => {
    if (selectedTag && !collectTags(d).includes(selectedTag)) return false;
    if (search.trim()) {
      const q = search.trim().toLowerCase();
      if (!d.title.toLowerCase().includes(q) && !d.content.toLowerCase().includes(q)) return false;
    }
    return true;
  });

  // NB-23：工作台导航回调（与项目模式共享同一 NoteWorkbench 实现，此处是笔记本侧落点）
  // 已有文档：先同步捕获草稿到按文档队列，再立即选中；SQLite 写入在后台完成，
  // 不把文档首帧阻塞在磁盘 I/O 上。heading 经 openArticleAtLine 锚点定位。
  const wbOpenDocument = (doc: Article, heading?: string, blockId?: string) => {
    void wbRef.current?.flush();
    revealArticleDate(doc);
    if (blockId) {
      const line = findBlockLine(doc.content, blockId);
      if (line != null) {
        openArticleAtLine(doc.id, line);
        return;
      }
    }
    if (heading) {
      const line = findHeadingLine(doc.content, heading);
      if (line != null) {
        openArticleAtLine(doc.id, line);
        return;
      }
    }
    setSelectedId(doc.id);
  };

  // [[双链]] 指向不存在标题 → 笔记本侧新建：建好即选中并进编辑态聚焦标题（命名引导）
  // NB-31：新建失败不选中不跳转（store 里不会出现幽灵记录），错误可见
  const wbCreateDocument = async (title: string) => {
    const article: Article = {
      id: crypto.randomUUID(),
      title,
      content: '',
      articleType: 'manual',
      edited: false,
      createdAt: new Date().toISOString(),
      blocksJson: null,
    };
    try {
      await saveArticle(article);
    } catch (e) {
      setGenMessage(`❌ 新建失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    revealArticleDate(article);
    setSelectedId(article.id);
    // 工作台 article 变更先复位预览，挂载后经 handle 进编辑态
    window.setTimeout(() => {
      wbRef.current?.enterEdit();
      wbRef.current?.focusTitle();
    }, 60);
  };

  // ⌘K 快速切换器跳转：目标在本空间则选中并消费 pending；否则留给目标空间的工作台处理
  // N5：若携带 pendingAnchorLine（Tasks 页行级回链），预览渲染后滚动到对应 hb-line-N 锚点
  useEffect(() => {
    if (!pendingArticleId) return;
    const pendingArticle = docs.find((d) => d.id === pendingArticleId);
    if (pendingArticle) {
      const anchor = pendingAnchorLine;
      revealArticleDate(pendingArticle);
      setSelectedId(pendingArticleId);
      // 同文档锚点时 id 不变不触发工作台复位——显式回预览态确保 hb-line-N 锚点 DOM 存在
      wbRef.current?.enterPreview();
      clearPendingArticle();
      clearPendingAnchor();
      if (anchor != null) {
        // 等预览 DOM（hb-line-N）渲染完成再滚；锚点见 MarkdownView 标题/列表项 id
        setTimeout(() => {
          document.getElementById(`hb-line-${anchor}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
        }, 150);
      }
    }
  }, [pendingArticleId, docs, clearPendingArticle, pendingAnchorLine, clearPendingAnchor, revealArticleDate]);

  // 当前选中的是否今日 journal：决定捕获栏与痕迹面板的显隐
  const isTodayJournal = !!selected && selected.articleType === 'journal' && selected.title === today;

  // 今日痕迹（聚合走 UI 面板，不注入正文）：今日笔记 / 今日 AI 解读 / 今日 Top5
  const localDate = (iso: string) => new Date(iso).toLocaleDateString('sv-SE');
  const todayNotes = useMemo(
    () => articles.filter((a) => a.articleType === 'manual' && localDate(a.createdAt) === today),
    [articles, today]
  );
  const todayAiArticles = useMemo(
    () =>
      articles.filter(
        (a) => (a.articleType === 'nightly' || a.articleType === 'deep-dive') && localDate(a.createdAt) === today
      ),
    [articles, today]
  );

  useEffect(() => {
    if (!journal) return;
    let cancelled = false;
    getDailyPicks(undefined, 200)
      .then((picks) => {
        if (!cancelled) setTodayPicks(picks.filter((p) => p.date === today));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [journal, today]);

  // 快速捕获：Enter 把一条速记追加进「## 速记」（时间顺序沉底）
  // NB-31：写盘失败时保留输入框文本与错误提示，用户可直接重试，不丢速记
  const handleCapture = async () => {
    const text = captureText.trim();
    if (!text || !selected || !isTodayJournal) return;
    // 基线取工作台实时内容（编辑/分屏态），防止心跳随后用旧快照覆盖捕获
    const base = wbRef.current?.getLiveMarkdown() ?? selected.content;
    const newMd = appendCaptureLine(base, text);
    try {
      await updateArticleContent(selected.id, newMd);
    } catch (e) {
      console.error('Failed to capture quick note:', e);
      setGenMessage(`❌ 速记保存失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    setCaptureText('');
    wbRef.current?.applyExternalContent(newMd); // 同步基线 + remount 编辑器加载新内容
  };

  const selectArticle = (id: string) => {
    if (selected && id !== selected.id) void wbRef.current?.flush();
    setSelectedId(id); // 工作台 article 变更自动复位预览态
  };

  const selectCalendarDate = (dateKey: string) => {
    if (selected && notebookDateKey(selected) !== dateKey) void wbRef.current?.flush();
    setSelectedDate(dateKey);
    setCalendarMonth(new Date(`${dateKey}T12:00:00`));
    const first = docs.find((article) => notebookDateKey(article) === dateKey);
    setSelectedId(first?.id ?? null);
    setBatchIds(new Set());
  };

  const returnCalendarToToday = () => {
    const now = todayStr();
    setSelectedDate(now);
    setCalendarMonth(new Date(`${now}T12:00:00`));
    setSelectedId(firstNotebookArticleIdForDate(docs, now));
    setBatchIds(new Set());
  };

  // NB-30：列表项点击分发——常规=选中；Shift=范围选（从锚点到当前）；⌘/Ctrl=切换勾/取消勾
  const handleListClick = (e: ReactMouseEvent, doc: Article) => {
    if (e.shiftKey && batchAnchor && listDocs.length > 0) {
      // Shift+范围选：从锚点到当前项之间的全部文档加入 batchIds
      const anchorIdx = listDocs.findIndex((d) => d.id === batchAnchor);
      const curIdx = listDocs.findIndex((d) => d.id === doc.id);
      if (anchorIdx >= 0 && curIdx >= 0) {
        const [lo, hi] = anchorIdx <= curIdx ? [anchorIdx, curIdx] : [curIdx, anchorIdx];
        const rangeIds = listDocs.slice(lo, hi + 1).map((d) => d.id);
        setBatchIds(new Set(rangeIds));
        return; // Shift 选不触发单篇选中
      }
    }
    if (e.metaKey || e.ctrlKey) {
      // ⌘/Ctrl：切换勾选
      setBatchIds((prev) => {
        const next = new Set(prev);
        if (next.has(doc.id)) next.delete(doc.id);
        else next.add(doc.id);
        console.log('[NB-30 笔记本选择] Cmd+点击', doc.id, 'batchIds ->', [...next]);
        return next;
      });
      setBatchAnchor(doc.id);
      return;
    }
    // 常规点击：清 batch、选中
    if (batchIds.size > 0) {
      console.log('[NB-30 笔记本选择] 普通点击清空 batch，原 batchIds =', [...batchIds]);
      setBatchIds(new Set());
    }
    setBatchAnchor(doc.id);
    selectArticle(doc.id);
  };

  // NB-30：批量删除（两步确认 3s 回落，与单篇删除范式一致）
  const handleBatchDelete = async () => {
    if (!batchConfirm) {
      setBatchConfirm(true);
      setTimeout(() => setBatchConfirm(false), 3000);
      return;
    }
    // 先落盘当前编辑文档（若被选中则其内容即将被删）
    await wbRef.current?.flush();
    // 快照所有待删 ID（避免循环中 state 变化影响迭代）
    const ids = [...batchIds];
    console.log('[NB-30 笔记本批量删除] batchIds =', ids, 'size =', ids.length);
    // 单次后端事务删除，避免多个 SQLite 连接并发写锁导致只成功一篇。
    const deletedIds = await tauriDeleteArticles(ids);
    // 后端成功后再复位选中态和本地列表；失败时保留当前界面，避免“假删除”。
    if (selected && deletedIds.includes(selected.id)) setSelectedId(null);
    const idSet = new Set(deletedIds);
    useAppStore.setState((s) => ({
      articles: s.articles.filter((a) => !idSet.has(a.id)),
    }));
    useProjectStore.setState((s) => ({
      memberships: s.memberships.filter((m) => !idSet.has(m.articleId)),
    }));
    setBatchIds(new Set());
    setBatchConfirm(false);
    setBatchAnchor(null);
  };

  // NB-30：Esc 清除批量选择
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && batchIds.size > 0) {
        setBatchIds(new Set());
        setBatchConfirm(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [batchIds.size]);

  // 新建笔记：免弹窗直建「未命名文档」进编辑态（聚焦标题并全选，直接输入即改名）
  // NB-31：新建失败不选中不跳转，错误可见（store 不插入幽灵记录）
  const handleNew = async () => {
    await wbRef.current?.flush();
    const article: Article = {
      id: crypto.randomUUID(),
      title: '未命名文档',
      content: '',
      articleType: 'manual',
      edited: false,
      createdAt: new Date().toISOString(),
      blocksJson: null,
    };
    try {
      await saveArticle(article);
    } catch (e) {
      setGenMessage(`❌ 新建失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    revealArticleDate(article);
    setSelectedId(article.id);
    window.setTimeout(() => {
      wbRef.current?.enterEdit();
      wbRef.current?.focusTitle();
    }, 60);
  };

  // NB-07 模板列表 = 带 #template 标签的 manual 笔记（笔记即模板，随时可编辑，零新存储；
  // 对标 Obsidian Templates——其模板也只是模板文件夹里的普通文件）
  const templates = useMemo(
    () =>
      articles
        .filter((a) => a.articleType === 'manual' && isTemplateContent(a.content))
        .sort((a, b) => a.title.localeCompare(b.title, 'zh-CN')),
    [articles]
  );

  // NB-07 从模板新建：剥离 #template 标签（防新笔记自己也变成模板）+ 变量替换
  // （{{title}}/{{date}}/{{time}}，{{title}} 取创建时刻值，同 Obsidian 插入语义）；
  // 标题置「未命名文档」并聚焦全选，引导立即命名（基线由工作台 article 复位自动同步实例化内容）
  const handleNewFromTemplate = async (tpl: Article) => {
    await wbRef.current?.flush();
    const now = new Date();
    const content = instantiateTemplate(tpl.content, {
      title: '未命名文档',
      date: todayStr(),
      time: now.toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' }),
    });
    const article: Article = {
      id: crypto.randomUUID(),
      title: '未命名文档',
      content,
      articleType: 'manual',
      edited: false,
      createdAt: now.toISOString(),
      blocksJson: null,
    };
    // NB-31：模板实例化落盘失败则不跳转、不关菜单（可重试），错误可见
    try {
      await saveArticle(article);
    } catch (e) {
      setGenMessage(`❌ 新建失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    revealArticleDate(article);
    setSelectedId(article.id);
    setTemplateMenuOpen(false);
    window.setTimeout(() => {
      wbRef.current?.enterEdit();
      wbRef.current?.focusTitle();
    }, 60);
  };

  const remainingExampleCount = useMemo(
    () => missingNoteExamples(articles.map((article) => article.title)).length,
    [articles],
  );

  const handleImportExamples = async () => {
    if (importingExamples) return;
    await wbRef.current?.flush();
    const drafts = noteExampleArticles(articles.map((article) => article.title));
    if (drafts.length === 0) {
      const firstExisting = articles.find((article) => article.title === noteExamples[0]?.title);
      if (firstExisting) setSelectedId(firstExisting.id);
      setGenMessage('6 篇功能范例已经在笔记本中。');
      setTemplateMenuOpen(false);
      return;
    }
    setImportingExamples(true);
    let firstSaved: Article | null = null;
    try {
      for (const article of drafts) {
        await saveArticle(article);
        firstSaved ??= article;
      }
      if (firstSaved) {
        revealArticleDate(firstSaved);
        setSelectedId(firstSaved.id);
      }
      setTemplateMenuOpen(false);
      setGenMessage(`已导入 ${drafts.length} 篇功能范例；同名笔记未覆盖。`);
    } catch (e) {
      setGenMessage(`❌ 范例导入未完成：${e instanceof Error ? e.message : String(e)}。再次导入只会补齐缺失项。`);
    } finally {
      setImportingExamples(false);
    }
  };

  // NB-31：后端删除成功后才复位选中；失败保留选中态与列表项（防"假删除"）
  const handleDelete = async () => {
    if (!selected) return;
    const id = selected.id;
    try {
      await deleteArticle(id);
    } catch (e) {
      setGenMessage(`❌ 删除失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    setSelectedId(null);
  };

  // NB-13：列表项右键菜单（Obsidian 形态：重命名 / 单篇导出 / 在访达中显示 / 删除，三空间同源体验）
  // 重命名走 updateArticleTitle：同一 article 在笔记本/深度解读/AI 工作室任一入口改名，
  // 全局同源生效（同一条 articles 记录），并自动同步全库 [[双链]]（NB-06）与语义索引（N3）。
  const openDocMenu = (e: ReactMouseEvent, doc: Article) => {
    e.preventDefault();
    e.stopPropagation();
    const items: CtxMenuItem[] = [
      {
        label: '重命名',
        icon: <Pencil size={13} />,
        onClick: () => {
          renameDocDoneRef.current = false;
          setRenamingDocId(doc.id);
          setRenameDocDraft(doc.title);
        },
      },
      {
        label: '导出为 Markdown',
        icon: <FolderDown size={13} />,
        onClick: () => void doExportArticle(doc),
      },
    ];
    // 仅笔记有 .md 落盘文件；deep-dive 等 DB 型文章无源文件，不显示该项
    if (doc.articleType === 'manual' || doc.articleType === 'journal') {
      items.push({
        label: '在访达中显示',
        icon: <FolderOpen size={13} />,
        onClick: () => void doRevealArticle(doc),
      });
    }
    items.push({
      label: '删除',
      icon: <Trash2 size={13} />,
      danger: true,
      onClick: () => void deleteDocById(doc),
    });
    setCtxMenu({ x: e.clientX, y: e.clientY, items });
  };

  /**
   * 行内重命名提交：走 updateArticleTitle（同源改名 + NB-06 双链同步 + N3 索引）。
   * 若改的是当前打开的文档，工作台 draftTitle 经 article.title 变化自动同步
   * （NB-23 NoteWorkbench 内的标题同步 effect），无需手动干预，也不会被 flush 覆盖回旧名。
   * 预览态头部直接读 article.title（store 驱动），改名后自动同步。
   */
  const submitRenameDoc = async (doc: Article) => {
    if (renameDocDoneRef.current) return; // Enter→blur 双触发防重
    renameDocDoneRef.current = true;
    const v = renameDocDraft.trim();
    setRenamingDocId(null);
    if (v && v !== doc.title) {
      try {
        await updateArticleTitle(doc.id, v);
      } catch (e) {
        // NB-31：改名失败可见；内存标题已乐观更新，下次成功改名/落盘随之修正
        setGenMessage(`❌ 改名失败：${e instanceof Error ? e.message : String(e)}`);
      }
    }
  };

  // NB-13 单篇导出：当前打开的文档先 flush 未落盘内容；完成以访达高亮导出文件作反馈
  const doExportArticle = async (doc: Article) => {
    try {
      if (selected?.id === doc.id) await wbRef.current?.flush();
      const report = await exportArticle(doc.id);
      await revealItemInDir(report.path);
    } catch (e) {
      console.warn('[export] 单篇导出失败:', e);
    }
  };

  // NB-13 在访达中显示：notes/<id>.md 源文件
  const doRevealArticle = async (doc: Article) => {
    try {
      const dir = await getDataDir();
      await revealItemInDir(`${dir}/notes/${doc.id}.md`);
    } catch (e) {
      console.warn('[notes] 访达定位失败:', e);
    }
  };

  // NB-13 删除任意列表项（复用会话内 journal 防重建语义；删当前选中则复位右栏）
  // NB-31：后端删除成功后才复位选中与防重建标记；失败保留列表项并给出错误（防"假删除"）
  const deleteDocById = async (doc: Article) => {
    try {
      await deleteArticle(doc.id);
    } catch (e) {
      setGenMessage(`❌ 删除失败：${e instanceof Error ? e.message : String(e)}`);
      return;
    }
    if (selected?.id === doc.id) setSelectedId(null);
  };

  // 反链 / 未链接提及 / 大纲 / 状态栏 / 预览渲染等单文档工作台能力
  // 已随 NB-23 整体搬入共享组件 NoteWorkbench（此处只保留笔记本空间特有的今日痕迹面板）

  // 今日痕迹与正文完全分离：默认隐藏，从顶栏 ⋯ 进入独立视图。
  const traceCount = todayNotes.length + todayAiArticles.length + todayPicks.length;
  const tracePanel = tracesOpen && (
    <section
      className="absolute inset-0 z-30 flex min-h-0 flex-col bg-[var(--bg-canvas)]"
      aria-label="今日痕迹"
    >
      <div className="h-10 shrink-0 border-b border-[var(--border-default)] px-6 flex items-center gap-2 bg-[var(--bg-surface)]">
        <button
          autoFocus
          onClick={() => setTracesOpen(false)}
          className="p-1.5 -ml-1.5 rounded-lg text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-sunken)] transition-colors"
          title="返回文档"
          aria-label="返回文档"
        >
          <ArrowLeft size={14} />
        </button>
        <Sparkles size={14} className="text-[var(--gold)]" />
        <h2 className="text-sm font-bold text-[var(--text-primary)]">今日痕迹</h2>
        <span className="text-xs text-[var(--text-tertiary)]">{traceCount} 项</span>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-5">
        <div className="mx-auto max-w-3xl space-y-5">
          {traceCount === 0 && (
            <EmptyState icon={Sparkles} title="今天还没有可展示的痕迹" className="py-10" />
          )}
            {todayNotes.length > 0 && (
              <div>
                <p className="mb-2 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider">今日笔记</p>
                <div className="space-y-1.5">
                  {todayNotes.map((a) => (
                    <button
                      key={a.id}
                      onClick={() => {
                        setTracesOpen(false);
                        void selectArticle(a.id);
                      }}
                      className="w-full flex items-center gap-2 text-left px-3 py-2 rounded-lg bg-[var(--bg-surface)] border border-[var(--border-default)] hover:border-[var(--accent-border)] transition-colors"
                    >
                      <NotebookPen size={12} className="text-[var(--accent)] shrink-0" />
                      <span className="flex-1 text-xs text-[var(--text-secondary)] truncate">{a.title}</span>
                      <span className="text-xs text-[var(--text-tertiary)] shrink-0">
                        {new Date(a.createdAt).toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' })}
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}
            {todayAiArticles.length > 0 && (
              <div>
                <p className="mb-2 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider">今日 AI 解读</p>
                <div className="space-y-1.5">
                  {todayAiArticles.map((a) => (
                    <button
                      key={a.id}
                      onClick={() => {
                        setTracesOpen(false);
                        requestOpenArticle(a.id);
                      }}
                      className="w-full flex items-center gap-2 text-left px-3 py-2 rounded-lg bg-[var(--bg-surface)] border border-[var(--border-default)] hover:border-[var(--accent-border)] transition-colors"
                    >
                      <BookOpen size={12} className="text-[var(--success)] shrink-0" />
                      <span className="flex-1 text-xs text-[var(--text-secondary)] truncate">{a.title}</span>
                    </button>
                  ))}
                </div>
              </div>
            )}
            {todayPicks.length > 0 && (
              <div>
                <p className="mb-2 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider">今日 Top5 入选</p>
                <div className="space-y-1.5">
                  {todayPicks.map((p) => (
                    <button
                      key={p.id}
                      onClick={() => {
                        setTracesOpen(false);
                        setSelectedItemId(p.item.id);
                      }}
                      className="w-full flex items-center gap-2 text-left px-3 py-2 rounded-lg bg-[var(--bg-surface)] border border-[var(--border-default)] hover:border-[var(--accent-border)] transition-colors"
                    >
                      <span className="text-xs text-[var(--gold)] font-bold shrink-0">
                        {pickCategoryLabel[p.category] ?? p.category}
                      </span>
                      <span className="flex-1 text-xs text-[var(--text-secondary)] truncate">{p.item.title}</span>
                      {p.aiScore != null && (
                        <span className="shrink-0">
                          <ScoreBadge score={p.aiScore} />
                        </span>
                      )}
                    </button>
                  ))}
                </div>
              </div>
            )}
        </div>
      </div>
    </section>
  );

  return (
    <div ref={workspaceRef} className="relative flex h-full min-w-0 overflow-hidden">
      {/* 左侧文档列表（NB-14 可折叠，折叠态持久化 settings） */}
      {listCollapsed ? (
        <div className="w-10 border-r border-[var(--border-default)] bg-[var(--bg-sunken)] flex flex-col items-center pb-3 shrink-0">
          {/* NB-15b 首行固定：展开钮留在首行（与展开态折叠钮同一行），位置不随折叠跳变 */}
          <div className="h-10 w-full flex items-center justify-center shrink-0" data-tauri-drag-region>
            <button
              onClick={toggleListCollapsed}
              title={`展开列表（${listDocs.length}）`}
              className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)]"
            >
              <PanelLeftOpen size={15} />
            </button>
          </div>
          <span className="mt-3 text-xs text-[var(--text-tertiary)] [writing-mode:vertical-rl] tracking-widest select-none">
            {listTitle} · {listDocs.length}
          </span>
        </div>
      ) : (
      <div className="w-72 border-r border-[var(--border-default)] bg-[var(--bg-sunken)] flex flex-col shrink-0">
        {/* NB-15 首行：统一 h-10 与侧栏首行（红绿灯行）同高对齐，标题 + 折叠钮（规格与侧栏一致 w-7/icon15） */}
        <div className="h-10 border-b border-[var(--border-default)] flex items-center justify-between px-3 shrink-0" data-tauri-drag-region>
          {/* NB-20：drag-region 按事件 target 匹配，标题文字本身也要挂属性才可拖 */}
          <h3 className="text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider" data-tauri-drag-region>{listTitle}</h3>
          <button
            onClick={toggleListCollapsed}
            title="折叠列表"
            className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-surface)] hover:text-[var(--text-secondary)] shrink-0"
          >
            <PanelLeftClose size={15} />
          </button>
        </div>
        <div className="p-3 border-b border-[var(--border-default)] space-y-2">
          {journal && (
            <MonthlyActivityCalendar
              month={calendarMonth}
              selectedDate={selectedDate}
              activityCounts={activityCounts}
              selectedNoteCount={dateFilteredDocs.length}
              onMonthChange={setCalendarMonth}
              onSelectDate={selectCalendarDate}
              onClearDate={() => {
                setSelectedDate(null);
                setBatchIds(new Set());
              }}
              onToday={returnCalendarToToday}
            />
          )}
          {newDocLabel && (
            <div className="relative">
              <div className="flex gap-1">
                <button
                  onClick={handleNew}
                  title="新建空白笔记"
                  className="flex-1 flex items-center justify-center gap-1.5 px-3 py-2 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] text-[var(--text-secondary)] text-xs font-medium hover:border-[var(--accent-border)] hover:text-[var(--accent)] transition-colors"
                >
                  <FilePlus2 size={13} /> {newDocLabel}
                </button>
                {/* NB-07：模板下拉（Obsidian Templates 对标）——主按钮保持空白直建，习惯零回归 */}
                {enableTemplates && (
                  <button
                    onClick={() => setTemplateMenuOpen((v) => !v)}
                    title="从模板新建"
                    className={`flex items-center px-1.5 rounded-lg border transition-colors ${
                      templateMenuOpen
                        ? 'border-[var(--accent-border)] text-[var(--accent)] bg-[var(--accent-subtle)]'
                        : 'border-[var(--border-default)] bg-[var(--bg-surface)] text-[var(--text-tertiary)] hover:border-[var(--accent-border)] hover:text-[var(--accent)]'
                    }`}
                  >
                    <ChevronDown size={13} />
                  </button>
                )}
              </div>
              {enableTemplates && templateMenuOpen && (
                <>
                  {/* 透明遮罩：点击菜单外任意处关闭 */}
                  <div className="fixed inset-0 z-40" onClick={() => setTemplateMenuOpen(false)} />
                  <div className="absolute left-0 right-0 top-full mt-1 z-50 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] py-1 max-h-64 overflow-y-auto">
                    <button
                      onClick={handleNew}
                      className="w-full flex items-center gap-1.5 text-left px-3 py-2 text-xs font-medium text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] transition-colors"
                    >
                      <FilePlus2 size={12} className="text-[var(--text-tertiary)]" /> 空白笔记
                    </button>
                    {templates.length === 0 ? (
                      <p className="px-3 py-2 text-xs text-[var(--text-tertiary)] leading-relaxed">
                        还没有模板：给任意笔记加上{' '}
                        <code className="px-1 py-0.5 rounded-[6px] bg-[var(--bg-sunken)] font-mono text-[13px] text-[var(--accent)]">
                          #template
                        </code>
                        {' 标签，它就成为模板（支持 {{title}} / {{date}} / {{time}} 变量）'}
                      </p>
                    ) : (
                      <>
                        <p className="px-3 pt-2 pb-1 text-xs font-medium text-[var(--text-tertiary)] uppercase tracking-wider">
                          从模板新建
                        </p>
                        {templates.map((t) => (
                          <button
                            key={t.id}
                            onClick={() => void handleNewFromTemplate(t)}
                            className="w-full text-left px-3 py-1.5 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--accent)] transition-colors truncate"
                            title={t.title}
                          >
                            {t.title}
                          </button>
                        ))}
                      </>
                    )}
                    {enableStarterExamples && (
                      <>
                        <div className="my-1 border-t border-[var(--border-default)]" />
                        <button
                          type="button"
                          onClick={() => void handleImportExamples()}
                          disabled={importingExamples}
                          className="w-full flex items-start gap-2 px-3 py-2 text-left text-xs text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] hover:text-[var(--accent)] disabled:opacity-60"
                        >
                          {importingExamples ? <Loader2 size={13} className="mt-0.5 shrink-0 animate-spin" /> : <BookOpenText size={13} className="mt-0.5 shrink-0" />}
                          <span><span className="block font-medium">导入功能范例{remainingExampleCount > 0 ? `（${remainingExampleCount} 篇）` : ''}</span><span className="mt-0.5 block leading-5 text-[var(--text-tertiary)]">Markdown、大纲、任务、双链、搜索与模板；只补缺失项。</span></span>
                        </button>
                      </>
                    )}
                  </div>
                </>
              )}
            </div>
          )}
          <div className="relative">
            <Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="搜索标题或内容…"
              className="w-full text-xs pl-7 pr-2 py-1.5 rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] text-[var(--text-secondary)] placeholder:text-[var(--text-disabled)] focus:outline-none focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
            />
          </div>
          {genMessage && <p className="text-xs px-1 text-[var(--text-tertiary)]">{genMessage}</p>}
          {showTags && topTags.length > 0 && (
            <div className="flex flex-wrap gap-1">
              {topTags.map(([t, n]) => (
                <button
                  key={t}
                  onClick={() => setSelectedTag(selectedTag === t ? null : t)}
                  className={`flex items-center gap-0.5 text-xs px-1.5 py-0.5 rounded-full border transition-colors ${
                    selectedTag === t
                      ? 'border-[var(--accent)] bg-[var(--accent-subtle)] text-[var(--accent)]'
                      : 'border-[var(--border-default)] text-[var(--text-tertiary)] hover:border-[var(--accent-border)]'
                  }`}
                >
                  <Hash size={9} /> {t}
                  {n > 1 && <span className="opacity-60">{n}</span>}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex-1 overflow-y-auto p-2 space-y-1" data-perf-scroll="doc-list">
          {listDocs.length === 0 && (
            <p className="text-xs text-[var(--text-tertiary)] text-center py-10 px-4">
              {journal && selectedDate ? `${selectedDate} 暂无笔记` : emptyHint}
            </p>
          )}
          {listDocs.map((a) => {
            const badge =
              a.articleType === 'journal' && a.title === today
                ? { text: '今日', cls: typeBadge.journal.cls }
                : typeBadge[a.articleType] || typeBadge['deep-dive'];
            const tags = collectTags(a).slice(0, 3);
            // NB-08：搜索态列表卡片显示「命中上下文片段 + 高亮 + 命中数」（Obsidian 搜索面板对标）
            const sq = search.trim();
            const hits = sq ? countMatches(a.content, sq) : 0;
            const sctx = sq && hits > 0 ? firstMatchContext(a.content, sq) : null;
            const metaLine = (
              <div className="flex items-center gap-1.5 mt-1 flex-wrap">
                <span className={`text-xs px-1.5 py-0.5 rounded-[6px] ${badge.cls}`}>{badge.text}</span>
                {a.edited && (
                  <span className="text-xs px-1.5 py-0.5 rounded-[6px] bg-[var(--warning-subtle)] text-[var(--warning)]">已编辑</span>
                )}
                {hits > 0 && (
                  <span className="text-xs text-[var(--gold)] font-medium">{hits} 处命中</span>
                )}
                {tags.map((t) => (
                  <span key={t} className="text-xs text-[var(--accent)]">#{t}</span>
                ))}
                <span className="text-xs text-[var(--text-tertiary)]">
                  {new Date(a.createdAt).toLocaleDateString('zh-CN')}
                </span>
              </div>
            );
            // 行内重命名：点哪儿改哪儿（input 不能嵌 button，改名态整行换 div）
            if (renamingDocId === a.id) {
              return (
                <div
                  key={a.id}
                  onContextMenu={(e) => e.preventDefault()}
                  className="w-full text-left p-3 rounded-lg bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]"
                >
                  <input
                    autoFocus
                    value={renameDocDraft}
                    onChange={(e) => setRenameDocDraft(e.target.value)}
                    onFocus={(e) => e.currentTarget.select()}
                    onClick={(e) => e.stopPropagation()}
                    onKeyDown={(e) => {
                      e.stopPropagation();
                      if (e.key === 'Enter') void submitRenameDoc(a);
                      if (e.key === 'Escape') {
                        renameDocDoneRef.current = true;
                        setRenamingDocId(null);
                      }
                    }}
                    onBlur={() => void submitRenameDoc(a)}
                    className="w-full px-1.5 py-0.5 text-[13px] font-medium rounded-md border border-[var(--accent)] focus:outline-none focus:shadow-[0_0_0_3px_var(--accent-subtle)] bg-[var(--bg-surface)] text-[var(--text-primary)]"
                  />
                  {metaLine}
                </div>
              );
            }
            return (
              <button
                key={a.id}
                onClick={(e) => void handleListClick(e, a)}
                onContextMenu={(e) => openDocMenu(e, a)}
                className={`w-full text-left p-3 rounded-lg transition-colors ${
                  batchIds.has(a.id)
                    ? 'bg-[var(--accent-subtle)] ring-1 ring-[var(--accent-border)]'
                    : selected?.id === a.id
                      ? 'bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]'
                      : 'hover:bg-[var(--bg-surface)]'
                }`}
              >
                <p className="text-[13px] font-medium text-[var(--text-primary)] line-clamp-2">
                  <SearchHighlight text={a.title} query={sq} />
                </p>
                {sctx && (
                  <p className="text-xs text-[var(--text-tertiary)] line-clamp-2 mt-1">
                    <SearchHighlight
                      text={`${sctx.before}${sctx.match}${sctx.after}`}
                      query={sq}
                    />
                  </p>
                )}
                {metaLine}
              </button>
            );
          })}
        </div>
        {/* NB-30：批量操作栏（Shift+范围选 / ⌘ 切换勾选后出现） */}
        {batchIds.size > 0 && (
          <div className="border-t border-[var(--border-default)] bg-[var(--bg-surface)] px-3 py-2 flex items-center gap-2 shrink-0">
            <span className="flex-1 text-xs text-[var(--text-tertiary)]">
              已选 <span className="font-bold text-[var(--accent)]">{batchIds.size}</span> 篇
            </span>
            <button
              onClick={() => setBatchIds(new Set())}
              className="text-xs px-2 py-1 rounded-md text-[var(--text-tertiary)] hover:text-[var(--text-primary)] hover:bg-[var(--bg-sunken)] transition-colors"
              title="取消选择（Esc）"
            >
              取消
            </button>
            <button
              onClick={() => void handleBatchDelete()}
              className={`flex items-center gap-1 text-xs px-2.5 py-1 rounded-md font-medium transition-colors ${
                batchConfirm
                  ? 'bg-[var(--danger)] text-white hover:opacity-90'
                  : 'text-[var(--danger)] hover:bg-[var(--danger-subtle)]'
              }`}
              title={batchConfirm ? '再次点击确认删除' : '删除选中笔记'}
            >
              <Trash2 size={11} />
              {batchConfirm ? '确认删除？' : `删除 ${batchIds.size} 篇`}
            </button>
          </div>
        )}
      </div>
      )}

      {/* 右侧阅读/编辑区：NB-23 共享书写工作台（与 AI 工作室项目模式同一实现） */}
      <div className="relative flex-1 flex flex-col overflow-hidden">
        {selected ? (
          <>
            <div
              className="flex flex-1 min-h-0"
              aria-hidden={tracesOpen}
              inert={tracesOpen ? true : undefined}
            >
              <NoteWorkbench
                ref={wbRef}
                article={selected}
                onOpenDocument={(d, h, b) => void wbOpenDocument(d, h, b)}
                onCreateDocument={(t) => void wbCreateDocument(t)}
                onDelete={() => void handleDelete()}
                onAddSelectionToChat={captureChatSelection}
                moreMenuItems={journal ? [{
                  id: 'today-traces',
                  label: `今日痕迹 · ${traceCount}`,
                  icon: <Sparkles size={12} className="text-[var(--gold)]" />,
                  onSelect: () => setTracesOpen(true),
                }] : []}
                belowHeader={
              <>
                {changeStatusBar}
                {isTodayJournal && (
                /* N2 快速捕获栏（仅今日 journal）：Enter 追加进「## 速记」 */
                <div className="px-6 py-2 border-b border-[var(--border-default)] bg-[var(--bg-surface)] flex items-center gap-2 shrink-0">
                  <Zap size={13} className="text-[var(--gold)] shrink-0" />
                  <input
                    value={captureText}
                    onChange={(e) => setCaptureText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        void handleCapture();
                      }
                    }}
                    placeholder="速记一笔，Enter 存进今天的「## 速记」…"
                    className="flex-1 min-w-0 bg-transparent text-xs text-[var(--text-secondary)] placeholder:text-[var(--text-disabled)] focus:outline-none"
                  />
                  <button
                    onClick={() => void handleCapture()}
                    disabled={!captureText.trim()}
                    className="text-xs px-2.5 py-1 rounded-md bg-[var(--gold)] text-white font-medium hover:bg-[var(--warning)] disabled:opacity-40 transition-colors shrink-0"
                  >
                    记一笔
                  </button>
                </div>
                )}
              </>
                }
                suggestion={suggestion}
                onSuggestionDecision={(target, decision) => void decideSuggestion(target, decision)}
                statusExtra={
              <>
                {/* NB-04：快捷键参考卡（preset 自带键位体系的可见化入口） */}
                <ShortcutHelp />
                {/* NB-11：性能探针开关（宿主走查输入/滚动性能用） */}
                <button
                  onClick={() => setPerfOpen((v) => !v)}
                  className={`flex items-center gap-1 transition-colors ${
                    perfOpen ? 'text-[var(--accent)]' : 'hover:text-[var(--accent)]'
                  }`}
                  title="显示/隐藏性能探针（FPS + 心跳/分屏/扫描耗时）"
                >
                  <Activity size={11} /> 性能
                </button>
                {/* NEXT-001：性能夹具面板入口（语料播种 + 场景化 P50/P95 测量） */}
                <button
                  onClick={() => useAppStore.getState().setPerfFixtureOpen(true)}
                  className="flex items-center gap-1 transition-colors hover:text-[var(--accent)]"
                  title="打开性能夹具（200 篇固定语料 + 冷/热页签、A/B 切换、输入延迟测量）"
                >
                  <Gauge size={11} /> 夹具
                </button>
              </>
                }
              />
            </div>
            {tracePanel}
          </>
        ) : (
          <div className="flex-1 flex flex-col items-center justify-center">
            <EmptyState
              icon={BookOpen}
              title={emptyHint}
              action={enableStarterExamples ? (
                <div className="flex items-center gap-4">
                  <button type="button" onClick={() => void handleNew()} className="text-sm text-[var(--accent)] hover:underline">新建第一篇笔记</button>
                  <button type="button" onClick={() => void handleImportExamples()} disabled={importingExamples} className="inline-flex items-center gap-1.5 text-sm text-[var(--text-secondary)] hover:text-[var(--accent)] disabled:opacity-60">
                    {importingExamples ? <Loader2 size={14} className="animate-spin" /> : <BookOpenText size={14} />}导入 6 篇功能范例
                  </button>
                </div>
              ) : undefined}
            />
          </div>
        )}
      </div>
      {!agentCollapsed && !agentOverlay && (
        <VerticalResizeHandle
          value={visibleAgentWidth}
          min={MIN_NOTES_AGENT_WIDTH}
          max={maxAgentWidth}
          defaultValue={DEFAULT_NOTES_AGENT_WIDTH}
          direction={-1}
          onChange={setAgentWidth}
          onCommit={(width) => void updateSetting(NOTES_AGENT_WIDTH_KEY, String(Math.round(width)))}
          label="调整笔记本会话宽度"
        />
      )}
      {!agentCollapsed && (
        <aside
          className={agentOverlay
            ? 'absolute inset-y-0 right-0 z-20 flex bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]'
            : 'shrink-0 flex bg-[var(--bg-surface)]'}
          style={{
            width: `${agentOverlay
              ? Math.min(DEFAULT_NOTES_AGENT_WIDTH, Math.max(320, workspaceWidth - 48))
              : visibleAgentWidth}px`,
          }}
        >
          <ProjectChatPanel
            projectId={null}
            changeScopeId="notebook"
            selection={chatSelection}
            selectionLines={chatSelectionLines}
            onClearSelection={() => {
              setChatSelection(null);
              setChatSelectionLines(null);
            }}
            activeDocumentId={selected?.id ?? null}
            activeDocumentTitle={selected?.title ?? null}
            resolveActiveDocumentContext={async () => {
              const doc = selected;
              if (!doc || selectedDocIdRef.current !== doc.id) return null;
              if (wbRef.current?.needsFlush() && !(await wbRef.current.flush())) return null;
              const baseVersion = await documentCurrentVersion(doc.id);
              if (selectedDocIdRef.current !== doc.id) return null;
              return {
                articleId: doc.id,
                title: doc.title,
                baseVersion,
                markdown: wbRef.current?.getLiveMarkdown() ?? doc.content ?? '',
              };
            }}
            emptyHint="选中笔记内容并点击“加入会话”，让 Hermes 基于明确范围继续处理。"
            composerPlaceholder={chatSelection ? '对选中内容下指令…' : '向 Hermes 提问，或先选中一段笔记…'}
          />
        </aside>
      )}
      {/* NB-11：性能探针面板（fixed 定位挂工作台根，不占布局） */}
      {perfOpen && <PerfOverlay />}
      {/* NB-13：列表右键上下文菜单（fixed 定位 + mask，挂工作台根） */}
      <ContextMenu menu={ctxMenu} onClose={() => setCtxMenu(null)} />
    </div>
  );
}
