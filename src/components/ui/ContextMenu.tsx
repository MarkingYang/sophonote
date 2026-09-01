import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';

/**
 * NB-13：Obsidian 形态右键上下文菜单（三空间共用：笔记本 / 深度解读 / AI 工作室）。
 *
 * 用法：列表项 onContextMenu → setMenu({x, y, items})；mask 点外 / Esc 关闭。
 * danger 项内置两步确认：首击切换为红色确认文案（confirmLabel），3s 回落，再击执行。
 */

export interface CtxMenuItem {
  label: string;
  icon?: ReactNode;
  /** 危险动作（如删除）：两步确认 */
  danger?: boolean;
  /** 两步确认文案，默认「确认删除？」 */
  confirmLabel?: string;
  onClick: () => void;
}

export interface CtxMenuState {
  x: number;
  y: number;
  items: CtxMenuItem[];
}

const MENU_W = 176;
const ITEM_H = 32;

export default function ContextMenu({
  menu,
  onClose,
}: {
  menu: CtxMenuState | null;
  onClose: () => void;
}) {
  const [confirmIdx, setConfirmIdx] = useState<number | null>(null);
  const confirmTimer = useRef<number | undefined>(undefined);

  // 菜单实例变化（重新右键/关闭）时重置确认态
  useEffect(() => {
    setConfirmIdx(null);
    return () => window.clearTimeout(confirmTimer.current);
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [menu, onClose]);

  if (!menu) return null;

  // 视口夹取：右下放不下则翻转
  const h = menu.items.length * ITEM_H + 8;
  const x = Math.min(menu.x, window.innerWidth - MENU_W - 8);
  const y = Math.min(menu.y, window.innerHeight - h - 8);

  return (
    <>
      <div className="fixed inset-0 z-40" onClick={onClose} onContextMenu={(e) => { e.preventDefault(); onClose(); }} />
      <div
        className="fixed z-50 py-1 rounded-lg border border-border bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]"
        style={{ left: x, top: y, width: MENU_W }}
      >
        {menu.items.map((it, i) => {
          const confirming = confirmIdx === i;
          return (
            <button
              key={i}
              onClick={() => {
                if (it.danger && !confirming) {
                  setConfirmIdx(i);
                  window.clearTimeout(confirmTimer.current);
                  confirmTimer.current = window.setTimeout(() => setConfirmIdx(null), 3000);
                  return;
                }
                onClose();
                it.onClick();
              }}
              className={`w-full flex items-center gap-2 px-3 text-xs text-left transition-colors ${
                confirming
                  ? 'bg-[var(--danger)] text-[var(--bg-surface)] font-medium'
                  : it.danger
                    ? 'text-[var(--danger)] hover:bg-[var(--danger-subtle)]'
                    : 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]'
              }`}
              style={{ height: ITEM_H }}
            >
              {it.icon && <span className="shrink-0 opacity-70">{it.icon}</span>}
              <span className="truncate">{confirming ? (it.confirmLabel ?? '确认删除？') : it.label}</span>
            </button>
          );
        })}
      </div>
    </>
  );
}
