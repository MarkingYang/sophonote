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
  it('运行时直接显示真实进展与归并次数，工具结束后继续整理结果', async () => {
    await act(async () => root.render(createElement(ProcessRail, {
      phase: 'thinking', startedAt, processCards: [card('1'), card('2'), card('3', 'skill_view')],
    })));
    expect(host.textContent).not.toContain('3 次操作');
        expect(host.querySelectorAll('li')).toHaveLength(2);
    expect(host.textContent).toContain('2 次');
    expect(host.querySelectorAll('li')[0].textContent).toContain('已完成');
    expect(host.textContent).toContain('正在整理结果');
    expect(host.textContent).toContain('查看技能');
    expect(host.textContent).not.toContain('skill_view');
  });

  it('60 秒未更新提示等待；新推理事件恢复计时而不伪造终态', async () => {
    const props = { phase: 'thinking' as const, startedAt, processCards: [card('1')], lastEventAt: startedAt + 2000 };
    await act(async () => root.render(createElement(ProcessRail, props)));
    await act(async () => vi.advanceTimersByTime(60_000));
    expect(host.textContent).toContain('暂未收到新进展');
    expect(host.textContent).toContain('已 1 分 0 秒 未收到更新');
    expect(host.querySelector('.hb-chat-process-label')!.textContent).not.toContain('已完成');
    await act(async () => root.render(createElement(ProcessRail, { ...props, lastEventAt: Date.now() })));
    expect(host.textContent).toContain('正在整理结果');
    expect(host.querySelector('.hb-chat-process-wait')).toBeNull();
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


it('结果操作与进度并列，展开或关闭过程不会隐藏操作', async () => {
  const onCopy = vi.fn();
  await act(async () => root.render(createElement(ProcessRail, {
    phase: 'done', durationMs: 8000, processCards: [card('1')],
    actions: createElement('button', { onClick: onCopy }, '复制回答'),
  })));
  const row = host.querySelector('.sn-answer-actions')!;
  expect(row.textContent).toContain('8 秒');
  const buttons = row.querySelectorAll('button');
  await act(async () => buttons[1].click());
  expect(onCopy).toHaveBeenCalledTimes(1);
  expect(host.querySelector('ol')).toBeNull();
  await act(async () => buttons[0].click());
  expect(host.querySelector('ol')).not.toBeNull();
  expect(host.querySelector('ol')!.closest('.sn-answer-actions')).toBeNull();
  expect(row.textContent).toContain('复制回答');
});


it('运行时只预览三项并保留在途操作，展开全部后可回到最近进展', async () => {
  const cards = ['read_file', 'web_search', 'webfetch', 'terminal', 'skill_view'].map((name, i) => ({
    ...card(String(i), name), completedAt: startedAt + 1000 + i,
  }));
  cards[0].status = 'running';
  cards[2].status = 'failed';
  await act(async () => root.render(createElement(ProcessRail, { phase: 'thinking', startedAt, processCards: cards })));
  expect(host.querySelectorAll('li')).toHaveLength(3);
  expect(host.querySelector('ol')!.textContent).toContain('读取文件');
  expect(host.querySelector('ol')!.textContent).not.toContain('搜索网页');
  expect(host.textContent).toContain('进行中');
  expect(host.textContent).not.toContain('%');
  await act(async () => host.querySelector('button')!.click());
  expect(host.querySelectorAll('li')).toHaveLength(5);
  expect(host.textContent).toContain('共 5 次操作');
  expect(host.textContent).toContain('失败');
  await act(async () => host.querySelector('button')!.click());
  expect(host.querySelectorAll('li')).toHaveLength(3);
});

it('无工具事件时只显示当前处理状态，不捏造步骤，计时不进入状态播报', async () => {
  await act(async () => root.render(createElement(ProcessRail, { phase: 'thinking', startedAt, processCards: [] })));
  expect(host.textContent).toContain('正在理解任务');
  expect(host.querySelector('ol')).toBeNull();
  await act(async () => vi.advanceTimersByTime(2000));
  expect(host.textContent).toContain('4 秒');
  expect(host.querySelector('.hb-chat-process-label')!.textContent).toBe('正在理解任务');
});
