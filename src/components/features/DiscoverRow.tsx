import { Star, ExternalLink, Archive, GitFork, Trash2, Loader2, NotebookPen } from 'lucide-react';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import type { DailyPick, Item } from '../../types';
import ScoreBadge from '../ui/ScoreBadge';

/**
 * 发现页专属行组件（aihot 风格）：发丝线分隔、排名方块、评分胶囊徽章、
 * accent 引用块「入选理由」。与 InboxPanel 共用的 ItemCard 解耦。
 */
interface DiscoverRowProps {
  pick: DailyPick;
  /**
   * 排名方块语义（NEXT-048）：
   * - undefined → 使用 pick.rank（精选断面：aspect 内序号）；
   * - null → 不显示排名，行首改 mono 时间（全部 AI 动态，aihot /all 同构）。
   */
  rank?: number | null;
  /** 受控主题标注（items.ai_topics，NEXT-048）；与 aiTags 同区渲染 */
  topics?: string[];
  /** 当日首条精选 hero 化（标题 18px、摘要 3 行、徽章放大一号） */
  hero?: boolean;
  onOpen: (id: string) => void;
  onStar: (id: string) => void;
  onArchive: (id: string) => void;
  onDelete: (id: string) => void;
  onSediment?: (id: string) => void;
  sedimenting?: boolean;
  sedimented?: boolean;
}

const typeMeta: Record<string, { text: string; dot: string }> = {
  repo: { text: '仓库', dot: 'var(--dot-repo)' },
  paper: { text: '论文', dot: 'var(--dot-paper)' },
  product: { text: '产品', dot: 'var(--dot-product)' },
  article: { text: '文章', dot: 'var(--dot-article)' },
  model: { text: '模型', dot: 'var(--dot-model)' },
};

// 内容轻量标识（P0-5）：只展示有信息量的三档，其余不制造噪音
function contentBadge(item: Item): { text: string; color: string } | null {
  if (item.aiSummary) return { text: '已解读', color: 'var(--success)' };
  if (item.contentStatus === 'ready') return { text: '有正文', color: 'var(--text-secondary)' };
  if (item.contentStatus === 'partial') return { text: '部分正文', color: 'var(--warning)' };
  return null;
}

