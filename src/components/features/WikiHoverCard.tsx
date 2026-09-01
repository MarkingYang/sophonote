import { useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { NotebookPen } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import { calcCardPos, CARD_W, CARD_MAX_H, type CardPos } from '../../services/hoverCard';
import { findHeadingLine, findBlockLine } from '../../services/noteLinks';
import MarkdownView from './MarkdownView';

/**
 * NB-09 链接悬停预览卡（对标 Obsidian Page Preview 核心插件）：
 * 预览态悬停 [[双链]] 约 260ms 后弹出目标笔记预览卡，不跳转即可预览全文；
 * 移入卡片可继续滚动阅读（160ms 离开容忍窗口防误关），点击「打开」或卡片内链接才跳转。
 *
 * 卡片经 createPortal 挂到 body——链接处于 <p> 内，直接渲染 fixed <div> 会破坏 HTML 嵌套；
 * 卡片内的 MarkdownView 不再开 hoverPreview，杜绝悬停卡无限套娃。
 */

const SHOW_DELAY = 260;
const HIDE_DELAY = 160;

export function HoverWikiLink({
  title,
  onOpen,
  children,
}: {
  title: string;
  onOpen: () => void;
  children: ReactNode;
}) {
  const [pos, setPos] = useState<CardPos | null>(null);
  const anchorRef = useRef<HTMLAnchorElement>(null);
  const showTimer = useRef<number | null>(null);
  const hideTimer = useRef<number | null>(null);

  const clearTimers = () => {
    if (showTimer.current != null) {
      window.clearTimeout(showTimer.current);
      showTimer.current = null;
    }
    if (hideTimer.current != null) {
      window.clearTimeout(hideTimer.current);
      hideTimer.current = null;
    }
  };
  useEffect(() => clearTimers, []);

  const scheduleShow = () => {
    clearTimers();
    showTimer.current = window.setTimeout(() => {
      const el = anchorRef.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      setPos(
        calcCardPos({
          rectTop: r.top,
          rectBottom: r.bottom,
          rectLeft: r.left,
          viewW: window.innerWidth,
          viewH: window.innerHeight,
        })
      );
    }, SHOW_DELAY);
  };

  const scheduleHide = () => {
    clearTimers();
    hideTimer.current = window.setTimeout(() => setPos(null), HIDE_DELAY);
  };

  return (
    <>
      <a
        ref={anchorRef}
        href="#"
        className="wiki-link"
        onClick={(e) => {
          e.preventDefault();
          setPos(null);
          onOpen();
        }}
        onMouseEnter={scheduleShow}
        onMouseLeave={scheduleHide}
      >
        {children}
      </a>
      {pos &&
        createPortal(
          <WikiHoverCard
            title={title}
            pos={pos}
            onClose={() => setPos(null)}
            onNavigate={onOpen}
            onMouseEnter={clearTimers}
            onMouseLeave={scheduleHide}
          />,
          document.body
        )}
    </>
  );
}

function WikiHoverCard({
  title,
  pos,
  onClose,
  onNavigate,
  onMouseEnter,
  onMouseLeave,
}: {
  title: string;
  pos: CardPos;
  onClose: () => void;
  /** 点击「打开」：走链接同款跳转（含不存在即创建） */
  onNavigate: () => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
}) {
  const articles = useAppStore((s) => s.articles);
  const requestOpenArticle = useAppStore((s) => s.requestOpenArticle);
  const openArticleAtLine = useAppStore((s) => s.openArticleAtLine);
  const target = articles.find((a) => a.title === title);

  // 卡片内双链点击：按标题解析后跨空间路由跳转，并关闭本卡片；
  // NB-10：标题链接（heading）经 findHeadingLine 定位到目标标题行
  const openInner = (t: string, heading?: string, blockId?: string) => {
    const hit = articles.find((a) => a.title === t);
    onClose();
    if (!hit) return;
    if (blockId) {
      const line = findBlockLine(hit.content, blockId);
      if (line != null) { openArticleAtLine(hit.id, line); return; }
    }
    if (heading) {
      const line = findHeadingLine(hit.content, heading);
      if (line != null) { openArticleAtLine(hit.id, line); return; }
    }
    requestOpenArticle(hit.id);
  };

  return (
    <div
      className="fixed z-50 flex flex-col rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] overflow-hidden"
      style={{
        left: pos.left,
        width: CARD_W,
        maxHeight: CARD_MAX_H,
        ...(pos.above ? { bottom: window.innerHeight - pos.y } : { top: pos.y }),
      }}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
    >
      <div className="flex items-center gap-2 px-3.5 py-2 border-b border-[var(--border-default)] bg-[var(--bg-sunken)] shrink-0">
        <NotebookPen size={13} className="text-[var(--accent)] shrink-0" />
        <p className="flex-1 min-w-0 text-xs font-bold text-[var(--text-primary)] truncate" title={title}>
          {title}
        </p>
        <button
          onClick={() => {
            onClose();
            onNavigate();
          }}
          className="text-xs font-medium text-[var(--accent)] hover:text-[var(--accent-strong)] transition-colors shrink-0"
        >
          打开
        </button>
      </div>
      {target ? (
        <div className="flex-1 overflow-y-auto px-3.5 py-2.5">
          {target.content.trim() ? (
            <MarkdownView content={target.content} onOpenArticle={openInner} />
          ) : (
            <p className="text-xs text-[var(--text-tertiary)] py-2">（空笔记）</p>
          )}
        </div>
      ) : (
        <p className="px-3.5 py-3 text-xs text-[var(--text-tertiary)]">笔记尚不存在，点击链接即可创建</p>
      )}
    </div>
  );
}
