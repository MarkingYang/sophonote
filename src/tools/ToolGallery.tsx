import { ChevronRight } from 'lucide-react';
import { toolRegistry } from './registry';
import type { ToolCategory, ToolDefinition } from './types';
import { useAppStore } from '../stores/appStore';

/**
 * 工具库画廊（DEC-041）：工具域落地页。
 * 按分类分组展示全部已注册工具，点击进入该工具的独立整页。
 */

const CATEGORY_ORDER: ToolCategory[] = ['行动管理', '专注节奏'];

function ToolCard({ tool }: { tool: ToolDefinition }) {
  const openTool = useAppStore((s) => s.openTool);
  const Icon = tool.icon;
  return (
    <button
      onClick={() => openTool(tool.id)}
      className="group text-left rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-4 hover:border-[var(--accent-border)] hover:shadow-sm transition-all"
    >
      <div className="flex items-center gap-2.5 mb-2">
        <span className="w-8 h-8 rounded-md bg-[var(--accent-subtle)] text-[var(--accent)] flex items-center justify-center shrink-0">
          <Icon size={16} />
        </span>
        <span className="text-sm font-semibold text-[var(--text-primary)]">{tool.title}</span>
        <ChevronRight
          size={14}
          className="ml-auto text-[var(--text-tertiary)] opacity-0 group-hover:opacity-100 transition-opacity"
        />
      </div>
      <p className="text-xs text-[var(--text-secondary)] leading-relaxed mb-2">{tool.description}</p>
      <div className="flex flex-wrap gap-1">
        {tool.keywords.slice(0, 3).map((k) => (
          <span
            key={k}
            className="text-[10px] px-1.5 py-0.5 rounded bg-[var(--bg-sunken)] text-[var(--text-tertiary)]"
          >
            {k}
          </span>
        ))}
      </div>
    </button>
  );
}

export default function ToolGallery() {
  const categories = CATEGORY_ORDER.filter((c) => toolRegistry.some((t) => t.category === c));

  return (
    <div className="h-full overflow-y-auto p-5">
      <div className="max-w-3xl mx-auto">
        {categories.map((category) => (
          <section key={category} className="mb-6">
            <h3 className="text-xs font-semibold text-[var(--text-tertiary)] tracking-wide mb-3">{category}</h3>
            <div className="grid grid-cols-2 gap-3">
              {toolRegistry
                .filter((t) => t.category === category)
                .map((tool) => (
                  <ToolCard key={tool.id} tool={tool} />
                ))}
            </div>
          </section>
        ))}
        <p className="text-xs text-[var(--text-tertiary)] text-center mt-8">
          日历、提醒、闹钟等工具将陆续注册进入
        </p>
      </div>
    </div>
  );
}
