import { ArrowLeft } from 'lucide-react';
import { useAppStore } from '../stores/appStore';
import { getTool } from '../tools/registry';
import ToolGallery from '../tools/ToolGallery';

/**
 * 工具域壳（DEC-041）：工具域 = 注册式组件库。
 * - activeToolId 为空 → 工具库画廊（落地页）；
 * - 打开某工具 → 渲染该工具自包含的独立整页，页头提供返回画廊入口。
 * 壳层不硬编码任何工具，全部由 tools/registry.ts 驱动。
 */
export default function Tasks() {
  const activeToolId = useAppStore((s) => s.activeToolId);
  const openTool = useAppStore((s) => s.openTool);
  const activeTool = getTool(activeToolId);

  return (
    <div className="flex flex-col h-full">
      {/* NB-20：首行空白可拖窗；40px 统一页头 */}
      <header
        className="px-5 h-10 border-b border-[var(--border-default)] flex items-center gap-2 bg-[var(--bg-surface)]"
        data-tauri-drag-region
      >
        {activeTool && (
          <button
            onClick={() => openTool(null)}
            title="返回工具库"
            className="w-6 h-6 -ml-1 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)] transition-colors"
          >
            <ArrowLeft size={14} />
          </button>
        )}
        <h2 className="text-base font-semibold text-[var(--text-primary)]" data-tauri-drag-region>
          {activeTool ? `工具 · ${activeTool.title}` : '工具'}
        </h2>
      </header>
      <div className="flex-1 min-h-0">{activeTool ? <activeTool.Component /> : <ToolGallery />}</div>
    </div>
  );
}
