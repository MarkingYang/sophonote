// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), Channel: class { onmessage = null; } }));
import { invoke } from '@tauri-apps/api/core';
import { WorkflowProgress } from '../WorkflowProgress';
import KeptAlivePage from '../../layout/KeptAlivePage';
import { useAgentStore } from '../../../stores/agentStore';
import type { WorkflowSnapshot } from '../../../services/agentWorkflow';

const call = vi.mocked(invoke);
let root: Root;
let element: HTMLDivElement;
const snapshot = (runId = 'r', state: WorkflowSnapshot['state'] = 'running'): WorkflowSnapshot => ({
  runId, threadId: 't', workflowId: 'w', state, locked: ['running', 'cancelling', 'recovering'].includes(state), sequence: 1,
  children: [{ executionId: 'c', role: 'reviewer', objective: '核对资料', state: 'running', summary: null, unresolved: [], modelCalls: 1 }],
});
async function render(runId = 'r', active = true) {
  await act(async () => root.render(createElement(KeptAlivePage, { pageId: 'test', active, children: createElement(WorkflowProgress, { runId, threadId: 't' }) })));
}
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  call.mockReset(); useAgentStore.setState({ workflowsByRun: {} });
  element = document.createElement('div'); document.body.append(element); root = createRoot(element);
  vi.useFakeTimers();
});
afterEach(async () => { await act(async () => root.unmount()); element.remove(); vi.useRealTimers(); });

it('主回复结束后仍按工作流锁刷新，整体终态停止刷新', async () => {
  call.mockResolvedValueOnce({ success: true, data: snapshot() })
    .mockResolvedValue({ success: true, data: { ...snapshot('r', 'partial'), sequence: 2 } });
  await render(); expect(useAgentStore.getState().workflowsByRun.r.locked).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(element.querySelector('[aria-label="子任务结果"]')?.hasAttribute('hidden')).toBe(true);
  const warning = element.querySelector('[role="status"]')!;
  expect(warning.textContent).toContain('部分工作未完成');
  expect(warning.closest('details')).toBeNull();
  expect(useAgentStore.getState().workflowsByRun.r.locked).toBe(false);
  await act(async () => vi.advanceTimersByTimeAsync(5000)); expect(call).toHaveBeenCalledTimes(2);
});
it('保活隐藏页不轮询，显示后重新读取', async () => {
  call.mockResolvedValue({ success: true, data: snapshot() });
  await render(); await render('r', false);
  await act(async () => vi.advanceTimersByTimeAsync(5000)); expect(call).toHaveBeenCalledTimes(1);
  await render(); expect(call).toHaveBeenCalledTimes(2);
});
it('切换运行后丢弃迟到结果，未知恢复状态等待显式查询', async () => {
  let old!: (value: unknown) => void;
  call.mockImplementation(async (_cmd, args) => args?.runId === 'old' ? new Promise((resolve) => { old = resolve; }) : { success: true, data: snapshot('new', 'recovering') });
  await render('old'); await render('new');
  await act(async () => old({ success: true, data: snapshot('old', 'succeeded') }));
  expect(useAgentStore.getState().workflowsByRun.old).toBeUndefined();
  expect(element.textContent).toContain('等待核实');
  await act(async () => vi.advanceTimersByTimeAsync(5000)); expect(call).toHaveBeenCalledTimes(2);
});
it('查询失败不解锁已知工作流，不回显原生错误', async () => {
  useAgentStore.setState({ workflowsByRun: { r: snapshot() } });
  call.mockRejectedValue(new Error('PRIVATE DETAILS'));
  await render(); expect(element.textContent).toContain('暂时无法核对');
  expect(element.textContent).not.toContain('PRIVATE'); expect(useAgentStore.getState().workflowsByRun.r.locked).toBe(true);
  expect(await useAgentStore.getState().startRun('t', 'new')).toBeNull();
  expect(call).toHaveBeenCalledTimes(1);
});

it('整组停止后不把主回复当作完整协作成果', async () => {
  call.mockResolvedValue({ success: true, data: snapshot('r', 'cancelled') });
  await render(); expect(element.textContent).toContain('未完成的子任务不能视为已核实的成果');
  expect(useAgentStore.getState().workflowsByRun.r.locked).toBe(false);
  await act(async () => vi.advanceTimersByTimeAsync(5000)); expect(call).toHaveBeenCalledTimes(1);
});

it('目录按整体协作结果显示状态，旧轮结果不覆盖新轮', async () => {
  const thread = { id: 't', latestRunId: 'r', status: 'completed' as const, title: '任务', createdAt: 0, updatedAt: 0 };
  useAgentStore.setState({ threads: [thread], historyThreads: [] });
  call.mockResolvedValue({ success: true, data: snapshot('r', 'cancelled') });
  await render(); expect(useAgentStore.getState().threads[0].status).toBe('cancelled');
  await act(async () => {
    useAgentStore.setState({ threads: [{ ...thread, latestRunId: 'new' }] });
    useAgentStore.getState().setWorkflow('r', snapshot('r', 'partial'));
  });
  expect(useAgentStore.getState().threads[0].status).toBe('completed');
});


