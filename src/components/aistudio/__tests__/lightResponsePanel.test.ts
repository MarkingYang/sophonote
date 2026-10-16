import { createElement, act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
const fixture = vi.hoisted(() => ({ call: vi.fn(), start: vi.fn(), load: vi.fn(), upgrade: vi.fn(), fresh: vi.fn() }));
vi.mock('../../../services/lightResponse', () => ({ responseCall: fixture.call, startTextResponse: fixture.start, responseActive: (turn: {state: string}) => ['preparing','running'].includes(turn.state) }));
vi.mock('../../../stores/agentStore', () => ({ useAgentStore: { getState: () => ({ loadThreads: fixture.load }) } }));
vi.mock('../../../stores/appStore', () => ({ useAppStore: (select: (s: unknown) => unknown) => select({ settings: { aiConfig: { providers: { p: { protocol: 'openai', model: 'm', models: ['m2'] } } } } }) }));
vi.mock('../../features/MarkdownView', () => ({ default: ({content}: {content: string}) => createElement('p', null, content) }));
import LightResponsePanel from '../LightResponsePanel';
let root: Root; let container: HTMLDivElement;
const turn = { id: 'r', conversationId: 'response-1', message: 'original', answer: 'answer', state: 'completed', provider: 'p', model: 'm', usage: null };
beforeEach(() => {
  vi.clearAllMocks(); vi.useFakeTimers(); fixture.call.mockResolvedValue([turn]); fixture.load.mockResolvedValue(undefined);
  container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
async function mount() { await act(async () => root.render(createElement(LightResponsePanel, { conversationId: 'response-1', projectId: 'p', onUpgrade: fixture.upgrade, onNew: fixture.fresh }))); }
it('完成历史不进行空闲轮询，保留未知用量与模型选择', async () => {
  await mount(); await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
  expect(fixture.call).toHaveBeenCalledTimes(1); expect(container.textContent).toContain('用量未返回');
  expect(container.querySelectorAll('select option')).toHaveLength(2);
  expect(fixture.start).not.toHaveBeenCalled();
  const details = container.querySelector('details')!;
  expect(details.open).toBe(false);
  expect(details.textContent).toContain('用量未返回');
  await act(async () => details.querySelector('summary')!.click());
  expect(details.open).toBe(true);
});
it('运行中只允许停止，持久终态后停止轮询且迟到状态不继续调度', async () => {
  fixture.call.mockResolvedValueOnce([{ ...turn, answer: null, state: 'running' }]).mockResolvedValue([{ ...turn, state: 'cancelled' }]);
  await mount();
  const upgrade = [...container.querySelectorAll('button')].find(b => b.textContent === '使用资料与工具继续')!;
  expect(upgrade.disabled).toBe(true); expect(container.querySelector('[aria-label="停止"]')).not.toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  const count = fixture.call.mock.calls.length;
  await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
  expect(fixture.call).toHaveBeenCalledTimes(count);
});
it('升级仅取得可编辑摘录，不自动发送 Agent 或第二次模型请求', async () => {
  await mount(); fixture.call.mockResolvedValueOnce('已核实的参考摘录');
  const upgrade = [...container.querySelectorAll('button')].find(b => b.textContent === '使用资料与工具继续')!;
  await act(async () => upgrade.click());
  expect(fixture.call).toHaveBeenLastCalledWith('light_response_handoff', { conversationId: 'response-1', projectId: 'p' });
  expect(fixture.upgrade).toHaveBeenCalledWith('已核实的参考摘录\n'); expect(fixture.start).not.toHaveBeenCalled();
});
async function input(value: string) {
  const area = container.querySelector('textarea')!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(area, value);
    area.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
it('迟到提交回包保留用户新输入，并在历史刷新失败时显示已接纳轮次', async () => {
  await mount(); await input('first submission');
  const pending = deferred<typeof turn>(); fixture.start.mockReturnValueOnce(pending.promise);
  await act(async () => container.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
  expect(fixture.start).toHaveBeenCalledTimes(1);
  await input('new draft while waiting');
  fixture.call.mockRejectedValue(new Error('history unavailable'));
  await act(async () => pending.resolve({ ...turn, id: 'r2', state: 'running', message: 'first submission' }));
  expect(container.querySelector('textarea')!.value).toBe('new draft while waiting');
  expect(container.textContent).toContain('first submission');
  expect(container.querySelector('[aria-label="停止"]')).not.toBeNull();
});
it('未接纳的响应不能清空输入或触发 Agent 回退', async () => {
  await mount(); await input('keep this input'); fixture.start.mockResolvedValueOnce(null);
  await act(async () => container.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
  expect(container.querySelector('textarea')!.value).toBe('keep this input');
  expect(container.textContent).toContain('文本响应未接纳');
  expect(fixture.upgrade).not.toHaveBeenCalled();
});
it('停止后的新历史不会被较早轮询的迟到回包覆盖', async () => {
  fixture.call.mockResolvedValueOnce([{ ...turn, state: 'running' }]); await mount();
  const oldPoll = deferred<(typeof turn)[]>(); fixture.call.mockReturnValueOnce(oldPoll.promise);
  await act(async () => { await vi.advanceTimersByTimeAsync(500); });
  fixture.call.mockResolvedValueOnce(true).mockResolvedValue([{ ...turn, state: 'cancelled' }]);
  const stop = container.querySelector<HTMLButtonElement>('[aria-label="停止"]')!;
  await act(async () => stop.click());
  await act(async () => oldPoll.resolve([{ ...turn, state: 'running' }]));
  expect(container.textContent).not.toContain('正在回答');
  expect(container.querySelector('[aria-label="停止"]')).toBeNull();
});

it('输入法确认和换行不发送，普通回车发送一次', async () => {
  await mount(); await input('继续解释');
  const area = container.querySelector('textarea')!;
  fixture.start.mockReturnValue(new Promise(() => {}));
  for (const options of [{ isComposing: true }, { shiftKey: true }, { keyCode: 229 }]) {
    await act(async () => { area.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true, ...options })); });
  }
  expect(fixture.start).not.toHaveBeenCalled();
  await act(async () => { area.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })); });
  expect(fixture.start).toHaveBeenCalledTimes(1);
});

