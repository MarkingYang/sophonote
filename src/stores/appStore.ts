import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { Item, Source, Collection, Task, PomodoroSession, AppSettings, Article } from '../types';
import * as tauri from '../services/tauri';
import { generateEmbedding, chunkText, PROMPT_VERSIONS, composeEnrichSummary, type EnrichResult } from '../services/ai';
import { runHermesDiscoveryAnalysis } from '../services/discoveryAgent';
import { buildSedimentMarkdown } from '../services/sediment';
import { toggleTaskLine } from '../services/noteTasks';
import { renameWikilinks, containsWikilinkTo } from '../services/noteLinks';

/** N3：语义搜索统一结果（条目通道 + 笔记通道合并，按 distance 排序） */
export type SemanticHit =
  | { kind: 'item'; item: Item; distance: number; snippet?: string }
  | { kind: 'note'; noteId: string; title: string; articleType: Article['articleType']; distance: number; snippet: string };

/** N3：单篇笔记的 chunk 嵌入构建——chunk 0 = 标题，其后为正文分片（≤8 片） */
async function buildNoteChunks(a: Article): Promise<tauri.ChunkInput[]> {
  const texts: string[] = [a.title, ...chunkText(a.content, 800).slice(0, 8)];
  const inputs: tauri.ChunkInput[] = [];
  for (let idx = 0; idx < texts.length; idx++) {
    inputs.push({ idx, text: texts[idx], vector: await generateEmbedding(texts[idx]) });
  }
  return inputs;
}

// N3：笔记索引防抖定时器（保存写路径共用，停止输入 5s 后才真正嵌入，避免心跳连写打爆嵌入 API）
const noteIndexTimers = new Map<string, ReturnType<typeof setTimeout>>();

interface AppState {
  // 数据源
  sources: Source[];
  setSources: (sources: Source[]) => void;
  loadSources: () => Promise<void>;
  toggleSource: (id: string) => Promise<void>;
  updateSourceInterval: (id: string, minutes: number) => Promise<void>;
  updateSourceDiscoveryConfig: (id: string, generationPrompt: string, scoringRule: string, minScore: number) => Promise<void>;
  updateSourceTier: (id: string, tier: Source['tier']) => Promise<void>;
  updateSourceAdmission: (id: string, admission: Source['admission']) => Promise<void>;

  // 内容条目
  items: Item[];
  setItems: (items: Item[]) => void;
  addItems: (items: Item[]) => void;
  loadItems: () => Promise<void>;
  enrichItem: (id: string, fallbackItem?: Item) => Promise<void>;
  /** 手动编辑速览（结构化或纯文本）并落库，prompt_version 标记 manual-edit */
  saveItemAIEdit: (id: string, tags: string[], enrich: EnrichResult | null, plainSummary: string) => Promise<void>;
  updateItemStatus: (id: string, status: Item['status']) => Promise<void>;
  starItem: (id: string) => Promise<void>;
  archiveItem: (id: string) => Promise<void>;
  deleteItem: (id: string) => Promise<void>;

  // 收藏夹
  collections: Collection[];
  addCollection: (collection: Collection) => void;

  // 任务
  tasks: Task[];
  loadTasks: () => Promise<void>;
  addTask: (task: Task) => Promise<void>;
  toggleTask: (id: string) => Promise<void>;
  deleteTask: (id: string) => Promise<void>;

  // 番茄钟专注会话（DEC-034）
  pomodoroSessions: PomodoroSession[];
  loadPomodoroSessions: () => Promise<void>;
  addPomodoroSession: (session: PomodoroSession) => Promise<void>;

  // 文章（深度解读）
  articles: Article[];
  loadArticles: () => Promise<void>;
  /** 将按 itemId 精确读取的文章合并进有界列表缓存。 */
  upsertArticle: (article: Article) => void;
  saveArticle: (article: Article) => Promise<void>;
  updateArticleContent: (id: string, content: string) => Promise<void>;
  updateArticleTitle: (id: string, title: string) => Promise<void>;
  deleteArticle: (id: string) => Promise<void>;
  /** N1 一键沉淀：条目 → 笔记本笔记（自动携带来源反链与关键证据）；
   * 已沉淀过则直接打开既有笔记（防重复）。完成后跳转笔记本并打开该笔记 */
  sedimentToNote: (itemId: string) => Promise<{ created: boolean; noteId: string } | null>;

  // 设置
  settings: AppSettings;
  updateSettings: (settings: Partial<AppSettings>) => void;
  loadSettings: () => Promise<void>;

