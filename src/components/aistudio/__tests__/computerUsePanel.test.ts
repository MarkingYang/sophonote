// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('../../../services/tauri', () => ({
  computerUseStatus: vi.fn(), computerUseActionStart: vi.fn(),
  computerUseActionStatus: vi.fn(), hermesToolsetSetEnabled: vi.fn(),
}));
import { computerUseStatus, computerUseActionStart, computerUseActionStatus, type ComputerUseStatus } from '../../../services/tauri';
import { ComputerUsePanel, computerReadiness } from '../ComputerUsePanel';

const diagnostic: ComputerUseStatus = { installed: true, platform: 'darwin', platformSupported: true,
  version: 'test', ready: null, canGrant: true, accessibility: null, screenRecording: null, checks: [], error: null };
let root: Root;
let container: HTMLDivElement;
async function render(props = {}) {
  await act(async () => root.render(createElement(ComputerUsePanel, { enabled: true, supported: true, onRefresh: vi.fn(), ...props })));
}
function button(text: string) {
  return [...container.querySelectorAll('button')].find((item) => item.textContent === text)!;
}

describe('电脑操作状态与生命周期', () => {
  beforeEach(() => {
    vi.useFakeTimers(); vi.clearAllMocks();
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
    vi.mocked(computerUseStatus).mockResolvedValue(diagnostic);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); });
  it('keeps unknown permissions visible and does not claim ready', async () => {
    await render();
    expect(container.textContent).toContain('辅助功能：未知');
    expect(container.textContent).not.toContain('驱动已就绪');
    expect(computerReadiness({ ...diagnostic, ready: true })).toBe('驱动已就绪');
  });
  it('a failed refresh removes a formerly ready status', async () => {
    vi.mocked(computerUseStatus).mockResolvedValueOnce({ ...diagnostic, ready: true }).mockRejectedValueOnce(new Error('disconnected'));
    await render();
    expect(container.textContent).toContain('驱动已就绪');
    await act(async () => (container.querySelector('[aria-label="重新检测电脑操作"]') as HTMLButtonElement).click());
    expect(container.textContent).not.toContain('驱动已就绪');
    expect(container.textContent).toContain('disconnected');
  });
  it('does not offer setup mutations during a running conversation, but supports takeover', async () => {
    const stop = vi.fn(async () => {});
    await render({ locked: true, running: true, onStop: stop });
    expect(button('关闭电脑工具').disabled).toBe(true);
    expect(button('前往系统授权').disabled).toBe(true);
    await act(async () => button('停止并接管').click());
    expect(stop).toHaveBeenCalledOnce();
    expect(container.textContent).toContain('等待当前运行结束');
  });
  it('reports nonzero setup exit and stops polling after completion', async () => {
    vi.mocked(computerUseStatus).mockResolvedValue({ ...diagnostic, installed: false });
    vi.mocked(computerUseActionStart).mockResolvedValue({ running: true, exitCode: null, pid: 12 });
    vi.mocked(computerUseActionStatus).mockResolvedValue({ running: false, exitCode: 1, pid: 12 });
    await render();
    await act(async () => button('安装电脑驱动').click());
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(container.textContent).toContain('退出码 1');
    await act(async () => vi.advanceTimersByTimeAsync(10000));
    expect(computerUseActionStatus).toHaveBeenCalledOnce();
  });
  it('does not poll when unmounted and resumes the same action when reopened', async () => {
    vi.mocked(computerUseActionStart).mockResolvedValue({ running: true, exitCode: null, pid: 13 });
    vi.mocked(computerUseActionStatus).mockResolvedValue({ running: false, exitCode: 0, pid: 13 });
    await render();
    await act(async () => button('前往系统授权').click());
    await act(async () => root.render(null));
    await act(async () => vi.advanceTimersByTimeAsync(10000));
    expect(computerUseActionStatus).not.toHaveBeenCalled();
    await render();
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(computerUseActionStatus).toHaveBeenCalledWith('grant');
  });
  it('does not treat installer exit zero as a successful installation', async () => {
    vi.mocked(computerUseStatus).mockResolvedValue({ ...diagnostic, installed: false });
    vi.mocked(computerUseActionStart).mockResolvedValue({ running: true, exitCode: null, pid: 15 });
    vi.mocked(computerUseActionStatus).mockResolvedValue({ running: false, exitCode: 0, pid: 15 });
    await render();
    await act(async () => button('安装电脑驱动').click());
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(container.textContent).toContain('未检测到驱动');
    expect(container.textContent).not.toContain('驱动已就绪');
  });
  it('embedded setup names SophoNote and never offers a standalone installer', async () => {
    vi.mocked(computerUseStatus).mockResolvedValue({ ...diagnostic, embedded: true, installed: false });
    await render();
    expect(button('安装电脑驱动')).toBeUndefined();
    expect(container.textContent).toContain('内置组件缺失');
    expect(container.textContent).not.toContain('CuaDriver');
  });
  it('native permission request finishes without pretending the user granted it', async () => {
    vi.mocked(computerUseStatus).mockResolvedValue({ ...diagnostic, embedded: true, accessibility: false, screenRecording: false, ready: false });
    vi.mocked(computerUseActionStart).mockResolvedValue({ running: false, exitCode: 0, pid: null });
    await render();
    await act(async () => button('前往系统授权').click());
    expect(container.textContent).toContain('为 SophoNote 开启');
    expect(container.textContent).toContain('等待系统授权');
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(computerUseActionStatus).not.toHaveBeenCalled();
  });

});