it('次要操作默认收起，展开后仍只回填摘录', async () => {
  await mount();
  const more = container.querySelector<HTMLDetailsElement>('.sn-response-more')!;
  expect(more.open).toBe(false);
  await act(async () => more.querySelector('summary')!.click());
  expect(more.open).toBe(true);
  expect(fixture.start).not.toHaveBeenCalled();
  expect(fixture.upgrade).not.toHaveBeenCalled();
});
it('只有完整回答提供复制，不把流式或中断内容当作完整结果', async () => {
  fixture.call.mockResolvedValue([
    turn, { ...turn, id: 'cancelled', state: 'cancelled' },
    { ...turn, id: 'running', state: 'running' },
  ]);
  await mount();
  expect(container.querySelectorAll('[aria-label="复制回答"]')).toHaveLength(1);
});
it('历史打开后定位最新，上翻不被新回复抢走，回到底部后恢复跟随', async () => {
  const history = deferred<(typeof turn)[]>();
  fixture.call.mockReturnValueOnce(history.promise);
  await mount();
  const viewport = container.querySelector<HTMLDivElement>('.sn-chat-viewport')!;
  let height = 1000;
  Object.defineProperties(viewport, { scrollHeight: { get: () => height }, clientHeight: { value: 200 } });
  await act(async () => history.resolve([{ ...turn, state: 'running' }]));
  expect(viewport.scrollTop).toBe(1000);
  await act(async () => { viewport.scrollTop = 100; viewport.dispatchEvent(new Event('scroll', { bubbles: true })); });
  height = 1500;
  fixture.call.mockResolvedValue([{ ...turn, state: 'running', answer: 'more text' }]);
  await act(async () => vi.advanceTimersByTimeAsync(500));
  expect(viewport.scrollTop).toBe(100);
  const latest = container.querySelector<HTMLButtonElement>('[aria-label="回到最新消息"]')!;
  await act(async () => latest.click());
  expect(viewport.scrollTop).toBe(1500);
  expect(container.querySelector('[aria-label="回到最新消息"]')).toBeNull();
  height = 2000;
  fixture.call.mockResolvedValue([{ ...turn, state: 'running', answer: 'newest text' }]);
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(viewport.scrollTop).toBe(2000);
});

it('正文延迟增高只在跟随时贴底，卸载停止尺寸监听', async () => {
  let resized!: () => void;
  const disconnect = vi.fn();
  vi.stubGlobal('ResizeObserver', class {
    constructor(callback: () => void) { resized = callback; }
    observe() {}
    disconnect = disconnect;
  });
  await mount();
  const viewport = container.querySelector<HTMLDivElement>('.sn-chat-viewport')!;
  Object.defineProperties(viewport, { scrollHeight: { value: 1000 }, clientHeight: { value: 200 } });
  await act(async () => resized());
  expect(viewport.scrollTop).toBe(1000);
  await act(async () => { viewport.scrollTop = 100; viewport.dispatchEvent(new Event('scroll', { bubbles: true })); resized(); });
  expect(viewport.scrollTop).toBe(100);
  await act(async () => root.render(null));
  expect(disconnect).toHaveBeenCalledTimes(1);
});
