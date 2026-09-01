import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

/**
 * 全站统一空状态：48px 线性图标（--text-disabled）+ 14px/500 标题 + 12px 说明
 * + 可选行动按钮，容器内垂直居中（.hb-empty，见 index.css）。
 */
interface EmptyStateProps {
  icon: LucideIcon;
  title: string;
  desc?: string;
  action?: ReactNode;
  /** 容器附加类名（如 py-24） */
  className?: string;
}

export default function EmptyState({ icon: Icon, title, desc, action, className = '' }: EmptyStateProps) {
  return (
    <div className={`hb-empty ${className}`.trim()}>
      <Icon size={48} strokeWidth={1.2} className="hb-empty-icon" aria-hidden />
      <p className="hb-empty-title">{title}</p>
      {desc && <p className="hb-empty-desc">{desc}</p>}
      {action && <div className="hb-empty-action">{action}</div>}
    </div>
  );
}