  // API Key 状态（值只取 configured/空串；真实 Key 永不跨 IPC 进入 WebView）
  apiKeys: Record<string, string>;
  setApiKey: (provider: string, key: string) => Promise<void>;
  // 惰性读取配置状态；apiKeys 里有记录（含空串）即不重复访问钥匙串
  ensureApiKeyLoaded: (provider: string) => Promise<void>;

  // UI状态
  activePage: string;
  setActivePage: (page: string) => void;
  /** DEC-041：工具库导航状态；null = 工具库画廊，否则为打开中的工具 id */
  activeToolId: string | null;
  openTool: (id: string | null) => void;
  /** NEXT-001：性能夹具面板开关（App 级浮层，跨页签常驻） */
  perfFixtureOpen: boolean;
  setPerfFixtureOpen: (open: boolean) => void;
  searchQuery: string;
  setSearchQuery: (query: string) => void;
  selectedItemId: string | null;
  setSelectedItemId: (id: string | null) => void;
  /** ⌘K 快速切换器选中的待打开文档：按 articleType 跳到笔记/解读空间后由 DocWorkspace 消费 */
  pendingArticleId: string | null;
  requestOpenArticle: (id: string) => void;
  clearPendingArticle: () => void;
  /** N5：打开文档后要定位的源码行（1-based，hb-line-N 锚点），与 pendingArticleId 配套由 DocWorkspace 消费 */
  pendingAnchorLine: number | null;
  /** N5：打开文档并定位到指定行（Tasks 页行级回链） */
  openArticleAtLine: (id: string, line: number) => void;
  clearPendingAnchor: () => void;
  /** N5：翻转笔记中某任务行的勾选（直接写回源 .md 真相源，双向同步天然成立） */
  toggleNoteTask: (articleId: string, line: number) => Promise<void>;

  // 统计
  stats: {
    totalItems: number;
    unreadItems: number;
    starredItems: number;
    totalTasks: number;
    pendingTasks: number;
    totalLogs: number;
  };
  refreshStats: () => Promise<void>;

  // 语义搜索
  semanticResults: SemanticHit[] | null;   // null = 未处于语义搜索模式
  semanticSearching: boolean;
  indexing: { done: number; total: number } | null;
  semanticSearch: (query: string) => Promise<void>;
  clearSemanticResults: () => void;
  indexAllItems: () => Promise<number>;  // 返回失败条数（条目 + 笔记统一索引）
  /** N3：笔记写路径的防抖增量索引（保存/改名后调用，5s 静默期后嵌入） */
  scheduleNoteIndex: (noteId: string) => void;

  // 初始化
  initialized: boolean;
  initialize: () => Promise<void>;
}

const defaultSettings: AppSettings = {
  theme: 'system',
  language: 'zh',
  autoFetch: true,
  fetchIntervalHours: 6,
  notificationEnabled: true,
  aiConfig: {
    activeProvider: 'deepseek',
    providers: {
      deepseek: {
        id: 'deepseek',
        name: 'DeepSeek',
        protocol: 'openai',
        baseUrl: 'https://api.deepseek.com/v1',
        model: 'deepseek-v4-pro',
        models: ['deepseek-v4-pro', 'deepseek-v4-flash'],
      },
      kimi: {
        id: 'kimi',
        name: 'Kimi / 月之暗面',
        protocol: 'openai',
        baseUrl: 'https://api.moonshot.cn/v1',
        model: 'kimi-k3',
        models: ['kimi-k3', 'kimi-k2.7-code', 'kimi-k2.6', 'kimi-k2.5'],
      },
    },
  },
  semanticSearchEnabled: true,
};

// Rust 侧 topics/ai_tags 以逗号字符串返回，统一转成数组
function normalizeItem(i: any): Item {
  return {
    ...i,
    topics: typeof i.topics === 'string' ? i.topics.split(',').filter(Boolean) : (i.topics ?? []),
    aiTags: typeof i.aiTags === 'string' ? i.aiTags.split(',').filter(Boolean) : (i.aiTags ?? []),
  };
}

// StrictMode 下 effect 双跑防并发初始化：模块级 in-flight 锁
let initInFlight: Promise<void> | null = null;

