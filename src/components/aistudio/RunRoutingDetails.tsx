import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AgentEngine, RunSnapshot, RoutingSummary } from '../../stores/agentStore';

/** Read immutable Host evidence on demand; never infer a historical route in UI. */
export function RunRoutingDetails({ runId, engine, engineLabel }: {
  runId: string; engine: AgentEngine; engineLabel: string;
}) {
  const [open, setOpen] = useState(false);
  const [record, setRecord] = useState<{ runId: string; routing: RoutingSummary | null; error?: boolean } | null>(null);
  useEffect(() => {
    if (!open) return;
    let active = true;
    setRecord(null);
    void invoke<{ success: boolean; data: RunSnapshot | null }>('agent_run_snapshot', { runId })
      .then((result) => {
        if (!active) return;
        const valid = result.success && result.data?.runId === runId;
        const routing = valid ? result.data?.routing ?? null : null;
        setRecord({ runId, routing: routing?.engine === engine ? routing : null, error: !valid });
      }).catch(() => { if (active) setRecord({ runId, routing: null, error: true }); });
    return () => { active = false; };
  }, [open, runId, engine]);
  const current = record?.runId === runId ? record : null;
  return <details className="sn-task-details" onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary>执行详情</summary>
    {open && <div className="mt-1 space-y-1">
      <p>实际执行：{engineLabel}</p>
      {!current ? <p>正在读取选择依据…</p>
        : current.error ? <p>暂时无法读取选择依据，重新展开可重试</p>
          : current.routing ? <>
            {current.routing.taskLabel && <p>任务线索：{current.routing.taskLabel}</p>}
            <p>{current.routing.reason}</p>
            <p>策略：{current.routing.policyVersion}</p>
          </> : <p>此历史运行未记录选择依据</p>}
    </div>}
  </details>;
}
