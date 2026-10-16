import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
vi.mock('../../../services/clipboard', () => ({ copyMarkdownSource: vi.fn() }));
import { copyMarkdownSource } from '../../../services/clipboard';
import AnswerCopyButton from '../AnswerCopyButton';

const copy = vi.mocked(copyMarkdownSource);
let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  vi.useFakeTimers(); copy.mockReset();
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.useRealTimers(); });
const render = async (content: string) => { await act(async () => root.render(createElement(AnswerCopyButton, { content }))); };
it('复制完整 Markdown，成功后短暂反馈，不复制状态或路由信息', async () => {
  const content = '# 结论\n\n- **证据**\n\n```ts\nconst ok = true;\n```';
  copy.mockResolvedValue(true); await render(content);
  await act(async () => host.querySelector('button')!.click());
  expect(copy).toHaveBeenCalledWith(content);
  expect(host.textContent).toContain('已复制');
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(host.textContent).not.toContain('已复制');
});
it('失败可重试，不显示虚假的成功反馈', async () => {
  copy.mockResolvedValueOnce(false).mockResolvedValueOnce(true); await render('answer');
  await act(async () => host.querySelector('button')!.click());
  expect(host.querySelector('[role="status"]')!.textContent).toContain('复制失败');
  await act(async () => host.querySelector('button')!.click());
  expect(host.textContent).toContain('已复制');
  expect(host.textContent).not.toContain('复制失败');
});
it('复制期间不重复提交，旧回答的迟到结果不污染新回答', async () => {
  let resolve!: (value: boolean) => void;
  copy.mockReturnValue(new Promise(done => { resolve = done; })); await render('old');
  await act(async () => { host.querySelector('button')!.click(); host.querySelector('button')!.click(); });
  expect(copy).toHaveBeenCalledTimes(1);
  await render('new');
  await act(async () => resolve(true));
  expect(host.textContent).not.toContain('已复制');
  expect(host.querySelector('button')!.disabled).toBe(false);
});
