import { useCallback, useEffect, useMemo, useState } from 'react';
import { useAppStore } from '../stores/appStore';
import * as tauri from '../services/tauri';
import DiscoverRow from '../components/features/DiscoverRow';
import DiscoverReport from '../components/features/DiscoverReport';
import DiscoverTopics from '../components/features/DiscoverTopics';
import DiscoverLeaderboard from '../components/features/DiscoverLeaderboard';
import type { DailyPick, DiscoverCategory, Item } from '../types';
import { Sparkles, List, Newspaper, LayoutGrid, Trophy, Loader2, ChevronLeft } from 'lucide-react';

/**
 * 发现页 · 五断面 IA：精选 / 全部 AI 动态 / AI 日报 / 主题 / 模型榜。
 *
 * 数据面 = items 全量打分（items.ai_score/aspect/ai_topics），由 Hermes Skill
 * sophonote-ai-radar 经 Bridge save_discovery_scores 写入；本页纯只读消费
 * （db_discovery_feed）。daily_picks 退役为内部审计数据，不再驱动 UI。
 * 精选 = aspect∈五面 ∧ 近 7 天 ∧ ≥8.5 ∧ deep；全部 = ≥7 ∧ deep，时间线 + 游标分页。
 * 视觉令牌收敛在 .hb-discover 作用域（src/index.css），适配全局浅色/深色。
 */

type Section = 'featured' | 'all' | 'reports' | 'topics' | 'leaderboard';

const FEATURED_MIN = 8.5;
const ALL_MIN = 7;
const WINDOW_DAYS = 7;
const PAGE_SIZE = 40;

export function recentDiscoveryFallbackQuery(
  aspect: tauri.DiscoveryAspect | null,
): tauri.DiscoveryFeedQuery {
  return {
    minScore: ALL_MIN,
    requireDeep: true,
    aspect,
    limit: 6,
  };
}

const sections: { id: Section; name: string; icon: React.FC<{ size?: number; className?: string }> }[] = [
  { id: 'featured', name: '精选', icon: Sparkles },
  { id: 'all', name: '全部 AI 动态', icon: List },
  { id: 'reports', name: 'AI 日报', icon: Newspaper },
  { id: 'topics', name: '主题', icon: LayoutGrid },
  { id: 'leaderboard', name: '模型榜', icon: Trophy },
];

function dateLabel(d: string): string {
  const today = new Date().toLocaleDateString('sv-SE');
  const yesterday = new Date(Date.now() - 86400000).toLocaleDateString('sv-SE');
  if (d === today) return '今天';
  if (d === yesterday) return '昨天';
  return d;
}

function categoryOf(sourceId: string): DiscoverCategory {
  if (sourceId.includes('github')) return 'github';
  if (sourceId.includes('arxiv')) return 'arxiv';
  if (sourceId.includes('hacker')) return 'hackernews';
  if (sourceId.includes('producthunt')) return 'producthunt';
  if (sourceId.includes('aihot')) return 'aihot';
  return 'huggingface';
}

function laneOf(sourceId: string): 'github' | 'model' | 'product' {
  if (sourceId.includes('github')) return 'github';
  if (sourceId.includes('producthunt')) return 'product';
  return 'model';
}

/** feed 行 → DiscoverRow 的 DailyPick 形态（行组件复用，不引入第二套行渲染） */
function rowToPick(row: tauri.DiscoveryFeedRow, rank: number): DailyPick {
  const item: Item = {
    id: row.id,
    sourceId: row.sourceId,
    type: (row.type as Item['type']) || 'article',
    title: row.title,
    url: row.url ?? '',
    description: row.description ?? '',
    author: row.author ?? undefined,
    language: row.language ?? undefined,
    stars: row.stars ?? undefined,
    forks: row.forks ?? undefined,
    publishedAt: row.publishedAt ?? row.aiScoredAt,
    fetchedAt: row.fetchedAt ?? row.aiScoredAt,
    status: (row.status as Item['status']) || 'unread',
    aiSummary: row.aiSummary ?? undefined,
    aiTags: row.aiTags ? row.aiTags.split(',').map((t) => t.trim()).filter(Boolean) : undefined,
    contentStatus: (row.contentStatus as Item['contentStatus']) ?? undefined,
    qualityLevel: row.qualityLevel ?? undefined,
  };
  return {
    id: `${row.id}:${row.aiScoredAt}`,
    date: row.aiScoredAt.slice(0, 10),
    category: categoryOf(row.sourceId),
    rank,
    heatScore: null,
    aiScore: row.aiScore,
    reason: row.aiReason ?? null,
    selectionLane: laneOf(row.sourceId),
    createdAt: row.aiScoredAt,
    item,
  };
}