it('按真实目标显示子任务，点击查看摘要；隐藏调用次数和内部身份', async () => {
  const value = snapshot('r', 'succeeded');
  value.children = [
    { ...value.children[0], objective: '核对产品定价与来源', state: 'succeeded', summary: '已核对两份公开资料。', modelCalls: 7 },
    { ...value.children[0], executionId: 'other', objective: '整理功能差异', state: 'failed', unresolved: ['尚缺企业版价格'] },
  ];
  call.mockResolvedValue({ success: true, data: value });
  await render();
  expect(element.textContent).toContain('1/2 已完成');
  expect(element.textContent).toContain('核对产品定价与来源');
  expect(element.textContent).not.toContain('模型调用');
  expect(element.textContent).not.toContain('executionId');
  expect(element.textContent).not.toContain('执行详情');
  const buttons = element.querySelectorAll<HTMLButtonElement>('.sn-collaboration-task');
  await act(async () => buttons[0].click());
  expect(buttons[0].getAttribute('aria-expanded')).toBe('true');
  expect(element.textContent).toContain('已核对两份公开资料');
  await act(async () => buttons[1].click());
  expect(buttons[0].getAttribute('aria-expanded')).toBe('false');
  expect(element.textContent).toContain('尚缺企业版价格');
  expect(element.textContent).not.toContain('已核对两份公开资料');
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="收起子任务结果"]')!.click());
  expect(element.querySelector('[aria-label="子任务结果"]')!.hasAttribute('hidden')).toBe(true);
});

it('无子任务不展示协作空壳，但恢复状态提示不可隐藏', async () => {
  call.mockResolvedValueOnce({ success: true, data: null });
  await render(); expect(element.textContent).toBe('');
  call.mockResolvedValueOnce({ success: true, data: { ...snapshot('new', 'recovering'), children: [] } });
  await render('new');
  expect(element.querySelector('[aria-label="子任务"]')).toBeNull();
  expect(element.textContent).toContain('请勿重复发送');
});

it('切换运行不展示上轮选中的子结果，停止后不再转圈', async () => {
  call.mockResolvedValueOnce({ success: true, data: { ...snapshot(), children: [{ ...snapshot().children[0], summary: '旧轮结果' }] } });
  await render();
  await act(async () => element.querySelector<HTMLButtonElement>('.sn-collaboration-task')!.click());
  expect(element.textContent).toContain('旧轮结果');
  call.mockResolvedValueOnce({ success: true, data: snapshot('new', 'cancelled') });
  await render('new');
  expect(element.textContent).not.toContain('旧轮结果');
  expect(element.querySelector('.sn-task-spinner')).toBeNull();
  expect(element.textContent).toContain('0/1 已完成');
});


it('完整协作默认收为一行，展开后 Escape 收起结果并保留键盘焦点', async () => {
  const done = { ...snapshot('r', 'succeeded'), children: [{ ...snapshot().children[0], state: 'succeeded', summary: '核对完成' }] };
  call.mockResolvedValue({ success: true, data: done });
  await render();
  const toggle = element.querySelector<HTMLButtonElement>('.sn-collaboration-toggle')!;
  expect(toggle.textContent).toContain('已完成 1 项协作');
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
  expect(element.querySelector('[aria-label="子任务"]')!.parentElement!.hidden).toBe(true);
  await act(async () => toggle.click());
  const task = element.querySelector<HTMLButtonElement>('.sn-collaboration-task')!;
  await act(async () => task.click());
  const close = element.querySelector<HTMLButtonElement>('[aria-label="收起子任务结果"]')!;
  close.focus();
  await act(async () => close.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
  expect(document.activeElement).toBe(task);
  expect(task.getAttribute('aria-expanded')).toBe('false');
  expect(toggle.getAttribute('aria-expanded')).toBe('true');
  await act(async () => toggle.click());
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
});

it('完成事件不收起用户正在阅读的子任务，手动收起仍回到任务标签', async () => {
  call.mockResolvedValueOnce({ success: true, data: snapshot() });
  await render();
  const task = element.querySelector<HTMLButtonElement>('.sn-collaboration-task')!;
  await act(async () => task.click());
  call.mockResolvedValue({ success: true, data: { ...snapshot('r', 'succeeded'), sequence: 2,
    children: [{ ...snapshot().children[0], state: 'succeeded', summary: '最新核对结果' }] } });
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(element.querySelector('[aria-label="子任务结果"]')!.hasAttribute('hidden')).toBe(false);
  expect(element.textContent).toContain('最新核对结果');
  await act(async () => element.querySelector<HTMLButtonElement>('[aria-label="收起子任务结果"]')!.click());
  expect(document.activeElement).toBe(task);
  expect(element.querySelector('[aria-label="子任务"]')!.parentElement!.hidden).toBe(false);
});

it('整体成功但仍有待核实项时保持任务列表可见', async () => {
  call.mockResolvedValue({ success: true, data: { ...snapshot('r', 'succeeded'),
    children: [{ ...snapshot().children[0], state: 'succeeded', unresolved: ['来源时间待确认'] }] } });
  await render();
  expect(element.querySelector('.sn-collaboration-toggle')).toBeNull();
  expect(element.querySelector('[aria-label="子任务"]')!.parentElement!.hidden).toBe(false);
});
