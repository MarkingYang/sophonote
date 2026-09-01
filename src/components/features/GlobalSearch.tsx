import { useEffect, useRef, useState } from 'react';
import { Search, NotebookPen, BookOpen, Inbox, Loader2 } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import { globalSearch, type GlobalHit } from '../../services/tauri';
import SearchHighlight from './SearchHighlight';

/** NB-14 全局搜索面板：笔记 / 深度解读 / 收件箱条目三域统一检索。
 * 关键词 + 语义由后端 global_search 融合排序，前端只呈现统一结果——
 * 不区分、不展示命中来自哪条检索通道（用户指令）。
 * 入口：⌘K / Ctrl+K，或侧栏搜索按钮（CustomEvent 'sophonote:global-search'）。
 * 替代原 QuickSwitcher（仅 articles 内存搜索）。
 */

const GROUP_LABEL: Record<GlobalHit['kind'], string> = {
  note: '笔记',
  article: '深度解读',
  item: '收件箱',
};

const ITEM_TYPE_LABEL: Record<string, string> = {
  github: 'GitHub',
  arxiv: 'arXiv',
  hackernews: 'HN',
  huggingface: 'HF',
  huggingface_papers: '论文',
  producthunt: 'PH',
  aihot: 'AIHOT',
};

function badgeOf(hit: GlobalHit): string {
  if (hit.kind === 'item') return ITEM_TYPE_LABEL[hit.sub_type ?? ''] ?? (hit.sub_type ?? '条目');
  if (hit.sub_type === 'journal') return '日记';
  if (hit.sub_type === 'manual') return '笔记';
  return '解读';
}

export default function GlobalSearch() {
  const requestOpenArticle = useAppStore((s) => s.requestOpenArticle);
  const setSelectedItemId = useAppStore((s) => s.setSelectedItemId);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [hits, setHits] = useState<GlobalHit[]>([]);
  const [loading, setLoading] = useState(false);
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const seqRef = useRef(0);

  // ⌘K / Ctrl+K 开关；侧栏搜索按钮经 CustomEvent 唤起
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setOpen((v) => !v);
        setQuery('');
        setHits([]);
        setActive(0);
      }
    };
    const onOpenEvent = () => {
      setOpen(true);
      setQuery('');
      setHits([]);
      setActive(0);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('sophonote:global-search', onOpenEvent);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('sophonote:global-search', onOpenEvent);
    };
  }, []);

  useEffect(() => {
    if (open) requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  // 防抖请求后端融合检索；seq 丢弃过期响应
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    const seq = ++seqRef.current;
    const timer = setTimeout(() => {
      globalSearch(q, 30)
        .then((r) => {
          if (seqRef.current !== seq) return;
          setHits(r);
          setActive(0);
          setLoading(false);
        })
        .catch(() => {
          if (seqRef.current !== seq) return;
          setHits([]);
          setLoading(false);
        });
    }, 250);
    return () => clearTimeout(timer);
  }, [query]);

  // 按类型分组（组内保持后端统一排序），拼平铺列表供 ↑↓ 导航
  const groups: { kind: GlobalHit['kind']; items: GlobalHit[] }[] = [];
  for (const kind of ['note', 'article', 'item'] as const) {
    const items = hits.filter((h) => h.kind === kind);
    if (items.length > 0) groups.push({ kind, items });
  }
  const flat = groups.flatMap((g) => g.items);

  const jump = (hit: GlobalHit) => {
    if (hit.kind === 'item') {
      setSelectedItemId(hit.id);
    } else {
      requestOpenArticle(hit.id);
    }
    setOpen(false);
    setQuery('');
    setHits([]);
  };

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-50 bg-[var(--overlay-scrim-strong)] flex items-start justify-center pt-[14vh]"
      onClick={() => setOpen(false)}
    >
      <div
        className="w-[560px] max-w-[92vw] rounded-xl border border-border bg-[var(--bg-surface)] shadow-[var(--shadow-lg)] overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        {/* 搜索框 */}
        <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
          <Search size={15} className="text-[var(--text-tertiary)] shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown') {
                e.preventDefault();
                setActive((i) => Math.min(i + 1, flat.length - 1));
              } else if (e.key === 'ArrowUp') {
                e.preventDefault();
                setActive((i) => Math.max(i - 1, 0));
              } else if (e.key === 'Enter' && flat[active]) {
                jump(flat[active]);
              } else if (e.key === 'Escape') {
                setOpen(false);
              }
            }}
            placeholder="全局搜索：笔记、解读、收件箱…（↑↓ 选择，Enter 打开，Esc 关闭）"
            className="flex-1 bg-transparent text-sm text-[var(--text-primary)] placeholder:text-[var(--text-disabled)] focus:outline-none"
          />
          {loading && <Loader2 size={13} className="text-[var(--text-tertiary)] animate-spin shrink-0" />}
          <kbd className="font-mono text-[13px] px-1.5 py-0.5 rounded-[6px] bg-[var(--bg-sunken)] text-[var(--text-tertiary)] shrink-0">⌘K</kbd>
        </div>
        {/* 结果列表：按类型分组跳转，组内为后端统一排序（不展示检索通道） */}
        <div className="max-h-[46vh] overflow-y-auto py-1.5">
          {query.trim() === '' && (
            <p className="text-xs text-[var(--text-tertiary)] text-center py-8">输入关键词，统一检索笔记、深度解读与收件箱</p>
          )}
          {query.trim() !== '' && !loading && flat.length === 0 && (
            <p className="text-xs text-[var(--text-tertiary)] text-center py-8">没有匹配的结果</p>
          )}
          {groups.map((g) => {
            const offset = flat.indexOf(g.items[0]);
            return (
              <div key={g.kind}>
                <div className="px-4 pt-2 pb-1 text-[12px] font-semibold text-[var(--text-tertiary)] uppercase tracking-wide">
                  {GROUP_LABEL[g.kind]}
                  <span className="ml-1.5 font-normal">{g.items.length}</span>
                </div>
                {g.items.map((h, i) => {
                  const idx = offset + i;
                  return (
                    <button
                      key={`${h.kind}-${h.id}`}
                      onClick={() => jump(h)}
                      onMouseEnter={() => setActive(idx)}
                      className={`w-full flex items-center gap-2.5 px-4 py-2 text-left transition-colors ${
                        idx === active ? 'bg-[var(--accent-subtle)]' : ''
                      }`}
                    >
                      {h.kind === 'note' ? (
                        <NotebookPen size={14} className="text-[var(--accent)] shrink-0" />
                      ) : h.kind === 'article' ? (
                        <BookOpen size={14} className="text-[var(--success)] shrink-0" />
                      ) : (
                        <Inbox size={14} className="text-[var(--dot-paper)] shrink-0" />
                      )}
                      <span className="flex-1 min-w-0">
                        <span className="block text-[13px] font-medium text-[var(--text-primary)] truncate">
                          <SearchHighlight text={h.title} query={query.trim()} />
                        </span>
                        {h.snippet && (
                          <span className="block text-[12px] text-[var(--text-tertiary)] truncate">
                            <SearchHighlight text={h.snippet} query={query.trim()} />
                          </span>
                        )}
                      </span>
                      <span
                        className={`text-[12px] px-1.5 py-0.5 rounded shrink-0 ${
                          h.kind === 'note'
                            ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                            : h.kind === 'article'
                              ? 'bg-[var(--success-subtle)] text-[var(--success)]'
                              : 'bg-[color-mix(in_srgb,var(--dot-paper)_12%,transparent)] text-[var(--dot-paper)]'
                        }`}
                      >
                        {badgeOf(h)}
                      </span>
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
