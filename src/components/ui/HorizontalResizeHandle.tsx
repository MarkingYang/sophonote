import { useEffect, useRef } from 'react';

interface HorizontalResizeHandleProps {
  value: number;
  min: number;
  max: number;
  defaultValue: number;
  direction?: 1 | -1;
  onChange: (height: number) => void;
  onCommit?: (height: number) => void;
  label: string;
}

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), Math.max(min, max));

export default function HorizontalResizeHandle({
  value,
  min,
  max,
  defaultValue,
  direction = -1,
  onChange,
  onCommit,
  label,
}: HorizontalResizeHandleProps) {
  const cleanupRef = useRef<(() => void) | null>(null);
  useEffect(() => () => cleanupRef.current?.(), []);

  return (
    <div
      role="separator"
      aria-label={label}
      aria-orientation="horizontal"
      aria-valuemin={Math.round(min)}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(value)}
      tabIndex={0}
      className="group relative z-20 h-1.5 shrink-0 cursor-row-resize touch-none bg-transparent focus-visible:outline-none"
      title="拖拽调整高度；双击恢复默认"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        const startY = event.clientY;
        const startHeight = value;
        let latest = value;
        document.body.style.cursor = 'row-resize';
        document.body.style.userSelect = 'none';
        const onMove = (moveEvent: PointerEvent) => {
          latest = clamp(startHeight + direction * (moveEvent.clientY - startY), min, max);
          onChange(latest);
        };
        const cleanup = () => {
          window.removeEventListener('pointermove', onMove);
          window.removeEventListener('pointerup', cleanup);
          window.removeEventListener('pointercancel', cleanup);
          document.body.style.cursor = '';
          document.body.style.userSelect = '';
          cleanupRef.current = null;
        };
        cleanupRef.current?.();
        cleanupRef.current = cleanup;
        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', cleanup);
        window.addEventListener('pointercancel', cleanup);
      }}
      onDoubleClick={() => { onChange(defaultValue); onCommit?.(defaultValue); }}
      onKeyDown={(event) => {
        if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown' && event.key !== 'Home') return;
        event.preventDefault();
        const next = event.key === 'Home'
          ? defaultValue
          : value + direction * (event.key === 'ArrowDown' ? 1 : -1) * (event.shiftKey ? 24 : 8);
        const height = clamp(next, min, max);
        onChange(height);
        onCommit?.(height);
      }}
    >
      <span className="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-[var(--border-default)] transition-[height,background-color] group-hover:h-0.5 group-hover:bg-[var(--accent)] group-focus-visible:h-0.5 group-focus-visible:bg-[var(--accent)]" />
    </div>
  );
}
