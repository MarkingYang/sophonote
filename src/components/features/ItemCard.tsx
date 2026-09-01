import { Star, ExternalLink, Archive, GitFork, Trash2, ChevronRight, Loader2, NotebookPen, Radio } from 'lucide-react';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import type { Item } from '../../types';

interface ItemCardProps {
  item: Item;
  onStar: (id: string) => void;
  onArchive: (id: string) => void;
  onOpen: (id: string) => void;
  onDelete: (id: string) => void;
  compact?: boolean;
  /** Discover 场景：排名序号 */
  rank?: number;
  /** N1 一键沉淀为笔记（提供后显示入口） */
  onSediment?: (id: string) => void;
  sedimenting?: boolean;
  /** 该条目已有沉淀笔记（图标高亮，点击=打开既有笔记） */
  sedimented?: boolean;
  /** 多个信源报道同一故事时的交叉验证信号 */
  multiSource?: boolean;
}

// 类型识别色走全站 dot 令牌（--dot-*，index.css）；徽标形态 = 中性 chip + 彩色 dot
const typeLabels: Record<string, { text: string; dot: string }> = {
  repo: { text: '仓库', dot: 'var(--dot-repo)' },
  paper: { text: '论文', dot: 'var(--dot-paper)' },
  product: { text: '产品', dot: 'var(--dot-product)' },
  article: { text: '文章', dot: 'var(--dot-article)' },
  model: { text: '模型', dot: 'var(--dot-model)' },
};

