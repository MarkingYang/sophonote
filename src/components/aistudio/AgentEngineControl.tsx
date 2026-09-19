import { useEffect, useId, useRef, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import type { AgentEngine } from '../../stores/agentStore';

const engines = [{ id: 'hermes', label: 'Hermes' }, { id: 'pi', label: 'Pi' }, { id: 'claude_code', label: 'Claude Code' }, { id: 'opencode', label: 'OpenCode' }] as const;

export function AgentEngineControl({ value, disabled, onSelect }: {
  value: AgentEngine;
  disabled?: boolean;
  onSelect: (engine: AgentEngine) => Promise<boolean>;
}) {
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menuId = useId();
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);

  return <div ref={root} className="relative" onKeyDown={(event) => {
    if (event.key === 'Escape') { event.stopPropagation(); setOpen(false); trigger.current?.focus(); }
  }}>
    <button ref={trigger} type="button" aria-label="智能体引擎" aria-expanded={open}
      aria-controls={open ? menuId : undefined} disabled={disabled || pending}
      onClick={() => setOpen((current) => !current)}
      className="inline-flex h-7 items-center gap-1 rounded-lg bg-[var(--bg-sunken)] px-1.5 text-xs text-[var(--text-secondary)] hover:text-[var(--text-primary)] disabled:opacity-50">
      {engines.find((engine) => engine.id === value)?.label ?? 'Hermes'}<ChevronDown size={12} />
    </button>
    {open && <div id={menuId} aria-label="选择智能体引擎"
      className="absolute bottom-[calc(100%+8px)] left-0 z-40 w-max whitespace-nowrap rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-1 shadow-[var(--shadow-lg)]">
      {engines.map((engine) => <button key={engine.id} type="button"
        aria-pressed={value === engine.id}
        disabled={pending || disabled || value === engine.id}
        onClick={async () => {
          setPending(true);
          try { if (await onSelect(engine.id)) setOpen(false); }
          finally { setPending(false); }
        }}
        className="flex w-full items-center gap-1.5 rounded-lg px-1.5 py-1.5 text-left text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:cursor-default">
        <span>{engine.label}</span>
        <Check size={12} aria-hidden="true" className={`shrink-0 text-[var(--text-tertiary)] ${value === engine.id ? '' : 'invisible'}`} />
      </button>)}
    </div>}
  </div>;
}
