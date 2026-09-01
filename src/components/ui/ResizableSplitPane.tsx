import { useEffect, useRef, useState, type ReactNode } from 'react';

interface ResizableSplitPaneProps {
  direction: 'horizontal' | 'vertical';
  first: ReactNode;
  second: ReactNode;
  initialRatio?: number;
  minFirst?: number;
  minSecond?: number;
  label: string;
  className?: string;
  enabled?: boolean;
}

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

/** Binary workbench split: horizontal = side by side; vertical = stacked. */
export default function ResizableSplitPane({
  direction,
  first,
  second,
  initialRatio = 0.5,
  minFirst = 160,
  minSecond = 160,
  label,
  className = '',
  enabled = true,
}: ResizableSplitPaneProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const [ratio, setRatio] = useState(() => clamp(initialRatio, 0.1, 0.9));

  useEffect(() => () => cleanupRef.current?.(), []);

  const moveTo = (position: number) => {
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    const size = direction === 'horizontal' ? rect.width : rect.height;
    if (size <= 0) return;
    const start = direction === 'horizontal' ? rect.left : rect.top;
    const minRatio = Math.min(0.45, minFirst / size);
    const maxRatio = Math.max(0.55, 1 - minSecond / size);
    setRatio(clamp((position - start) / size, minRatio, maxRatio));
  };

  const separatorClass = direction === 'horizontal'
    ? 'w-1.5 cursor-col-resize'
    : 'h-1.5 cursor-row-resize';
  const lineClass = direction === 'horizontal'
    ? 'inset-y-0 left-1/2 w-px -translate-x-1/2'
    : 'inset-x-0 top-1/2 h-px -translate-y-1/2';

  return (
    <div ref={hostRef} className={`${direction === 'horizontal' ? 'flex-row' : 'flex-col'} flex h-full min-h-0 w-full min-w-0 overflow-hidden ${className}`}>
      <div className="min-h-0 min-w-0 overflow-hidden" style={{ flexBasis: enabled ? `${ratio * 100}%` : '100%', flexGrow: 0, flexShrink: 0 }}>{first}</div>
      <div
        role="separator"
        aria-label={label}
        aria-orientation={direction === 'horizontal' ? 'vertical' : 'horizontal'}
        aria-valuenow={Math.round(ratio * 100)}
        tabIndex={0}
        className={`${enabled ? '' : 'hidden'} group relative z-20 shrink-0 touch-none bg-transparent focus-visible:outline-none ${separatorClass}`}
        title="拖拽调整大小；双击均分"
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          const cursor = direction === 'horizontal' ? 'col-resize' : 'row-resize';
          document.body.style.cursor = cursor;
          document.body.style.userSelect = 'none';
          const onMove = (moveEvent: PointerEvent) => moveTo(direction === 'horizontal' ? moveEvent.clientX : moveEvent.clientY);
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
        onDoubleClick={() => setRatio(0.5)}
        onKeyDown={(event) => {
          const delta = (event.shiftKey ? 0.08 : 0.025) * (event.key === 'ArrowRight' || event.key === 'ArrowDown' ? 1 : -1);
          const accepted = direction === 'horizontal'
            ? event.key === 'ArrowLeft' || event.key === 'ArrowRight'
            : event.key === 'ArrowUp' || event.key === 'ArrowDown';
          if (accepted) {
            event.preventDefault();
            setRatio((current) => clamp(current + delta, 0.1, 0.9));
          } else if (event.key === 'Home') {
            event.preventDefault();
            setRatio(0.5);
          }
        }}
      >
        <span className={`absolute bg-[var(--border-default)] transition-[width,height,background-color] group-hover:bg-[var(--accent)] group-focus-visible:bg-[var(--accent)] ${lineClass}`} />
      </div>
      <div className={`${enabled ? '' : 'hidden'} min-h-0 min-w-0 flex-1 overflow-hidden`}>{second}</div>
    </div>
  );
}
