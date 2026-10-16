import { invoke } from '@tauri-apps/api/core';

export type WorkflowState = 'running' | 'cancelling' | 'recovering' | 'succeeded' | 'partial' | 'failed' | 'cancelled' | 'interrupted';
export interface WorkflowSnapshot {
  runId: string;
  threadId: string;
  workflowId: string;
  state: WorkflowState;
  locked: boolean;
  sequence: number;
  children: { executionId: string; role: string; objective: string; state: string; summary: string | null; unresolved: string[]; modelCalls: number | null }[];
}
export async function readWorkflow(runId: string): Promise<WorkflowSnapshot | null> {
  const result = await invoke<{ success: boolean; data: WorkflowSnapshot | null; error?: string }>('agent_workflow_snapshot', { runId });
  if (!result.success || (result.data && result.data.runId !== runId)) throw new Error('暂时无法核对协作状态，请查询后再继续');
  return result.data;
}
export function workflowStateLabel(state: string): string {
  return ({ queued: '等待执行', dispatching: '正在启动', running: '进行中', cancelling: '正在确认停止', recovering: '等待核实', succeeded: '已完成', partial: '部分完成', failed: '未完成', cancelled: '已停止', interrupted: '已中断' } as Record<string, string>)[state] ?? '状态待核实';
}
