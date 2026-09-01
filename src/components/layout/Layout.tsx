import { useEffect, useState } from 'react';
import { PanelLeftOpen } from 'lucide-react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getSetting, updateSetting } from '../../services/tauri';
import Sidebar from './Sidebar';
import VerticalResizeHandle from '../ui/VerticalResizeHandle';

interface LayoutProps {
  children: React.ReactNode;
}

const COLLAPSE_KEY = 'ui:sidebar-collapsed';
const SIDEBAR_WIDTH_KEY = 'ui:sidebar-width';
const DEFAULT_SIDEBAR_WIDTH = 224;
const MIN_SIDEBAR_WIDTH = 184;

/** 布局容器：Sidebar + main。
 *  AG-16 用户指令：SophoNote 侧栏折叠 = 完全隐藏（不留窄轨），
 *  展开按钮固定在主内容左上角（与侧栏首行折叠按钮位置一致——红绿灯右侧 pl-[70px]）。
 *  折叠状态提升到 Layout，Sidebar 只渲染展开态。
 */
export default function Layout({ children }: LayoutProps) {
  const [collapsed, setCollapsed] = useState(false);
  // NB-17：全屏时红绿灯隐藏，展开按钮贴最左 pl-2（与侧栏首行全屏态同口径）
  const [fullscreen, setFullscreen] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(DEFAULT_SIDEBAR_WIDTH);
  const [viewportWidth, setViewportWidth] = useState(() => window.innerWidth);
  const maxSidebarWidth = Math.max(
    MIN_SIDEBAR_WIDTH,
    Math.min(360, viewportWidth - 520)
  );
  const visibleSidebarWidth = Math.min(sidebarWidth, maxSidebarWidth);

  useEffect(() => {
    const win = getCurrentWindow();
    let alive = true;
    const sync = () => {
      win.isFullscreen().then((v) => { if (alive) setFullscreen(v); }).catch(() => {});
    };
    sync();
    const unlisten = win.onResized(sync);
    return () => {
      alive = false;
      unlisten.then((off) => off()).catch(() => {});
    };
  }, []);

  useEffect(() => {
    const syncWidth = () => setViewportWidth(window.innerWidth);
    window.addEventListener('resize', syncWidth);
    return () => window.removeEventListener('resize', syncWidth);
  }, []);

  useEffect(() => {
    let alive = true;
    Promise.all([getSetting(COLLAPSE_KEY), getSetting(SIDEBAR_WIDTH_KEY)])
      .then(([collapseValue, widthValue]) => {
        if (!alive) return;
        setCollapsed(collapseValue === '1');
        const savedWidth = Number(widthValue);
        if (Number.isFinite(savedWidth)) {
          setSidebarWidth(Math.max(MIN_SIDEBAR_WIDTH, Math.min(360, savedWidth)));
        }
      })
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  const toggleCollapsed = () => {
    setCollapsed((v) => {
      const next = !v;
      updateSetting(COLLAPSE_KEY, next ? '1' : '0').catch(() => {});
      return next;
    });
  };

  return (
    <div className="flex h-full bg-[var(--bg-canvas)]">
      {/* 侧栏折叠 = 完全隐藏（AG-16 用户指令：不留窄轨） */}
      {!collapsed && (
        <>
          <Sidebar onToggleCollapse={toggleCollapsed} width={visibleSidebarWidth} />
          <VerticalResizeHandle
            value={visibleSidebarWidth}
            min={MIN_SIDEBAR_WIDTH}
            max={maxSidebarWidth}
            defaultValue={DEFAULT_SIDEBAR_WIDTH}
            onChange={setSidebarWidth}
            onCommit={(width) => void updateSetting(SIDEBAR_WIDTH_KEY, String(Math.round(width)))}
            label="调整主导航宽度"
          />
        </>
      )}
      <main className="flex-1 flex flex-col min-w-0 overflow-hidden relative">
        {/* AG-16：侧栏折叠态 → 展开按钮固定在主内容左上角（与侧栏首行折叠按钮位置一致）。
            红绿灯占位 pl-[70px]（窗态）/ pl-2（全屏态），与 Sidebar 首行 padding 同口径。
            按钮规格 w-7 h-7 icon15 与侧栏首行折叠按钮完全一致（位置/尺寸/视觉）。 */}
        {collapsed && (
          <div
            className={`absolute top-0 left-0 h-10 flex items-center ${fullscreen ? 'pl-2' : 'pl-[70px]'} z-30 shrink-0`}
            data-tauri-drag-region
          >
            <button
              onClick={toggleCollapsed}
              title="展开侧栏"
              className="w-7 h-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] shrink-0 transition-colors"
            >
              <PanelLeftOpen size={15} />
            </button>
          </div>
        )}
        {children}
      </main>
    </div>
  );
}
