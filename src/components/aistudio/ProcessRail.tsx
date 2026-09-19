import { useEffect, useMemo, useState } from 'react';
import { Check, ChevronDown, Clock, Loader2, X } from 'lucide-react';
import type { AssistantPhase } from '../../services/agentMessagePhase';
import type { ToolCard } from '../../services/agentToolCards';
import { toolDisplayName, toolStepSummary } from '../../services/agentProcessRail';

function duration(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} 分 ${seconds % 60} 秒`;
}

/** 展示事件进展；无更新只提示等待，不推断引擎终态。 */
export default function ProcessRail({ phase, durationMs, processCards, startedAt, lastEventAt }: {
  phase: AssistantPhase;
  durationMs?: number;
  processCards: ToolCard[];
  startedAt?: number;
  lastEventAt?: number;
}) {
  const running = phase === 'thinking' || phase === 'answering';
  const [now, setNow] = useState(Date.now);
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  useEffect(() => {
    setUserOpen(null);
    if (!running) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);

  const steps = useMemo(() => {
    const grouped = new Map<string, { label: string; count: number; status: ToolCard['status']; names: Set<string> }>();
    for (const card of processCards) {
      const summary = toolStepSummary(card);
      const label = `${toolDisplayName(card.name)}${summary ? ` · ${summary}` : ''}`;
      const existing = grouped.get(label);
      if (!existing) grouped.set(label, { label, count: 1, status: card.status, names: new Set([card.name]) });
      else {
        existing.count++;
        existing.names.add(card.name);
        if (card.status === 'running' || (card.status === 'failed' && existing.status !== 'running')) existing.status = card.status;
      }
    }
    return [...grouped.values()];
  }, [processCards]);
  const currentStep = running ? [...processCards].reverse().find((card) => card.status === 'running') : undefined;
  const lastProgress = Math.max(lastEventAt ?? 0, startedAt ?? 0,
    ...processCards.map((card) => card.completedAt ?? card.startedAt));
  const idleMs = lastProgress > 0 ? Math.max(0, now - lastProgress) : 0;
  const waiting = running && idleMs >= 60_000;
  const elapsedMs = durationMs ?? (running && startedAt != null ? Math.max(0, now - startedAt) : undefined);
  const label = phase === 'error' ? '执行未完成' : phase === 'done' ? '已完成'
    : waiting ? '暂未收到新进展'
    : currentStep ? `正在${toolDisplayName(currentStep.name)}`
    : phase === 'answering' ? '正在生成回复'
    : processCards.length > 0 ? '等待模型回复' : '正在理解任务';
  const open = steps.length > 0 && (userOpen ?? phase === 'thinking');

  return <section className="hb-chat-process" role="region" aria-label="执行进度" data-waiting={waiting || undefined}>
    <button type="button" className="hb-chat-process-summary" disabled={steps.length === 0}
      onClick={() => setUserOpen(!open)} aria-expanded={steps.length > 0 ? open : undefined}>
      {waiting ? <Clock size={13} className="shrink-0" />
        : running ? <Loader2 size={13} className="shrink-0 animate-spin" />
        : phase === 'error' ? <X size={13} className="shrink-0 text-[var(--danger)]" />
        : <Check size={13} className="shrink-0 text-[var(--success)]" />}
      <span className="hb-chat-process-label">{label}</span>
      <span className="hb-chat-process-metrics">
        {processCards.length > 0 && <span>{processCards.length} 次操作</span>}
        {elapsedMs != null && <span>{duration(elapsedMs)}</span>}
      </span>
      {steps.length > 0 && <ChevronDown size={12} className={`shrink-0 transition-transform ${open ? '' : '-rotate-90'}`} />}
    </button>
    {waiting && <p className="hb-chat-process-wait" role="status">
      已 {duration(idleMs)} 未收到更新。可查看待确认事项；持续无响应时可停止任务后重试。
    </p>}
    {open && <ol className="hb-chat-process-steps">{steps.map((step) => <li key={step.label}
      className="flex min-w-0 items-start gap-2" title={[...step.names].join('、')}>
      {step.status === 'running' && running ? <Loader2 size={11} className="mt-0.5 shrink-0 animate-spin" />
        : step.status === 'failed' ? <X size={11} className="mt-0.5 shrink-0 text-[var(--danger)]" />
        : step.status === 'running' ? <Clock size={11} className="mt-0.5 shrink-0" />
        : <Check size={11} className="mt-0.5 shrink-0 text-[var(--success)]" />}
      <span className="min-w-0 flex-1 break-words">{step.label}</span>
      {step.count > 1 && <span className="shrink-0 tabular-nums text-[var(--text-tertiary)]">{step.count} 次</span>}
    </li>)}</ol>}
  </section>;
}
