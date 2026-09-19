import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import ProcessRail from '../ProcessRail';
import type { ToolCard } from '../../../services/agentToolCards';

let host: HTMLDivElement;
let root: Root;
const startedAt = 1_000_000;
const card = (callId: string, name = 'terminal'): ToolCard => ({
  callId, name, runId: 'r', threadId: 't', status: 'completed', startedAt, completedAt: startedAt + 1000,
});
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(startedAt + 2000);
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.useRealTimers();
});

describe('执行进度事实与等待提示', () => {
  it('归并操作仍显示真实调用数，工具结束后等待模型', async () => {
    await act(async () => root.render(createElement(ProcessRail, {
      phase: 'thinking', startedAt, processCards: [card('1'), card('2'), card('3', 'skill_view')],
    })));
    expect(host.textContent).toContain('3 次操作');
    expect(host.querySelectorAll('li')).toHaveLength(2);
    expect(host.textContent).toContain('等待模型回复');
    expect(host.textContent).toContain('查看技能');
    expect(host.textContent).not.toContain('skill_view');
  });

  it('60 秒未更新提示等待；新推理事件恢复计时而不伪造终态', async () => {
    const props = { phase: 'thinking' as const, startedAt, processCards: [card('1')], lastEventAt: startedAt + 2000 };
    await act(async () => root.render(createElement(ProcessRail, props)));
    await act(async () => vi.advanceTimersByTime(60_000));
    expect(host.textContent).toContain('暂未收到新进展');
    expect(host.textContent).toContain('已 1 分 0 秒 未收到更新');
    expect(host.textContent).not.toContain('已完成');
    await act(async () => root.render(createElement(ProcessRail, { ...props, lastEventAt: Date.now() })));
    expect(host.textContent).toContain('等待模型回复');
    expect(host.querySelector('[role="status"]')).toBeNull();
  });

  it('终态停止悬空工具转圈，默认收起且可再次展开', async () => {
    const props = { startedAt, processCards: [{ ...card('1'), status: 'running' as const }] };
    await act(async () => root.render(createElement(ProcessRail, { ...props, phase: 'thinking' })));
    await act(async () => root.render(createElement(ProcessRail, { ...props, phase: 'done', durationMs: 8000 })));
    expect(host.querySelector('ol')).toBeNull();
    await act(async () => host.querySelector('button')!.click());
    expect(host.querySelector('ol')).not.toBeNull();
    expect(host.querySelector('.animate-spin')).toBeNull();
    await act(async () => vi.advanceTimersByTime(120_000));
    expect(host.textContent).toContain('8 秒');
    expect(host.textContent).not.toContain('暂未收到新进展');
  });
});
