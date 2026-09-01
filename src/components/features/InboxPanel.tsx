import { useEffect, useState } from 'react';
import { useAppStore } from '../../stores/appStore';
import { fetchSourcesNow, getItems, getStories } from '../../services/tauri';
import type { Item } from '../../types';
import ItemCard from './ItemCard';
import SearchBox from './SearchBox';
import EmptyState from '../ui/EmptyState';
import { Database, Inbox as InboxIcon, Loader2, NotebookPen, RefreshCw, Sparkles } from 'lucide-react';

const filters = [
  { id: 'all', label: '全部' },
  { id: 'unread', label: '未读' },
  { id: 'starred', label: '收藏' },
  { id: 'archived', label: '归档' },
  { id: 'repo', label: '仓库' },
  { id: 'paper', label: '论文' },
  { id: 'product', label: '产品' },
];

const PAGE_SIZE = 30;

function normalizeItem(item: Item): Item {
  const raw = item as Item & { topics?: string | string[]; aiTags?: string | string[] };
  return {
    ...item,
    topics: typeof raw.topics === 'string' ? raw.topics.split(',').filter(Boolean) : (raw.topics ?? []),
    aiTags: typeof raw.aiTags === 'string' ? raw.aiTags.split(',').filter(Boolean) : (raw.aiTags ?? []),
  };
}

function itemQuery(filter: string, query: string, offset: number) {
  const status = ['unread', 'starred', 'archived'].includes(filter) ? filter : undefined;
  const itemType = ['repo', 'paper', 'product', 'article', 'model'].includes(filter) ? filter : undefined;
  return {
    status,
    itemType,
    query: query.trim() || undefined,
    excludeArchived: filter !== 'archived',
    offset,
    limit: PAGE_SIZE + 1,
  };
}

/**
 * 收件箱：数据源同步条目的统一处理入口，并承接旧 Library 的检索与索引能力。
 */
