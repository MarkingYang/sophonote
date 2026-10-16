import { useEffect, useId, useRef, useState } from 'react';
import { Check, Clock, Loader2, X, Minus, ChevronDown } from 'lucide-react';
import { readWorkflow, workflowStateLabel } from '../../services/agentWorkflow';
import { useAgentStore } from '../../stores/agentStore';
import { usePageSurfaceActive, useSurfaceAgentStore } from '../layout/KeptAlivePage';

/** Only an active, visible workflow refreshes. Never merge child event sequences. */
function TaskStatusIcon({ state, active }: { state: string; active: boolean }) {
  if (state === 'succeeded') return <Check size={13} />;
  if (active && (state === 'running' || state === 'dispatching')) return <Loader2 size={13} className="sn-task-spinner" />;
  if (state === 'failed' || state === 'partial') return <X size={13} />;
  if (state === 'cancelled' || state === 'interrupted') return <Minus size={13} />;
  return <Clock size={13} />;
}

export function WorkflowProgress({ runId, threadId }: { runId: string; threadId: string }) {
  const surfaceActive = usePageSurfaceActive();
  const detailId = useId();
  const taskListId = useId();
  const triggers = useRef(new Map<string, HTMLButtonElement>());
  const [expandedRun, setExpandedRun] = useState<string | null>(null);
  const [selection, setSelection] = useState<{ runId: string; executionId: string } | null>(null);
  const snapshot = useSurfaceAgentStore((s) => s.workflowsByRun[runId]);
  const [error, setError] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!surfaceActive) return;
    let active = true;
    let pending = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      if (!active || pending || document.hidden) return;
      pending = true;
      clearTimeout(timer);
      try {
        const value = await readWorkflow(runId);
        if (!active) return;
        if (value && value.threadId !== threadId) throw new Error('wrong owner');
        setError(false);
        useAgentStore.getState().setWorkflow(runId, value);
        if (value?.locked && value.state !== 'recovering') timer = setTimeout(() => void refresh(), 1000);
      } catch { if (active) setError(true); }
      finally { pending = false; }
    };
    const visible = () => {
      clearTimeout(timer);
      if (!document.hidden) void refresh();
    };
    setError(false);
    void refresh();
    document.addEventListener('visibilitychange', visible);
    return () => { active = false; clearTimeout(timer); document.removeEventListener('visibilitychange', visible); };
  }, [runId, threadId, retry, surfaceActive]);
  const children = snapshot?.children ?? [];
  const selected = selection?.runId === runId ? children.find(child => child.executionId === selection.executionId) : undefined;
  const completed = children.filter(child => child.state === 'succeeded').length;
  const canCollapse = !error && snapshot?.state === 'succeeded' && !snapshot.locked && children.length > 0
    && children.every(child => child.state === 'succeeded' && child.unresolved.length === 0);
  const expanded = !canCollapse || expandedRun === runId || !!selected;
  const closeResult = () => {
    const trigger = selected && triggers.current.get(selected.executionId);
    // Keep the task list visible so its trigger can receive focus after closing.
    setExpandedRun(runId);
    setSelection(null);
    trigger?.focus();
  };
  if (!snapshot && !error) return null;
  return <section aria-label="任务进度" className="text-xs leading-relaxed text-[var(--text-tertiary)]">
    {error && <p role="alert">暂时无法核对任务状态。<button className="sn-inline-action" type="button" onClick={() => setRetry(v => v + 1)}>重新查询</button></p>}
    {snapshot?.state === 'recovering' && <p role="status">执行状态尚未核实，请勿重复发送。<button className="sn-inline-action" type="button" onClick={() => setRetry(v => v + 1)}>查询状态</button></p>}
    {snapshot?.state === 'cancelled' && <p role="status">已停止，未完成的子任务不能视为已核实的成果。</p>}
    {snapshot?.state === 'failed' && <p role="status">任务未能完成，请核对结果后再使用。</p>}
    {snapshot?.state === 'partial' && <p role="status">部分工作未完成，请留意结果中的缺口。</p>}
    {snapshot?.state === 'interrupted' && <p role="status">任务已中断，未完成的工作需要重新核实。</p>}
    {children.length > 0 && <div className="sn-collaboration">
      {canCollapse ? <button type="button" className="sn-collaboration-toggle" aria-expanded={expanded} aria-controls={taskListId}
        onClick={() => { setExpandedRun(expanded ? null : runId); setSelection(null); }}>
        <Check size={13} className="text-[var(--success)]" />
        <span>已完成 {children.length} 项协作</span>
        <ChevronDown size={12} className={expanded ? '' : '-rotate-90'} />
      </button> : <div className="sn-collaboration-heading">
        <span>协作任务</span>
        <span>{completed}/{children.length} 已完成</span>
        {snapshot?.locked && <span>{workflowStateLabel(snapshot.state)}</span>}
      </div>}
      <div id={taskListId} hidden={!expanded}>
      <ul className="sn-collaboration-tasks" aria-label="子任务">
        {children.map(child => <li key={child.executionId}>
          <button type="button" className="sn-collaboration-task" data-state={child.state}
            ref={node => { if (node) triggers.current.set(child.executionId, node); else triggers.current.delete(child.executionId); }}
            aria-expanded={selected?.executionId === child.executionId} aria-controls={detailId}
            title={child.objective || '子任务'}
            onKeyDown={event => { if (event.key === 'Escape' && selected?.executionId === child.executionId) { event.preventDefault(); event.stopPropagation(); closeResult(); } }}
            onClick={() => setSelection(selected?.executionId === child.executionId ? null : { runId, executionId: child.executionId })}>
            <TaskStatusIcon state={child.state} active={!!snapshot?.locked && snapshot.state !== 'recovering'} />
            <span className="sn-collaboration-task-name">{child.objective || '子任务'}</span>
            <span className="sn-collaboration-task-state">{workflowStateLabel(child.state)}</span>
          </button>
        </li>)}
      </ul>
      <div id={detailId} hidden={!selected} className="sn-collaboration-result" role="region" aria-label="子任务结果" tabIndex={-1}
        onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); closeResult(); } }}>
        {selected && <>
          <div className="sn-collaboration-result-heading">
            <h3>{selected.objective || '子任务'}</h3>
            <button type="button" className="sn-inline-action" aria-label="收起子任务结果" onClick={closeResult}>收起</button>
          </div>
          <p className="sn-collaboration-result-state">{workflowStateLabel(selected.state)}</p>
          {selected.summary ? <p className="sn-collaboration-result-text">{selected.summary}</p>
            : <p>{selected.state === 'succeeded' ? '此子任务未提供结果摘要。' : '尚无可用结果。'}</p>}
          {selected.unresolved.length > 0 && <div className="sn-collaboration-unresolved">
            <p>待核实</p>
            <ul>{selected.unresolved.map((item, index) => <li key={index}>{item}</li>)}</ul>
          </div>}
        </>}
      </div>
      </div>
    </div>}
  </section>;
}