export default function Discover() {
  const {
    starItem, archiveItem, deleteItem, setSelectedItemId,
    sedimentToNote, articles, addItems,
  } = useAppStore();
  const [section, setSection] = useState<Section>('featured');
  const [aspect, setAspect] = useState<tauri.DiscoveryAspect | null>(null);
  const [selectedTopic, setSelectedTopic] = useState<string | null>(null);
  const [rows, setRows] = useState<tauri.DiscoveryFeedRow[]>([]);
  const [recentFallbackRows, setRecentFallbackRows] = useState<tauri.DiscoveryFeedRow[]>([]);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [sedimentingId, setSedimentingId] = useState<string | null>(null);

  // N1：已沉淀条目的笔记图标高亮（manual 笔记带 itemId）
  const sedimentedItemIds = useMemo(
    () => new Set(articles.filter((a) => a.articleType === 'manual' && a.itemId).map((a) => a.itemId as string)),
    [articles]
  );

  const feedQuery = useCallback(
    (cursor?: string | null): tauri.DiscoveryFeedQuery =>
      section === 'featured'
        ? { minScore: FEATURED_MIN, windowDays: WINDOW_DAYS, requireDeep: true, aspect, limit: 100 }
        : {
            minScore: ALL_MIN,
            requireDeep: true,
            aspect: section === 'all' ? aspect : null,
            topic: section === 'topics' ? selectedTopic : null,
            cursor: cursor ?? null,
            limit: PAGE_SIZE,
          },
    [section, aspect, selectedTopic]
  );

  const isFeed = section === 'featured' || section === 'all' || (section === 'topics' && selectedTopic !== null);

  const reloadFeed = useCallback(async () => {
    setLoading(true);
    try {
      const page = await tauri.getDiscoveryFeed(feedQuery(null));
      setRows(page.rows);
      if (section === 'featured' && page.rows.length === 0) {
        const recent = await tauri.getDiscoveryFeed(recentDiscoveryFallbackQuery(aspect));
        setRecentFallbackRows(recent.rows);
      } else {
        setRecentFallbackRows([]);
      }
      setNextCursor(section === 'all' ? (page.nextCursor ?? null) : null);
    } catch (e) {
      console.error('Failed to load discovery feed:', e);
      setRows([]);
      setRecentFallbackRows([]);
      setNextCursor(null);
    } finally {
      setLoading(false);
    }
  }, [feedQuery, section]);

  useEffect(() => {
    if (isFeed) {
      reloadFeed();
    } else {
      setLoading(false);
      setRows([]);
      setRecentFallbackRows([]);
      setNextCursor(null);
    }
  }, [isFeed, reloadFeed]);

  const loadMore = async () => {
    if (!nextCursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await tauri.getDiscoveryFeed(feedQuery(nextCursor));
      setRows((prev) => [...prev, ...page.rows]);
      setNextCursor(page.nextCursor ?? null);
    } catch (e) {
      console.error('Failed to load more discovery rows:', e);
    } finally {
      setLoadingMore(false);
    }
  };

  const showingRecentFallback = section === 'featured' && rows.length === 0 && recentFallbackRows.length > 0;
  const displayedRows = showingRecentFallback ? recentFallbackRows : rows;

  // 按打分日分组（feed 已按 ai_scored_at DESC 排序，Map 保序即倒序）
  const groups = useMemo(() => {
    const m = new Map<string, tauri.DiscoveryFeedRow[]>();
    for (const row of displayedRows) {
      const date = row.aiScoredAt.slice(0, 10);
      const arr = m.get(date) || [];
      arr.push(row);
      m.set(date, arr);
    }
    return Array.from(m.entries());
  }, [displayedRows]);

  const handleSediment = async (id: string) => {
    setSedimentingId(id);
    try {
      await sedimentToNote(id);
    } catch (e) {
      // NB-31：沉淀创建失败不跳转（store 无幽灵记录），记录错误
      console.error('Sediment to note failed:', e);
    } finally {
      setSedimentingId(null);
    }
  };

  const activeSection = sections.find((s) => s.id === section)!;

  const chip = (active: boolean) =>
    `hb-d-chip ${active ? 'is-active' : ''}`;

  const timelineMode =
    section === 'featured' || section === 'all' || (section === 'topics' && selectedTopic !== null);
  const shellMode =
    section === 'topics' && selectedTopic !== null ? 'topic-feed' : section;
  const shellModeClass = `hb-d-mode-${shellMode}`;

  return (
    <div className={`hb-discover hb-d-shell ${shellModeClass} flex h-full`}>
      {/* 左列：五断面导航（aihot 侧栏式 mono eyebrow 分组） */}
      <aside className="hb-d-sidebar shrink-0 flex flex-col">
        {/* NB-15 首行统一 h-10（与侧栏红绿灯行对齐）；NB-20 首行空白可拖窗 */}
        <header className="hb-d-sidebar-head px-4 h-10 flex items-center justify-between" data-tauri-drag-region>
          <h2 className="text-sm font-bold text-[var(--d-ink)]" data-tauri-drag-region>发现</h2>
        </header>
        <nav className="hb-d-sidebar-nav flex-1 overflow-y-auto">
          <p className="hb-d-eyebrow px-3 pt-1.5 pb-1.5">内容</p>
          <div className="space-y-0.5">
            {sections.map((s) => {
              const Icon = s.icon;
              const isActive = section === s.id;
              return (
                <button
                  key={s.id}
                  onClick={() => { setSection(s.id); setAspect(null); setSelectedTopic(null); }}
                  className={`hb-d-side-btn ${isActive ? 'is-active' : ''}`}
                >
                  <Icon size={15} className={isActive ? 'text-[var(--d-brand)]' : 'text-[var(--d-ink-3)]'} />
                  <span className="flex-1 truncate">{s.name}</span>
                </button>
              );
            })}
          </div>
        </nav>
        <footer className="hb-d-sidebar-foot" />
      </aside>

      {/* 右列：当前断面 feed */}
      <main className={`hb-d-main ${shellModeClass} flex-1 overflow-y-auto`}>
        <header className="hb-d-main-head px-5 pt-0 h-10 flex items-center justify-between gap-3" data-tauri-drag-region>
          <div className="flex items-center gap-3 min-w-0" data-tauri-drag-region>
            <h3 className="hb-d-main-title text-[13px] font-bold text-[var(--d-ink)] shrink-0" data-tauri-drag-region>
              {selectedTopic ?? activeSection.name}
            </h3>
          </div>
          <div className="flex items-center gap-2.5">
            <span className="text-[12px] font-mono text-[var(--d-ink-faint)]">{new Date().toLocaleDateString('zh-CN')}</span>
          </div>
        </header>

        {section === 'reports' ? (
          <DiscoverReport />
        ) : section === 'leaderboard' ? (
          <DiscoverLeaderboard />
        ) : section === 'topics' && !selectedTopic ? (
          <DiscoverTopics onSelect={setSelectedTopic} />
        ) : (
          <>
            {selectedTopic ? (
              <div className="hb-d-section-shell hb-d-back-bar">
                <button
                  onClick={() => setSelectedTopic(null)}
                  className="hb-d-back-link inline-flex items-center gap-1"
                >
                  <ChevronLeft size={14} /> 返回主题地图
                </button>
              </div>
            ) : (
              <div className="hb-d-section-shell hb-d-chip-bar">
                <div className="hb-d-chip-wrap">
                  <button className={chip(aspect === null)} onClick={() => setAspect(null)}>全部</button>
                  {tauri.DISCOVERY_ASPECTS.map((a) => (
                    <button key={a} className={chip(aspect === a)} onClick={() => setAspect(a)}>{a}</button>
                  ))}
                </div>
              </div>
            )}

        {loading ? (
          <div className="hb-d-section-shell space-y-6">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="flex gap-3">
                <div className="hb-d-skel w-6 h-6 rounded-[7px] shrink-0" />
                <div className="flex-1 space-y-2.5">
                  <div className="hb-d-skel h-5 w-3/4" />
                  <div className="hb-d-skel h-3.5 w-full" />
                  <div className="hb-d-skel h-3.5 w-1/2" />
                </div>
              </div>
            ))}
          </div>
        ) : groups.length === 0 ? (
          <div className="hb-d-section-shell hb-d-empty-wrap py-24">
            <div className="hb-d-empty-ring"><div className="hb-d-empty-core" /></div>
            <div>
              <p className="text-[15px] font-bold text-[var(--d-ink)]">
                {section === 'featured' ? '还没有精选内容' : '还没有动态'}
              </p>
              <p className="mt-2 max-w-md text-[12px] leading-6 text-[var(--d-ink-3)]">
                {section === 'featured'
                  ? '计划任务完成深度解读后，近 7 天达到精选门槛的内容会出现在这里。'
                  : '计划任务或会话完成发现与深度解读后，结果会按时间沉淀在这里。'}
              </p>
            </div>
          </div>
        ) : (
          <>
            {showingRecentFallback && (
              <div className="hb-d-section-shell pb-2 pt-4">
                <div className="flex items-center justify-between gap-4 rounded-xl border border-[var(--d-divider)] bg-[var(--d-surface)] px-4 py-3">
                  <div>
                    <p className="text-[13px] font-bold text-[var(--d-ink)]">本周暂无新精选，先看看近期积累</p>
                    <p className="mt-1 text-[11px] leading-5 text-[var(--d-ink-3)]">以下内容已完成深度解读且达到发现门槛，但不计入本周精选。</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => { setSection('all'); setAspect(null); }}
                    className="hb-d-chip shrink-0"
                  >
                    查看全部
                  </button>
                </div>
              </div>
            )}
            {groups.map(([date, dayRows], gIdx) => (
              <section
                key={date}
                className={`hb-d-day-section ${timelineMode ? 'hb-d-timeline-focus' : ''}`}
              >
                <div className="hb-d-daybar">
                  <span className="hb-d-daybar-main">{dateLabel(date)}</span>
                  <span className="hb-d-daybar-sub">
                    {date} · {dayRows.length} 条
                  </span>
                </div>
                <div className="hb-d-section-shell">
                  {dayRows.map((row, index) => (
                    <DiscoverRow
                      key={`${row.id}:${row.aiScoredAt}`}
                      pick={rowToPick(row, index + 1)}
                      rank={section === 'featured' && !showingRecentFallback ? index + 1 : null}
                      hero={section === 'featured' && !showingRecentFallback && gIdx === 0 && index === 0}
                      topics={row.aiTopics}
                      onStar={async (id) => { await starItem(id); await reloadFeed(); }}
                      onArchive={async (id) => { await archiveItem(id); await reloadFeed(); }}
                      onDelete={async (id) => { await deleteItem(id); await reloadFeed(); }}
                      onOpen={(id) => {
                        // ISSUE-044：发现时间线独立分页，条目可能不在首页最近 300 条缓存中。
                        // 点击时注入当前完整快照，避免 ItemDetail 因 items.find 失败直接返回 null。
                        addItems([rowToPick(row, index + 1).item]);
                        setSelectedItemId(id);
                      }}
                      onSediment={(id) => void handleSediment(id)}
                      sedimenting={sedimentingId === row.id}
                      sedimented={sedimentedItemIds.has(row.id)}
                    />
                  ))}
                </div>
              </section>
            ))}
            {(section === 'all' || selectedTopic) && nextCursor && (
              <div className="hb-d-section-shell py-6 flex justify-center">
                <button
                  onClick={loadMore}
                  disabled={loadingMore}
                  className="hb-d-load-more"
                >
                  {loadingMore && <Loader2 size={13} className="animate-spin" />}
                  加载更多
                </button>
              </div>
            )}
            <div className="h-8" />
          </>
        )}
          </>
        )}
      </main>
    </div>
  );
}