export default function InboxPanel() {
  const {
    items, searchQuery, starItem, archiveItem, updateItemStatus, deleteItem,
    addItems, setSelectedItemId, loadItems, refreshStats, stats, settings, semanticResults,
    semanticSearching, indexing, semanticSearch, clearSemanticResults,
    indexAllItems, requestOpenArticle,
  } = useAppStore();
  const [activeFilter, setActiveFilter] = useState('all');
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [refreshMsg, setRefreshMsg] = useState('');
  const [pageItems, setPageItems] = useState<Item[]>([]);
  const [pageLoading, setPageLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [reloadVersion, setReloadVersion] = useState(0);
  const [searchMode, setSearchMode] = useState<'keyword' | 'semantic'>('keyword');
  const [semanticInput, setSemanticInput] = useState('');
  const [searchError, setSearchError] = useState('');
  const [indexMsg, setIndexMsg] = useState('');
  const [multiItemIds, setMultiItemIds] = useState<Set<string>>(new Set());
  const semanticEnabled = settings.semanticSearchEnabled ?? true;

  useEffect(() => {
    if (searchMode !== 'keyword') return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      setPageLoading(true);
      getItems(itemQuery(activeFilter, searchQuery, 0))
        .then((raw) => {
          if (cancelled) return;
          const normalized = raw.map(normalizeItem);
          setPageItems(normalized.slice(0, PAGE_SIZE));
          setHasMore(normalized.length > PAGE_SIZE);
        })
        .catch((error) => {
          if (!cancelled) setSearchError(error instanceof Error ? error.message : String(error));
        })
        .finally(() => { if (!cancelled) setPageLoading(false); });
    }, searchQuery.trim() ? 180 : 0);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [activeFilter, reloadVersion, searchMode, searchQuery]);

  useEffect(() => {
    let cancelled = false;
    getStories(200)
      .then((stories) => {
        if (cancelled) return;
        const ids = new Set<string>();
        for (const story of stories) {
          if (story.signalLevel === 'multi') story.itemIds.forEach((id) => ids.add(id));
        }
        setMultiItemIds(ids);
      })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [items.length]);

  useEffect(() => {
    if (!semanticEnabled && searchMode === 'semantic') {
      setSearchMode('keyword');
      clearSemanticResults();
    }
  }, [clearSemanticResults, searchMode, semanticEnabled]);

  // 点击卡片：标记已读并打开阅读视图
  const openItem = (id: string) => {
    const item = pageItems.find((candidate) => candidate.id === id);
    if (item) addItems([item]);
    void updateItemStatus(id, 'read');
    setPageItems((current) => activeFilter === 'unread'
      ? current.filter((candidate) => candidate.id !== id)
      : current.map((candidate) => candidate.id === id ? { ...candidate, status: 'read' } : candidate));
    setSelectedItemId(id);
  };

  const handleStar = async (id: string) => {
    const current = pageItems.find((item) => item.id === id);
    const nextStatus = current?.status === 'starred' ? 'unread' : 'starred';
    setPageItems((list) => activeFilter === 'starred' && nextStatus !== 'starred'
      ? list.filter((item) => item.id !== id)
      : list.map((item) => item.id === id ? { ...item, status: nextStatus } : item));
    await starItem(id);
    await refreshStats();
  };

  const handleArchive = async (id: string) => {
    setPageItems((list) => activeFilter === 'archived'
      ? list.map((item) => item.id === id ? { ...item, status: 'archived' } : item)
      : list.filter((item) => item.id !== id));
    await archiveItem(id);
    await refreshStats();
  };

  const handleDelete = async (id: string) => {
    setPageItems((list) => list.filter((item) => item.id !== id));
    await deleteItem(id);
  };

  const loadMore = async () => {
    setLoadingMore(true);
    try {
      const raw = await getItems(itemQuery(activeFilter, searchQuery, pageItems.length));
      const normalized = raw.map(normalizeItem);
      setPageItems((current) => [...current, ...normalized.slice(0, PAGE_SIZE)]);
      setHasMore(normalized.length > PAGE_SIZE);
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoadingMore(false);
    }
  };

  const handleRefresh = async () => {
    setIsRefreshing(true);
    setRefreshMsg('');
    try {
      // 与定时调度同一抓取入口：Rust 抓取 → 去重写 SQLite → 前端重新加载
      const results = await fetchSourcesNow();
      const ok = results.filter((r) => r.success);
      const failed = results.filter((r) => !r.success);
      const newTotal = ok.reduce((s, r) => s + r.newItems, 0);
      const parts = [`${ok.length} 个源成功，新增 ${newTotal} 条`];
      if (failed.length > 0) {
        parts.push(`${failed.length} 个失败：${failed.map((f) => `${f.sourceId}（${(f.error || '').slice(0, 30)}）`).join('、')}`);
      }
      setRefreshMsg(parts.join('；'));
      await loadItems();
      await refreshStats();
      setReloadVersion((version) => version + 1);
    } catch (err) {
      setRefreshMsg(`刷新失败：${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setIsRefreshing(false);
    }
  };

  const switchFilter = (id: string) => {
    setActiveFilter(id);
  };

  const switchSearchMode = (mode: 'keyword' | 'semantic') => {
    setSearchMode(mode);
    setSearchError('');
    setIndexMsg('');
    if (mode === 'keyword') clearSemanticResults();
  };

  const runSemanticSearch = async () => {
    setSearchError('');
    try {
      await semanticSearch(semanticInput);
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : String(error));
    }
  };

  const runIndexing = async () => {
    setSearchError('');
    setIndexMsg('');
    try {
      const failed = await indexAllItems();
      setIndexMsg(failed > 0 ? `索引完成（${failed} 条失败）` : '索引完成，可以语义搜索了');
    } catch (error) {
      setSearchError(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <div className="rounded-[var(--radius-lg)] border border-[var(--border-default)] bg-[var(--bg-surface)] overflow-hidden">
      {/* 面板头 */}
      <header className="px-4 py-3 border-b border-[var(--border-default)] flex items-center justify-between gap-2">
        <div className="flex items-center gap-2.5 min-w-0">
          <h3 className="text-sm font-semibold text-[var(--text-primary)] shrink-0">收件箱</h3>
          <span className="text-[12px] px-1.5 py-0.5 rounded-full bg-[var(--accent-subtle)] text-[var(--accent)] font-medium shrink-0">
            {stats.unreadItems} 未读
          </span>
          {stats.starredItems > 0 && (
            <span className="text-[12px] px-1.5 py-0.5 rounded-full bg-[var(--warning-subtle)] text-[var(--warning)] font-medium shrink-0">
              {stats.starredItems} 收藏
            </span>
          )}
          <span className="text-[12px] text-[var(--text-tertiary)] truncate hidden sm:inline">
            原始条目、收藏与检索 · 首次拉取起保留 7 天
          </span>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <div className="flex items-center rounded-md border border-[var(--border-default)] overflow-hidden">
            {(['keyword', 'semantic'] as const).map((mode) => {
              const disabled = mode === 'semantic' && !semanticEnabled;
              return (
                <button
                  key={mode}
                  onClick={() => !disabled && switchSearchMode(mode)}
                  disabled={disabled}
                  title={disabled ? '语义搜索已关闭，请到设置的 AI 配置中开启' : undefined}
                  className={`px-2 py-1 text-xs font-medium transition-colors ${
                    disabled
                      ? 'text-[var(--text-disabled)] cursor-not-allowed'
                      : searchMode === mode
                        ? 'bg-[var(--accent)] text-white'
                        : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'
                  }`}
                >
                  {mode === 'keyword' ? '关键词' : '语义'}
                </button>
              );
            })}
          </div>
          {searchMode === 'keyword' ? (
            <SearchBox />
          ) : (
            <div className="flex items-center gap-1.5">
              <div className="relative w-56">
                <Sparkles size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--accent)]" />
                <input
                  value={semanticInput}
                  onChange={(event) => setSemanticInput(event.target.value)}
                  onKeyDown={(event) => { if (event.key === 'Enter') void runSemanticSearch(); }}
                  placeholder="描述你想找的内容…"
                  className="input pl-8 py-1.5 text-xs"
                />
              </div>
              <button
                onClick={() => void runSemanticSearch()}
                disabled={semanticSearching || !semanticInput.trim()}
                className="px-2.5 py-1.5 rounded-md text-xs font-medium bg-[var(--accent)] text-white disabled:opacity-50 flex items-center gap-1"
              >
                {semanticSearching && <Loader2 size={12} className="animate-spin" />}
                搜索
              </button>
              <button
                onClick={() => void runIndexing()}
                disabled={indexing !== null}
                title="为收件箱条目与笔记建立语义索引"
                className="px-2.5 py-1.5 rounded-md text-xs font-medium border border-[var(--border-default)] text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-50 flex items-center gap-1"
              >
                {indexing ? <Loader2 size={12} className="animate-spin" /> : <Database size={12} />}
                {indexing ? `${indexing.done}/${indexing.total}` : '建立索引'}
              </button>
            </div>
          )}
          <button
            onClick={handleRefresh}
            className="p-1.5 rounded-lg text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:bg-[var(--accent-subtle)] transition-colors"
            title="刷新全部数据源"
          >
            <RefreshCw size={14} className={isRefreshing ? 'animate-spin' : ''} />
          </button>
        </div>
      </header>

      {refreshMsg && (
        <div className="px-4 py-1.5 text-[12px] border-b border-[var(--border-default)] bg-[var(--bg-sunken)] text-[var(--text-secondary)] flex items-center justify-between">
          <span className="truncate">{refreshMsg}</span>
          <button onClick={() => setRefreshMsg('')} className="ml-3 shrink-0 text-[var(--text-tertiary)] hover:text-[var(--text-secondary)]">✕</button>
        </div>
      )}

      {(searchError || indexMsg) && (
        <div className={`px-4 py-1.5 text-xs border-b border-[var(--border-default)] ${
          searchError ? 'bg-[var(--danger-subtle)] text-[var(--danger)]' : 'bg-[var(--success-subtle)] text-[var(--success)]'
        }`}>
          {searchError || indexMsg}
        </div>
      )}

      {/* 过滤器 */}
      <div className="px-4 py-2 border-b border-[var(--border-default)] flex items-center gap-1 flex-wrap">
        {filters.map((f) => (
          <button
            key={f.id}
            onClick={() => switchFilter(f.id)}
            className={`px-3 py-1 rounded-md text-[13px] font-medium transition-colors ${
              activeFilter === f.id
                ? 'bg-[var(--accent)] text-white'
                : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'
            }`}
          >
            {f.label}
          </button>
        ))}
        <span className="ml-auto text-[12px] text-[var(--text-tertiary)]">
          {searchMode === 'semantic' && semanticResults
            ? `${semanticResults.length} 条`
            : `已加载 ${pageItems.length}${hasMore ? '+' : ''} 条`}
        </span>
      </div>

      {/* 内容区 */}
      <div className="p-4">
        {searchMode === 'semantic' && semanticResults !== null ? (
          semanticResults.length === 0 ? (
            <EmptyState
              icon={Sparkles}
              title="没有找到语义相近的内容"
              desc="先建立索引，再换个描述试试"
              className="py-12"
            />
          ) : (
            <div className="space-y-3">
              {semanticResults.map((hit) => hit.kind === 'note' ? (
                <button
                  key={hit.noteId}
                  onClick={() => requestOpenArticle(hit.noteId)}
                  className="w-full text-left p-3 rounded-[var(--radius-lg)] border border-[var(--accent-border)] bg-[var(--accent-subtle)] hover:border-[var(--accent)] transition-colors"
                >
                  <div className="flex items-center gap-2 mb-1 text-[12px] text-[var(--accent)] font-medium">
                    <NotebookPen size={12} />
                    {hit.articleType === 'manual' ? '笔记' : hit.articleType === 'journal' ? '日记' : 'AI 解读'}
                    <span>相似度 {(1 / (1 + hit.distance)).toFixed(2)}</span>
                  </div>
                  <p className="text-sm font-semibold text-[var(--text-primary)]">{hit.title}</p>
                  <p className="text-xs text-[var(--text-tertiary)] mt-1 line-clamp-2">命中片段：{hit.snippet.slice(0, 160)}</p>
                </button>
              ) : (
                <div key={hit.item.id} className="space-y-1">
                  <div className="text-[12px] text-[var(--accent)] font-medium px-1">
                    相似度 {(1 / (1 + hit.distance)).toFixed(2)}
                    {hit.snippet ? ` · 命中片段：${hit.snippet.slice(0, 100)}` : ''}
                  </div>
                  <ItemCard
                    item={hit.item}
                    onStar={handleStar}
                    onArchive={handleArchive}
                    onOpen={openItem}
                    onDelete={handleDelete}
                    multiSource={multiItemIds.has(hit.item.id)}
                  />
                </div>
              ))}
            </div>
          )
        ) : pageLoading ? (
          <div className="py-12 flex items-center justify-center text-[var(--text-tertiary)]">
            <Loader2 size={18} className="animate-spin" />
          </div>
        ) : pageItems.length === 0 ? (
          <EmptyState
            icon={InboxIcon}
            title="暂无内容"
            desc="点击上方刷新按钮或「一键同步」获取最新数据"
            className="py-12"
          />
        ) : (
          <>
            <div className="space-y-3">
              {pageItems.map((item) => (
                <ItemCard
                  key={item.id}
                  item={item}
                  onStar={handleStar}
                  onArchive={handleArchive}
                  onOpen={openItem}
                  onDelete={handleDelete}
                  multiSource={multiItemIds.has(item.id)}
                />
              ))}
            </div>
            {hasMore && (
              <button
                onClick={() => void loadMore()}
                disabled={loadingMore}
                className="mt-3 w-full py-2 rounded-lg text-xs text-[var(--text-secondary)] border border-[var(--border-default)] hover:bg-[var(--bg-sunken)] transition-colors"
              >
                {loadingMore ? '加载中…' : '加载更多'}
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
