// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { RunRoutingDetails } from '../RunRoutingDetails';

const call = vi.mocked(invoke);
let root: Root;
let container: HTMLDivElement;
const render = async (runId: string) => {
  await act(async () => root.render(createElement(RunRoutingDetails, { runId, engine: 'pi', engineLabel: 'Pi' })));
};
const toggle = async (open: boolean) => {
  await act(async () => {
    const details = container.querySelector('details')!;
    details.open = open;
    details.dispatchEvent(new Event('toggle'));
  });
};
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  call.mockReset(); container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });

it('仅展开时读取当次审计，旧数据不编造自动路由原因', async () => {
  call.mockResolvedValue({ success: true, data: { runId: 'old' } });
  await render('old'); expect(call).not.toHaveBeenCalled();
  await toggle(true);
  expect(call).toHaveBeenCalledWith('agent_run_snapshot', { runId: 'old' });
  expect(container.textContent).toContain('此历史运行未记录选择依据');
  expect(container.textContent).toContain('实际执行：Pi');
});

it('只显示与当前 Run 和引擎一致的持久化说明', async () => {
  call.mockResolvedValue({ success: true, data: { runId: 'new', routing: { engine: 'pi', policyVersion: 'automatic-v2', taskLabel: '限定资料处理', reason: '围绕提供的资料处理' } } });
  await render('new'); await toggle(true);
  expect(container.textContent).toContain('限定资料处理');
  expect(container.textContent).toContain('automatic-v2');
});

it('切换 Run 后忽略旧查询迟到的结果', async () => {
  let resolveOld!: (value: unknown) => void;
  call.mockImplementation(async (_command, args) => args?.runId === 'old'
    ? new Promise((resolve) => { resolveOld = resolve; })
    : { success: true, data: { runId: 'new', routing: { engine: 'pi', policyVersion: 'automatic-v2', taskLabel: null, reason: '新运行依据' } } });
  await render('old'); await toggle(true); await render('new');
  await act(async () => resolveOld({ success: true, data: { runId: 'old', routing: { engine: 'pi', reason: '旧运行污染' } } }));
  expect(container.textContent).toContain('新运行依据');
  expect(container.textContent).not.toContain('旧运行污染');
});

it('查询失败不显示原生错误，重新展开可重试', async () => {
  call.mockRejectedValueOnce(new Error('PRIVATE native failure')).mockResolvedValue({ success: true, data: { runId: 'r' } });
  await render('r'); await toggle(true);
  expect(container.textContent).toContain('重新展开可重试');
  expect(container.textContent).not.toContain('PRIVATE');
  await toggle(false); await toggle(true);
  expect(container.textContent).toContain('此历史运行未记录选择依据');
});
