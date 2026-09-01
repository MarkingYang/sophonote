import { useEffect, useRef } from 'react';

interface VerticalResizeHandleProps {
  value: number;
  min: number;
  max: number;
  defaultValue: number;
  /** 右拖变宽为 1；右拖变窄为 -1（右侧面板）。 */
  direction?: 1 | -1;
  onChange: (width: number) => void;
  onCommit?: (width: number) => void;
  label: string;
}

const clamp = (value: number, min: number, max: number) =>
  Math.min(Math.max(value, min), Math.max(min, max));

/**
 * 统一竖向分隔线：鼠标/触控拖拽、键盘方向键、双击复位。
 * 6px 命中区保持易操作，视觉仍是一条 1px 细线。
 */
export default function VerticalResizeHandle({
  value,
  min,
  max,
  defaultValue,
  direction = 1,
  onChange,
  onCommit,
  label,
}: VerticalResizeHandleProps) {
  const cleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => () => cleanupRef.current?.(), []);

  const apply = (next: number, commit = false) => {
    const width = clamp(next, min, max);
    onChange(width);
    if (commit) onCommit?.(width);
  };

  return (
    <div
      role="separator"
      aria-label={label}
      aria-orientation="vertical"
      aria-valuemin={Math.round(min)}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(value)}
      tabIndex={0}
      className="group relative z-20 w-1.5 shrink-0 cursor-col-resize touch-none bg-transparent focus-visible:outline-none"
      title="拖拽调整宽度；双击恢复默认"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        const startX = event.clientX;
        const startWidth = value;
        let latest = value;
        document.body.style.cursor = 'col-resize';
        document.body.style.userSelect = 'none';

        const cleanup = () => {
          window.removeEventListener('pointermove', onMove);
          window.removeEventListener('pointerup', onUp);
          window.removeEventListener('pointercancel', onUp);
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
          cleanupRef.current = null;
        };
        const onMove = (moveEvent: PointerEvent) => {
          latest = clamp(startWidth + direction * (moveEvent.clientX - startX), min, max);
          onChange(latest);
        };
        const onUp = () => {
          cleanup();
          onCommit?.(latest);
        };
        cleanupRef.current?.();
        cleanupRef.current = cleanup;
        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', onUp);
        window.addEventListener('pointercancel', onUp);
      }}
      onDoubleClick={() => apply(defaultValue, true)}
      onKeyDown={(event) => {
        const step = event.shiftKey ? 24 : 8;
        if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
          event.preventDefault();
          const delta = event.key === 'ArrowRight' ? step : -step;
          apply(value + direction * delta, true);
        } else if (event.key === 'Home') {
          event.preventDefault();
          apply(defaultValue, true);
        }
      }}
    >
      <span className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-[var(--border-default)] transition-[width,background-color,opacity] duration-150 group-hover:w-0.5 group-hover:bg-[var(--accent)] group-focus-visible:w-0.5 group-focus-visible:bg-[var(--accent)]" />
    </div>
  );
}