export default function DiscoverRow({
  pick, rank, topics, hero = false, onOpen, onStar, onArchive, onDelete,
  onSediment, sedimenting, sedimented,
}: DiscoverRowProps) {
  const item = pick.item;
  const t = typeMeta[item.type] || typeMeta.article;
  const badge = contentBadge(item);
  const isStarred = item.status === 'starred';
  const isArchived = item.status === 'archived';
  const shownRank = rank === undefined ? pick.rank : rank;
  const rankCls = shownRank != null && shownRank <= 3 ? ` hb-d-rank-${shownRank}` : '';
  const time = new Date(item.publishedAt);

  return (
    <div className={`hb-d-rowwrap group relative${hero ? ' is-hero' : ''}`}>
      <div className="hb-d-row" onClick={() => onOpen(item.id)}>
        {/* 行首方块：精选=aspect 内序号（1-3 名暖色实心）；全部=mono 打分时间 */}
        {shownRank != null ? (
          <span className={`hb-d-rank${rankCls}`} aria-label={`第 ${shownRank} 名`}>{shownRank}</span>
        ) : (
          <span className="hb-d-rank hb-d-rank-time font-mono" title={`打分于 ${pick.createdAt}`}>
            {time.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
          </span>
        )}

        <div className="flex-1 min-w-0">
          {/* 标题行：类型/内容 chip + 标题 + 外链 */}
          <div className="flex items-center gap-2">
            <span className="hb-d-typechip">
              <span className="hb-d-dot" style={{ background: t.dot }} />
              {t.text}
            </span>
            {badge && (
              <span className="hb-d-typechip" style={{ color: badge.color, borderColor: 'color-mix(in srgb, currentColor 35%, var(--d-chip-border))' }}>
                {badge.text}
              </span>
            )}
            <h3 className="hb-d-title flex-1 min-w-0">{item.title}</h3>
            {item.url && (
              <a
                href={item.url}
                target="_blank"
                rel="noopener noreferrer"
                className="shrink-0 text-[var(--d-ink-3)] hover:text-[var(--d-brand)] transition-colors"
                onClick={(e) => e.stopPropagation()}
                title="查看原文"
              >
                <ExternalLink size={13} />
              </a>
            )}
          </div>

          {/* 速览：AI 摘要优先 */}
          {(item.aiSummary || item.description) && (
            <p className="hb-d-summary">{item.aiSummary || item.description}</p>
          )}

          {/* 元数据 + 热度/AI 评分 */}
          <div className="hb-d-meta">
            {item.author && <span>@{item.author}</span>}
            {item.language && <span>{item.language}</span>}
            {item.stars != null && (
              <span className="flex items-center gap-1" style={{ color: 'var(--d-amber)' }}>
                <Star size={11} fill="currentColor" />
                {item.stars.toLocaleString()}
              </span>
            )}
            {item.forks != null && (
              <span className="flex items-center gap-1">
                <GitFork size={11} />
                {item.forks.toLocaleString()}
              </span>
            )}
            {shownRank != null && (
              <span className="font-mono">
                {time.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
              </span>
            )}
            <span className="ml-auto flex items-center gap-3">
              {pick.heatScore != null && (
                <span className="hb-d-score" title="入选时热度快照（star/赞/分）">热度 {pick.heatScore.toLocaleString()}</span>
              )}
              {pick.aiScore != null && (
                <ScoreBadge score={pick.aiScore} large={hero} />
              )}
            </span>
          </div>

          {/* 入选理由：鼠尾草绿 note 块 */}
          {pick.reason && (
            <div className="hb-d-reason" title={pick.reason}>
              <span className="hb-d-reason-label">入选理由</span>
              {pick.reason}
            </div>
          )}

          {/* 受控主题 + AI 标签 */}
          {((topics && topics.length > 0) || (item.aiTags && item.aiTags.length > 0)) && (
            <div className="flex flex-wrap gap-1.5 mt-2">
              {topics?.map((tag: string) => (
                <span key={`t-${tag}`} className="hb-d-tag hb-d-tag-topic">{tag}</span>
              ))}
              {item.aiTags?.map((tag: string) => (
                <span key={tag} className="hb-d-tag">{tag}</span>
              ))}
            </div>
          )}
        </div>

        {/* 操作区：悬停浮现 */}
        <div
          className="hb-d-action-row shrink-0 flex flex-col gap-0.5"
          onClick={(e) => e.stopPropagation()}
        >
          {onSediment && (
            <button
              onClick={() => onSediment(item.id)}
              disabled={sedimenting}
              className={`hb-d-iconbtn disabled:opacity-50 ${sedimented ? 'hb-d-iconbtn-active' : ''}`}
              title={sedimented ? '打开已沉淀的笔记' : '存为笔记（带来源与证据）'}
            >
              {sedimenting ? <Loader2 size={14} className="animate-spin" /> : <NotebookPen size={14} />}
            </button>
          )}
          <button
            onClick={() => onStar(item.id)}
            className={`hb-d-iconbtn ${isStarred ? 'hb-d-iconbtn-active' : ''}`}
            title={isStarred ? '取消收藏' : '收藏'}
          >
            <Star size={14} fill={isStarred ? 'currentColor' : 'none'} />
          </button>
          <button
            onClick={() => onArchive(item.id)}
            className={`hb-d-iconbtn ${isArchived ? 'hb-d-iconbtn-active' : ''}`}
            title="归档"
          >
            <Archive size={14} />
          </button>
          <button
            onClick={async () => {
              if (await confirmDialog('删除这条内容？将同时删除其向量索引，且不可恢复。', { title: 'SophoNote', kind: 'warning' })) {
                onDelete(item.id);
              }
            }}
            className="hb-d-iconbtn hb-d-iconbtn-danger"
            title="删除（连带向量索引）"
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>
    </div>
  );
}