export const useAppStore = create<AppState>()(
  persist(
    (set, get) => ({
      sources: [],
      setSources: (sources) => set({ sources }),
      loadSources: async () => {
        try {
          const sources = await tauri.getSources();
          set({ sources: sources.map(s => {
            let config: Record<string, unknown> = typeof s.config === 'object' && s.config !== null ? (s.config as Record<string, unknown>) : {};
            if (typeof s.config === 'string') {
              try { config = JSON.parse(s.config); } catch { config = {}; }
            }
            return {
              ...s,
              config,
              createdAt: new Date().toISOString(), // fallback
            };
          }) });
        } catch (e) {
          console.error('Failed to load sources:', e);
        }
      },
      toggleSource: async (id) => {
        // Optimistic update
        set((state) => ({
          sources: state.sources.map((s) =>
            s.id === id ? { ...s, enabled: !s.enabled } : s
          ),
        }));
        try {
          await tauri.toggleSource(id);
        } catch (e) {
          console.error('Failed to toggle source:', e);
          // Revert on error
          await get().loadSources();
        }
      },
      updateSourceInterval: async (id, minutes) => {
        // Optimistic update
        set((state) => ({
          sources: state.sources.map((s) =>
            s.id === id ? { ...s, fetchIntervalMinutes: minutes } : s
          ),
        }));
        try {
          await tauri.updateSourceInterval(id, minutes);
        } catch (e) {
          console.error('Failed to update interval:', e);
          await get().loadSources();
        }
      },
      updateSourceDiscoveryConfig: async (id, generationPrompt, scoringRule, minScore) => {
        const config = { generationPrompt, scoringRule, minScore };
        set((state) => ({
          sources: state.sources.map((source) => (
            source.id === id ? { ...source, config: { ...source.config, ...config } } : source
          )),
        }));
        try {
          await tauri.updateSourceDiscoveryConfig(id, generationPrompt, scoringRule, minScore);
        } catch (error) {
          console.error('Failed to update source discovery config:', error);
          await get().loadSources();
          throw error;
        }
      },

      // 信源分层 / 准入状态（借鉴 ai-news-radar source_tier + 观察区）
      updateSourceTier: async (id, tier) => {
        set((state) => ({
          sources: state.sources.map((s) => (s.id === id ? { ...s, tier } : s)),
        }));
        try {
          await tauri.updateSourceTier(id, tier);
        } catch (e) {
          console.error('Failed to update source tier:', e);
          await get().loadSources();
        }
      },
      updateSourceAdmission: async (id, admission) => {
        set((state) => ({
          sources: state.sources.map((s) => (s.id === id ? { ...s, admission } : s)),
        }));
        try {
          await tauri.updateSourceAdmission(id, admission);
          // 准入状态影响默认视图条目集合（probation/skipped 不出现在列表）
          await get().loadItems();
        } catch (e) {
          console.error('Failed to update source admission:', e);
          await get().loadSources();
        }
      },

      items: [],
      setItems: (items) => set({ items }),
      addItems: (newItems) =>
        set((state) => {
          const existingIds = new Set(state.items.map((i) => i.id));
          const filtered = newItems.filter((i) => !existingIds.has(i.id));
          return { items: [...filtered, ...state.items] };
        }),
      loadItems: async () => {
        try {
          // DEC-028：全局缓存只保留 SQLite 已排除过期后的 7 天活动池，不再固定截断 300 条。
          const raw = await tauri.getItems();
          set({
            items: raw.map((i: any) => ({
              ...i,
              topics: typeof i.topics === 'string' ? i.topics.split(',').filter(Boolean) : (i.topics ?? []),
              aiTags: typeof i.aiTags === 'string' ? i.aiTags.split(',').filter(Boolean) : (i.aiTags ?? []),
            })),
          });
        } catch (e) {
          console.error('Failed to load items:', e);
        }
      },
      enrichItem: async (id, fallbackItem) => {
        const item = get().items.find((i) => i.id === id) ?? fallbackItem;
        if (!item) return;

        // P0-4 调用链：检查 item_contents → 没有正文先抓 → quality>=2 才调用 AI
        const content = await tauri.getItemContent(id);
        if (content?.status === 'unsupported') {
          throw new Error(content.errorMessage || '该来源证据不足，暂不生成解读');
        }
        if (!content || content.qualityLevel < 2 || !content.evidenceJson) {
          throw new Error('正文证据不足（未获取到有效正文），无法生成解读');
        }

        // content_hash 未变化且已有解读 → 不重复生成
        const hashKey = `enrich_hash:${id}`;
        const lastHash = await tauri.getSetting(hashKey).catch(() => '');
        if (item.aiSummary && content.contentHash && lastHash === content.contentHash) {
          return;
        }

        await runHermesDiscoveryAnalysis(id, 'quick');
        const updated = (await tauri.getItems()).find((candidate) => candidate.id === id);
        if (!updated?.aiSummary) {
          throw new Error('Hermes Agent 已结束，但没有保存速览结果');
        }
        const updatedTags = typeof (updated as unknown as { aiTags?: unknown }).aiTags === 'string'
          ? ((updated as unknown as { aiTags: string }).aiTags.split(',').filter(Boolean))
          : (updated.aiTags ?? []);
        if (content.contentHash) {
          await tauri.updateSetting(hashKey, content.contentHash);
        }
        set((state) => ({
          items: state.items.map((i) =>
            i.id === id ? { ...i, aiSummary: updated.aiSummary, aiTags: updatedTags } : i
          ),
        }));
      },
      saveItemAIEdit: async (id, tags, enrich, plainSummary) => {
        const summary = enrich ? composeEnrichSummary(enrich) : plainSummary;
        await tauri.updateItemAI(
          id,
          summary,
          tags,
          PROMPT_VERSIONS.manualEdit,
          enrich ? JSON.stringify(enrich) : undefined
        );
        set((state) => ({
          items: state.items.map((i) =>
            i.id === id ? { ...i, aiSummary: summary, aiTags: tags } : i
          ),
        }));
      },

      updateItemStatus: async (id, status) => {
        set((state) => ({
          items: state.items.map((i) => (i.id === id ? { ...i, status } : i)),
        }));
        try {
          await tauri.updateItemStatus(id, status);
        } catch (e) {
          console.error('Failed to update item status:', e);
        }
      },
      starItem: async (id) => {
        const item = get().items.find((i) => i.id === id);
        const newStatus = item?.status === 'starred' ? 'unread' : 'starred';
        set((state) => ({
          items: state.items.map((i) =>
            i.id === id ? { ...i, status: newStatus } : i
          ),
        }));
        try {
          await tauri.updateItemStatus(id, newStatus);
        } catch (e) {
          console.error('Failed to star item:', e);
        }
      },
      archiveItem: async (id) => {
        set((state) => ({
          items: state.items.map((i) =>
            i.id === id ? { ...i, status: 'archived' } : i
          ),
        }));
        try {
          await tauri.updateItemStatus(id, 'archived');
        } catch (e) {
          console.error('Failed to archive item:', e);
        }
      },
      deleteItem: async (id) => {
        // 乐观移除；后端会连带清理收藏夹引用、关联文章与向量索引
        set((state) => ({
          items: state.items.filter((i) => i.id !== id),
          semanticResults: state.semanticResults
            ? state.semanticResults.filter((h) => h.kind !== 'item' || h.item.id !== id)
            : null,
        }));
        try {
          await tauri.deleteItem(id);
          await get().refreshStats();
        } catch (e) {
          console.error('Failed to delete item:', e);
          await get().loadItems();
        }
      },

      collections: [
        { id: 'favorites', name: '收藏夹', icon: '⭐', color: '#fbbf24', createdAt: new Date().toISOString() },
        { id: 'ai-models', name: 'AI模型', icon: '🤖', color: '#8b5cf6', createdAt: new Date().toISOString() },
        { id: 'architecture', name: '架构设计', icon: '🏗️', color: '#06b6d4', createdAt: new Date().toISOString() },
        { id: 'products', name: '产品分析', icon: '📱', color: '#f472b6', createdAt: new Date().toISOString() },
      ],
      addCollection: (collection) =>
        set((state) => ({ collections: [...state.collections, collection] })),

      tasks: [],
      pomodoroSessions: [],
      loadTasks: async () => {
        try {
          const tasks = await tauri.getTasks();
          set({ tasks: tasks.map(t => ({
            ...t,
            recurring: (t.recurring as 'daily' | 'weekly' | 'none' | undefined) ?? 'none',
          })) });
        } catch (e) {
          console.error('Failed to load tasks:', e);
        }
      },
      addTask: async (task) => {
        set((state) => ({ tasks: [...state.tasks, task] }));
        try {
          await tauri.insertTask(task as any);
        } catch (e) {
          console.error('Failed to save task:', e);
        }
      },
      toggleTask: async (id) => {
        const task = get().tasks.find((t) => t.id === id);
        if (!task) return;
        const newStatus = task.status === 'done' ? 'todo' : 'done';
        const updated = {
          ...task,
          status: newStatus as Task['status'],
          completedAt: newStatus === 'done' ? new Date().toISOString() : undefined,
        };
        set((state) => ({
          tasks: state.tasks.map((t) => (t.id === id ? updated : t)),
        }));
        try {
          await tauri.insertTask(updated as any);
        } catch (e) {
          console.error('Failed to toggle task:', e);
        }
      },
      deleteTask: async (id) => {
        set((state) => ({
          tasks: state.tasks.filter((t) => t.id !== id),
        }));
        try {
          await tauri.deleteTask(id);
        } catch (e) {
          console.error('Failed to delete task:', e);
        }
      },

      loadPomodoroSessions: async () => {
        try {
          const sessions = await tauri.listPomodoroSessions();
          set({ pomodoroSessions: sessions });
        } catch (e) {
          console.error('Failed to load pomodoro sessions:', e);
        }
      },
      addPomodoroSession: async (session) => {
        set((state) => ({
          pomodoroSessions: state.pomodoroSessions.some((s) => s.id === session.id)
            ? state.pomodoroSessions.map((s) => (s.id === session.id ? session : s))
            : [session, ...state.pomodoroSessions],
        }));
        try {
          await tauri.insertPomodoroSession(session);
        } catch (e) {
          console.error('Failed to save pomodoro session:', e);
        }
      },

      articles: [],
      loadArticles: async () => {
        try {
          const articles = await tauri.getArticles(200);
          set({ articles });
        } catch (e) {
          console.error('Failed to load articles:', e);
        }
      },
      upsertArticle: (article) =>
        set((state) => ({
          articles: state.articles.some((a) => a.id === article.id)
            ? state.articles.map((a) => (a.id === article.id ? article : a))
            : [article, ...state.articles],
        })),
      saveArticle: async (article) => {
        // NB-31 失败契约（新建）：先后端落盘，成功才进内存列表并调度索引。
        // 创建失败时 store 不出现该记录，调用方不会跳转到不存在的笔记（防"假新建"）；
        // 异常向上抛出，由调用方决定提示与重试。
        try {
          await tauri.insertArticle(article);
        } catch (e) {
          console.error('Failed to save article:', e);
          throw e;
        }
        set((state) => ({ articles: [article, ...state.articles] }));
        get().scheduleNoteIndex(article.id); // N3：新笔记（沉淀/新建/今日页）纳入语义索引
      },
      updateArticleContent: async (id, content) => {
        // NB-31 失败契约（正文修改）：保留乐观更新（内存是编辑器当下的真相），
        // 但写盘失败必须把异常抛给调用方——由调用方保留 dirty、展示错误并可重试；
        // 索引只在写盘成功后调度，避免失败内容进入语义面。
        set((state) => ({
          articles: state.articles.map((a) =>
            a.id === id ? { ...a, content, edited: true } : a
          ),
        }));
        try {
          await tauri.updateArticle(id, content);
        } catch (e) {
          console.error('Failed to update article:', e);
          throw e;
        }
        get().scheduleNoteIndex(id); // N3：内容变化 → 防抖增量索引
      },
      updateArticleTitle: async (id, title) => {
        // NB-31 失败契约（改名）：乐观更新 + 失败上抛。改名未落盘时不执行双链级联
        // 改写——改名本身未生效，先去改其它文档的 [[旧标题]] 只会制造新的不一致。
        const prevTitle = get().articles.find((a) => a.id === id)?.title;
        set((state) => ({
          articles: state.articles.map((a) => (a.id === id ? { ...a, title } : a)),
        }));
        try {
          await tauri.renameArticle(id, title);
        } catch (e) {
          console.error('Failed to rename article:', e);
          throw e;
        }
        // NB-06 改名同步双链（Obsidian「Update links on file rename」对标）：
        // 全库把 [[旧标题]] 改写为 [[新标题]]（围栏感知，![[嵌入]] 同步覆盖），根治改名死链。
        // NB-10：别名 [[旧|别名]] 与标题链接 [[旧#标题]] 形态同步改写（containsWikilinkTo 三形态扫描）。
        // 不改写被改名文档自身——它可能正挂在编辑器里，心跳会以编辑器快照为准覆盖库值
        // NB-31：级联单篇失败不阻断其余文档（改名本身已成功），仅记录；索引仍在最后调度
        if (prevTitle && prevTitle !== title) {
          const affected = get().articles.filter(
            (a) => a.id !== id && containsWikilinkTo(a.content, prevTitle)
          );
          for (const t of affected) {
            const next = renameWikilinks(t.content, prevTitle, title);
            if (next !== t.content) {
              try {
                await get().updateArticleContent(t.id, next);
              } catch (e) {
                console.error('Failed to sync wikilinks for:', t.title, e);
              }
            }
          }
        }
        get().scheduleNoteIndex(id); // N3：标题是 chunk 0，改名后需重建
      },
      deleteArticle: async (id) => {
        // NB-31 失败契约（删除）：先后端删除，成功才从内存列表移除（防"假删除"）。
        // 失败时记录保留在列表与选中态，异常上抛由调用方提示。
        try {
          await tauri.deleteArticle(id);
        } catch (e) {
          console.error('Failed to delete article:', e);
          throw e;
        }
        set((state) => ({ articles: state.articles.filter((a) => a.id !== id) }));
      },
      sedimentToNote: async (itemId) => {
        const state = get();
        const item = state.items.find((i) => i.id === itemId);
        if (!item) return null;
        // 已沉淀 → 直接打开既有笔记（防重复）
        const existing = state.articles.find((a) => a.itemId === itemId && a.articleType === 'manual');
        if (existing) {
          get().requestOpenArticle(existing.id);
          return { created: false, noteId: existing.id };
        }
        // 取已落库速览与证据（Rust 侧均有缓存，不触发新抓取）
        let enrich: EnrichResult | null = null;
        try {
          const json = await tauri.getItemEnrich(itemId);
          if (json) enrich = JSON.parse(json) as EnrichResult;
        } catch { /* 无结构化速览则走 aiSummary/description 兜底 */ }
        let evidence: { id: string; kind: string; url: string; text: string }[] = [];
        try {
          const c = await tauri.getItemContent(itemId);
          if (c?.evidenceJson) evidence = JSON.parse(c.evidenceJson);
        } catch { /* 证据缺失不阻断沉淀 */ }
        const article: Article = {
          id: crypto.randomUUID(),
          itemId: item.id,
          title: item.title,
          content: buildSedimentMarkdown(item, enrich, evidence),
          articleType: 'manual',
          edited: false,
          createdAt: new Date().toISOString(),
        };
        await get().saveArticle(article);
        get().requestOpenArticle(article.id);
        return { created: true, noteId: article.id };
      },

      settings: defaultSettings,
      updateSettings: async (newSettings) => {
        set((state) => ({ settings: { ...state.settings, ...newSettings } }));
        // Sync AI 配置（供应商列表 + 当前启用指针）到 SQLite；API Key 走 setApiKey → 钥匙串
        if (newSettings.aiConfig !== undefined) {
          try {
            await tauri.updateSetting('ai_config', JSON.stringify(newSettings.aiConfig));
          } catch (e) {
            console.error('Failed to sync ai_config:', e);
          }
        }
      },
      apiKeys: {},
      setApiKey: async (provider, key) => {
        try {
          if (key) {
            await tauri.saveApiKey(provider, key);
          } else {
            await tauri.deleteApiKey(provider);
          }
          set((state) => ({
            apiKeys: { ...state.apiKeys, [provider]: key ? 'configured' : '' },
          }));
        } catch (e) {
          console.error('Failed to sync api key:', e);
          throw e;
        }
      },
      ensureApiKeyLoaded: async (provider) => {
        if (get().apiKeys[provider] !== undefined) return;
        let configured = false;
        try {
          configured = await tauri.hasApiKey(provider);
        } catch {}
        set((state) =>
          state.apiKeys[provider] !== undefined
            ? state
            : { apiKeys: { ...state.apiKeys, [provider]: configured ? 'configured' : '' } }
        );
      },
      loadSettings: async () => {
        // 从 SQLite 读取 AI 配置（与内置预设合并），再从钥匙串逐个读取各供应商的 Key
        try {
          const raw = await tauri.getSetting('ai_config');
          let saved: Partial<AppSettings['aiConfig']> = {};
          try {
            saved = raw ? JSON.parse(raw) : {};
          } catch {
            console.warn('ai_config 持久化数据损坏，使用默认配置');
          }
          const savedProviders =
            saved.providers && Object.keys(saved.providers).length > 0 ? saved.providers : {};
          set((state) => {
            const base = state.settings.aiConfig?.providers ?? defaultSettings.aiConfig.providers;
            return {
              settings: {
                ...state.settings,
                aiConfig: {
                  activeProvider:
                    saved.activeProvider ||
                    state.settings.aiConfig?.activeProvider ||
                    defaultSettings.aiConfig.activeProvider,
                  providers: { ...defaultSettings.aiConfig.providers, ...base, ...savedProviders },
                  lastAgentModelByProvider:
                    saved.lastAgentModelByProvider ??
                    state.settings.aiConfig?.lastAgentModelByProvider,
                  embedding: saved.embedding ?? state.settings.aiConfig?.embedding,
                },
              },
            };
          });
        } catch {
          // 尚未保存过 AI 配置，使用内置预设
          set((state) => ({
            settings: { ...state.settings, aiConfig: defaultSettings.aiConfig },
          }));
        }
        // 注意：API Key 不在此处预读。启动时逐个读钥匙串会连续弹授权框，
        // 改为各功能首次使用时经 ensureApiKeyLoaded 惰性读取。
      },

      activePage: 'discover',
      setActivePage: (page) => set({ activePage: page }),
      activeToolId: null,
      openTool: (id) => set({ activeToolId: id }),
      perfFixtureOpen: false,
      setPerfFixtureOpen: (open) => set({ perfFixtureOpen: open }),
      searchQuery: '',
      setSearchQuery: (query) => set({ searchQuery: query }),
      selectedItemId: null,
      setSelectedItemId: (id) => set({ selectedItemId: id }),
      pendingArticleId: null,
      requestOpenArticle: (id) => {
        const article = get().articles.find((a) => a.id === id);
        if (!article) return;
        set({
          pendingArticleId: id,
          activePage: article.articleType === 'manual' || article.articleType === 'journal' ? 'notes' : 'articles',
        });
      },
      clearPendingArticle: () => set({ pendingArticleId: null }),
      pendingAnchorLine: null,
      openArticleAtLine: (id, line) => {
        set({ pendingAnchorLine: line });
        get().requestOpenArticle(id);
      },
      clearPendingAnchor: () => set({ pendingAnchorLine: null }),
      toggleNoteTask: async (articleId, line) => {
        const a = get().articles.find((x) => x.id === articleId);
        if (!a) return;
        const next = toggleTaskLine(a.content, line);
        if (next === a.content) return;
        await get().updateArticleContent(articleId, next); // 写回真相源；N3 防抖索引随之调度
      },

      stats: {
        totalItems: 0,
        unreadItems: 0,
        starredItems: 0,
        totalTasks: 0,
        pendingTasks: 0,
        totalLogs: 0,
      },
      refreshStats: async () => {
        try {
          const stats = await tauri.getStats();
          set({ stats });
        } catch (e) {
          console.error('Failed to refresh stats:', e);
        }
      },

      semanticResults: null,
      semanticSearching: false,
      indexing: null,
      semanticSearch: async (query) => {
        if (!query.trim()) {
          set({ semanticResults: null });
          return;
        }
        set({ semanticSearching: true });
        try {
          const vector = await generateEmbedding(query);
          // 三通道检索（N3：条目级 + 条目 chunk 级 + 笔记 chunk 级）
          const [hits, chunkHits, noteHits] = await Promise.all([
            tauri.vecSearch(vector, 20),
            tauri.vecSearchChunks(vector, 20).catch(() => []),
            tauri.vecSearchNoteChunks(vector, 20).catch(() => []),
          ]);
          // 条目通道：chunk 命中与条目级命中按 id 合并，取最小 distance
          const byId = new Map<string, SemanticHit>();
          for (const h of chunkHits) {
            const prev = byId.get(h.item.id);
            if (!prev || h.distance < prev.distance) {
              byId.set(h.item.id, { kind: 'item', item: h.item, distance: h.distance, snippet: h.chunkText });
            }
          }
          for (const h of hits) {
            const prev = byId.get(h.item.id);
            if (!prev || h.distance < prev.distance) {
              byId.set(h.item.id, { kind: 'item', item: h.item, distance: h.distance, snippet: prev?.kind === 'item' ? prev.snippet : undefined });
            }
          }
          // 笔记通道：按 noteId 取最佳片段
          const byNote = new Map<string, SemanticHit>();
          for (const h of noteHits) {
            const prev = byNote.get(h.noteId);
            if (!prev || h.distance < prev.distance) {
              byNote.set(h.noteId, {
                kind: 'note',
                noteId: h.noteId,
                title: h.title,
                articleType: h.articleType as Article['articleType'],
                distance: h.distance,
                snippet: h.chunkText,
              });
            }
          }
          const merged = [...byId.values(), ...byNote.values()]
            .sort((a, b) => a.distance - b.distance)
            .slice(0, 20)
            .map((h) => (h.kind === 'item' ? { ...h, item: normalizeItem(h.item) } : h));
          set({ semanticResults: merged });
        } finally {
          set({ semanticSearching: false });
        }
      },
      clearSemanticResults: () => set({ semanticResults: null }),
      scheduleNoteIndex: (noteId) => {
        const prev = noteIndexTimers.get(noteId);
        if (prev) clearTimeout(prev);
        noteIndexTimers.set(
          noteId,
          setTimeout(() => {
            noteIndexTimers.delete(noteId);
            const state = get();
            if (state.settings.semanticSearchEnabled === false) return;
            const art = state.articles.find((a) => a.id === noteId);
            if (!art || !art.content.trim()) return;
            void (async () => {
              try {
                await tauri.vecUpsertNoteChunks(art.id, await buildNoteChunks(art));
              } catch (e) {
                // 未配置嵌入模型等情况静默跳过，不打断写作
                console.error('Failed to index note:', noteId, e);
              }
            })();
          }, 5000)
        );
      },
      indexAllItems: async () => {
        if (get().indexing) return 0;
        const all = get().items;
        // chunk 级索引为准（chunk 0 即元数据，覆盖旧条目级索引的语义面）
        const chunkIndexed = new Set(await tauri.vecChunkIndexedIds());
        const pending = all.filter((i) => !chunkIndexed.has(i.id));
        // N3：笔记/文档同步纳入（manual/journal/AI 解读全量；正文为空的不索引）
        const noteIndexed = new Set(await tauri.vecNoteChunkIndexedIds().catch(() => [] as string[]));
        const pendingNotes = get().articles.filter((a) => !noteIndexed.has(a.id) && a.content.trim());
        const total = pending.length + pendingNotes.length;
        if (total === 0) return 0;

        let failed = 0;
        const bump = () =>
          set((state) => ({
            indexing: state.indexing ? { ...state.indexing, done: state.indexing.done + 1 } : null,
          }));
        set({ indexing: { done: 0, total } });
        try {
          for (const item of pending) {
            try {
              // chunk 0：元数据文本（标题 / 简介 / AI 摘要）
              const metaText = [item.title, item.description, item.aiSummary]
                .filter(Boolean)
                .join('\n');
              const texts: string[] = [metaText];
              // chunk 1..N：本地已就绪正文（只读缓存，不触发网络抓取）
              const cached = await tauri.getContentCached(item.id).catch(() => null);
              if (cached?.contentText) {
                texts.push(...chunkText(cached.contentText, 800).slice(0, 4));
              }
              const inputs: tauri.ChunkInput[] = [];
              for (let idx = 0; idx < texts.length; idx++) {
                inputs.push({ idx, text: texts[idx], vector: await generateEmbedding(texts[idx]) });
              }
              await tauri.vecUpsertChunks(item.id, inputs);
              // 条目级索引保持兼容（复用 chunk 0 向量，无额外嵌入调用）
              await tauri.vecUpsertEmbedding(item.id, inputs[0].vector).catch(() => {});
            } catch (e) {
              failed++;
              // 配置类错误（无 key/无模型）后续必然全部失败，直接中断
              if (failed === 1 && String(e).includes('未配置')) throw e;
              console.error('Failed to index chunks for item:', item.id, e);
            }
            bump();
          }
          for (const art of pendingNotes) {
            try {
              await tauri.vecUpsertNoteChunks(art.id, await buildNoteChunks(art));
            } catch (e) {
              failed++;
              if (failed === 1 && String(e).includes('未配置')) throw e;
              console.error('Failed to index chunks for note:', art.id, e);
            }
            bump();
          }
        } finally {
          set({ indexing: null });
        }
        return failed;
      },

      initialized: false,
      initialize: async () => {
        if (get().initialized) return;
        if (initInFlight) return initInFlight;
        initInFlight = (async () => {
          try {
            await tauri.initDatabase();
            await get().loadSettings();
            await get().loadSources();
            await get().loadItems();
            await get().loadTasks();
            await get().loadPomodoroSessions();
            await get().loadArticles();
            await get().refreshStats();
            set({ initialized: true });
          } catch (e) {
            console.error('Failed to initialize app:', e);
          } finally {
            initInFlight = null;
          }
        })();
        return initInFlight;
      },
    }),
    {
      name: 'sophonote-storage',
      version: 3,
      migrate: (persisted: any, version: number) => {
        // v1 → v2：aiConfig 从单配置改为多供应商结构，丢弃旧缓存避免崩溃
        if (version < 2 && persisted?.settings?.aiConfig && !persisted.settings.aiConfig.providers) {
          return { ...persisted, settings: defaultSettings };
        }
        if (version < 3 && persisted?.activePage === 'library') {
          return { ...persisted, activePage: 'discover' };
        }
        return persisted;
      },
      partialize: (state) => ({
        // 供应商配置不含密钥，可直接持久化；API Key 只在内存 + 钥匙串
        settings: state.settings,
        collections: state.collections,
        activePage: state.activePage,
      }),
    }
  )
);