export default function ItemCard({ item, onStar, onArchive, onOpen, onDelete, compact, rank, onSediment, sedimenting, sedimented, multiSource }: ItemCardProps) {
  const typeInfo = typeLabels[item.type] || typeLabels.article;
  const isStarred = item.status === 'starred';
  const isArchived = item.status === 'archived';

  // 内容轻量标识（P0-5）：区分「系统发现了」和「系统读过了」
  const contentBadge = item.aiSummary
    ? { text: '已解读', cls: 'bg-[var(--accent-subtle)] text-[var(--accent)]' }
    : item.contentStatus === 'ready'
      ? { text: '有正文', cls: 'bg-[var(--success-subtle)] text-[var(--success)]' }
      : item.contentStatus === 'partial'
        ? { text: '部分正文', cls: 'bg-[var(--warning-subtle)] text-[var(--warning)]' }
        : item.description
          ? { text: '有摘要', cls: 'bg-[var(--bg-sunken)] text-[var(--text-secondary)]' }
          : { text: '仅标题', cls: 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]' };

  if (compact) {
    return (
      <div
        className={`flex items-center gap-3 px-4 py-2.5 border-b border-[var(--border-default)] hover:bg-[var(--bg-sunken)] transition-colors cursor-pointer ${
          item.status === 'unread' ? 'bg-[var(--bg-surface)]' : 'bg-[color:color-mix(in_srgb,var(--bg-sunken)_50%,transparent)]'
        }`}
        onClick={() => onOpen(item.id)}
      >
        <span className="hb-chip shrink-0">
          <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: typeInfo.dot }} />
          {typeInfo.text}
        </span>
        <h3 className={`flex-1 text-sm truncate ${item.status === 'unread' ? 'font-medium text-[var(--text-primary)]' : 'text-[var(--text-secondary)]'}`}>
          {item.title}
        </h3>
        <span className="text-xs text-[var(--text-tertiary)] shrink-0">
          {new Date(item.publishedAt).toLocaleDateString('zh-CN')}
        </span>
      </div>
    );
  }

  return (
    <div
      className="card p-4 group cursor-pointer"
      onClick={() => onOpen(item.id)}
    >
      <div className="flex items-start gap-3">
        {/* 排名（Discover 场景） */}
        {rank !== undefined && (
          <span className="shrink-0 w-6 text-center text-sm font-bold text-[var(--text-tertiary)] pt-0.5">
            {rank}
          </span>
        )}

        {/* 类型标签 */}
        <span className="hb-chip shrink-0 mt-0.5">
          <span className="w-1.5 h-1.5 rounded-full shrink-0" style={{ background: typeInfo.dot }} />
          {typeInfo.text}
        </span>

        {/* 内容轻量标识 */}
        <span className={`shrink-0 text-xs px-1.5 py-0.5 rounded-full mt-0.5 ${contentBadge.cls}`}>
          {contentBadge.text}
        </span>

        {multiSource && (
          <span className="shrink-0 text-xs px-1.5 py-0.5 rounded-full mt-0.5 bg-[var(--success-subtle)] text-[var(--success)] flex items-center gap-1" title="多个信源报道了同一故事">
            <Radio size={10} /> 多源验证
          </span>
        )}

        <div className="flex-1 min-w-0">
          {/* 标题 */}
          <div className="flex items-center gap-2">
            <h3
              className={`text-sm font-semibold truncate group-hover:text-[var(--accent)] transition-colors ${
                item.status === 'unread' ? 'text-[var(--text-primary)]' : 'text-[var(--text-secondary)]'
              }`}
            >
              {item.title}
            </h3>
            {item.url && (
              <a
                href={item.url}
                target="_blank"
                rel="noopener noreferrer"
                className="shrink-0 text-[var(--text-tertiary)] hover:text-[var(--accent)] transition-colors"
                onClick={(e) => e.stopPropagation()}
                title="查看原文"
              >
                <ExternalLink size={13} />
              </a>
            )}
          </div>

          {/* 速览内容：AI 摘要优先（含亮点/技术栈等维度），否则原始描述 */}
          {(item.aiSummary || item.description) && (
            <p className="text-sm text-[var(--text-secondary)] mt-1.5 line-clamp-3 leading-relaxed whitespace-pre-line">
              {item.aiSummary || item.description}
            </p>
          )}

          {/* AI标签 */}
          {item.aiTags && item.aiTags.length > 0 && (
            <div className="flex flex-wrap gap-1 mt-2">
              {item.aiTags.map((tag: string) => (
                <span key={tag} className="hb-chip">
                  {tag}
                </span>
              ))}
            </div>
          )}

          {/* 元数据 + 下钻提示 */}
          <div className="flex items-center gap-3 mt-2">
            {item.author && (
              <span className="text-[12px] text-[var(--text-tertiary)]">@{item.author}</span>
            )}
            {item.language && (
              <span className="text-[12px] text-[var(--text-tertiary)]">{item.language}</span>
            )}
            {item.stars != null && (
              <span className="flex items-center gap-0.5 text-[12px] text-[var(--warning)]">
                <Star size={11} fill="currentColor" />
                {item.stars.toLocaleString()}
              </span>
            )}
            {item.forks != null && (
              <span className="flex items-center gap-0.5 text-[12px] text-[var(--text-tertiary)]">
                <GitFork size={11} />
                {item.forks.toLocaleString()}
              </span>
            )}
            <span className="text-[12px] text-[var(--text-tertiary)]">
              {new Date(item.publishedAt).toLocaleDateString('zh-CN')}
            </span>
            <span className="ml-auto flex items-center gap-0.5 text-[12px] text-[var(--text-tertiary)] group-hover:text-[var(--accent)] transition-colors">
              详情
              <ChevronRight size={11} />
            </span>
          </div>
        </div>

        {/* 操作按钮 */}
        <div className="shrink-0 flex flex-col gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
          {onSediment && (
            <button
              onClick={(e) => { e.stopPropagation(); onSediment(item.id); }}
              disabled={sedimenting}
              className={`p-1.5 rounded-md transition-colors disabled:opacity-50 ${
                sedimented
                  ? 'text-[var(--accent)] bg-[var(--accent-subtle)]'
                  : 'text-[var(--text-tertiary)] hover:text-[var(--accent)] hover:bg-[var(--accent-subtle)]'
              }`}
              title={sedimented ? '打开已沉淀的笔记' : '存为笔记（带来源与证据）'}
            >
              {sedimenting ? <Loader2 size={14} className="animate-spin" /> : <NotebookPen size={14} />}
            </button>
          )}
          <button
            onClick={(e) => { e.stopPropagation(); onStar(item.id); }}
            className={`p-1.5 rounded-md transition-colors ${
              isStarred ? 'text-[var(--gold)] bg-[var(--warning-subtle)]' : 'text-[var(--text-tertiary)] hover:text-[var(--gold)] hover:bg-[var(--bg-sunken)]'
            }`}
            title={isStarred ? '取消收藏' : '收藏'}
          >
            <Star size={14} fill={isStarred ? 'currentColor' : 'none'} />
          </button>
          <button
            onClick={(e) => { e.stopPropagation(); onArchive(item.id); }}
            className={`p-1.5 rounded-md transition-colors ${
              isArchived ? 'text-[var(--text-secondary)] bg-[var(--bg-sunken)]' : 'text-[var(--text-tertiary)] hover:text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'
            }`}
            title="归档"
          >
            <Archive size={14} />
          </button>
          <button
            onClick={async (e) => {
              e.stopPropagation();
              if (await confirmDialog('删除这条内容？将同时删除其向量索引，且不可恢复。', { title: 'SophoNote', kind: 'warning' })) {
                onDelete(item.id);
              }
            }}
            className="p-1.5 rounded-md text-[var(--text-tertiary)] hover:text-[var(--danger)] hover:bg-[var(--danger-subtle)] transition-colors"
            title="删除（连带向量索引）"
          >
            <Trash2 size={14} />
          </button>
        </div>
      </div>
    </div>
  );
}
