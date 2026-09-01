import {
  Compass,
  Sparkles,
  Wrench,
  Settings,
  NotebookPen,
  MessageSquareText,
  Search,
  PanelLeftClose,
  CalendarClock,
} from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState } from 'react';
import { preloadPage } from '../../services/pagePreload';

export const navItems = [
  { id: 'discover', label: '发现', icon: Compass },
  { id: 'conversation', label: '会话', icon: MessageSquareText },
  { id: 'ai-studio', label: '工作室', icon: Sparkles },
  { id: 'notes', label: '笔记本', icon: NotebookPen },
  { id: 'scheduled-tasks', label: '计划任务', icon: CalendarClock },
  { id: 'tasks', label: '工具', icon: Wrench },
];

/** 唤起全局搜索面板（GlobalSearch 监听该事件，与 ⌘K 同入口） */
function openGlobalSearch() {
  window.dispatchEvent(new Event('sophonote:global-search'));
}

interface SidebarProps {
  onToggleCollapse: () => void;
  width?: number;
}

/** NB-14/15/16/17/25 + AG-16 侧栏：折叠态由 Layout 管理（完全隐藏，不留窄轨）。
 *  本组件只渲染展开态（w-56）。折叠/展开按钮固定在首行（红绿灯右侧）。
 *  AG-16 用户指令：「SophoNote 上方首行这一行是一个完整的区域，折叠按钮固定在这个位置。
 *  折叠按钮点击之后 SophoNote 部分区域会完全折叠，不可见。再次点击恢复。」 */
export default function Sidebar({ onToggleCollapse, width = 224 }: SidebarProps) {
  const activePage = useAppStore((state) => state.activePage);
  const setActivePage = useAppStore((state) => state.setActivePage);
  // NB-17：全屏时 macOS 红绿灯隐藏 → 首行撤让位区、按钮贴到最左
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const sync = () => {
      win
        .isFullscreen()
        .then((v) => {
          if (alive) setFullscreen(v);
        })
        .catch(() => {});
    };
    sync();
    const unlisten = win.onResized(sync);
    return () => {
      alive = false;
      unlisten.then((off) => off()).catch(() => {});
    };
  }, []);

  return (
    <aside
      className="h-full bg-[var(--bg-canvas)] flex flex-col shrink-0 overflow-hidden"
      style={{ width }}
    >
      {/* NB-15 首行：与 macOS 红绿灯（缩小/放大/关闭）同行——第一区域 = 折叠 + 搜索（用户指令）。
          左 70px 为红绿灯让位区；空白处 data-tauri-drag-region 可拖拽窗口。
          NB-17：全屏撤让位贴最左。
          AG-16：折叠按钮固定在此首行（位置不变），点击后侧栏完全隐藏。 */}
      <div
        className={`h-10 border-b border-[var(--border-default)] flex items-center gap-1 ${fullscreen ? 'pl-2' : 'pl-[70px]'} pr-2 shrink-0`}
        data-tauri-drag-region
      >
        <button
          onClick={onToggleCollapse}
          title="折叠侧栏"
          className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] shrink-0 transition-colors"
        >
          <PanelLeftClose size={15} />
        </button>
        <button
          onClick={openGlobalSearch}
          title="全局搜索（⌘K）"
          className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] shrink-0 transition-colors"
        >
          <Search size={15} />
        </button>
        <div className="flex-1 h-full" data-tauri-drag-region />
      </div>

      {/* 品牌（首行下方，NB-15 下移一行） */}
      <div className="px-4 py-2.5">
        <div className="flex items-center gap-2.5">
          <img
            src="/logo-light.png"
            alt="SophoNote"
            className="h-7 w-7 shrink-0 select-none rounded-md object-cover dark:hidden"
            draggable={false}
          />
          <img
            src="/logo.png"
            alt=""
            aria-hidden="true"
            className="hidden h-7 w-7 shrink-0 select-none rounded-md object-cover dark:block"
            draggable={false}
          />
          <div className="flex-1 min-w-0">
            <h1 className="text-[13px] font-bold text-[var(--text-primary)] leading-tight">SophoNote</h1>
            <p className="text-[12px] text-[var(--text-tertiary)] leading-tight">AI 知识管家</p>
          </div>
        </div>
      </div>

      {/* 导航 */}
      <nav className="flex-1 p-2 overflow-y-auto">
        <div className="space-y-0.5">
          {navItems.map((item) => {
            const Icon = item.icon;
            const isActive = activePage === item.id;
            return (
              <button
                key={item.id}
                onClick={() => setActivePage(item.id)}
                onPointerEnter={() => void preloadPage(item.id)?.catch(() => {})}
                onFocus={() => void preloadPage(item.id)?.catch(() => {})}
                className={`relative w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-[13px] font-medium transition-colors text-left ${
                  isActive
                    ? 'bg-[var(--accent-subtle)] text-[var(--accent)] font-semibold before:absolute before:left-0 before:top-1.5 before:bottom-1.5 before:w-[3px] before:rounded-full before:bg-[var(--accent)] before:content-[\'\']'
                    : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
                }`}
              >
                <Icon size={16} />
                {item.label}
              </button>
            );
          })}
        </div>
      </nav>

      {/* 底部 */}
      <div className="p-2 border-t border-[var(--border-default)]">
        <button
          onClick={() => setActivePage('settings')}
          onPointerEnter={() => void preloadPage('settings')?.catch(() => {})}
          onFocus={() => void preloadPage('settings')?.catch(() => {})}
          className={`relative w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-[13px] font-medium transition-colors text-left ${
            activePage === 'settings'
              ? 'bg-[var(--accent-subtle)] text-[var(--accent)] font-semibold before:absolute before:left-0 before:top-1.5 before:bottom-1.5 before:w-[3px] before:rounded-full before:bg-[var(--accent)] before:content-[\'\']'
              : 'text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]'
          }`}
        >
          <Settings size={16} />
          设置
        </button>
      </div>
    </aside>
  );
}
