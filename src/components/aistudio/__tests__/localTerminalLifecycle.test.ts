import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@xterm/xterm', () => ({ Terminal: class { cols = 80; rows = 24; loadAddon() {} open() {} clear() {} write() {} focus() {} dispose() {} onData() { return { dispose() {} }; } attachCustomKeyEventHandler() {} } }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }));
vi.mock('../../../services/tauri', () => ({ closeLocalTerminal: vi.fn().mockResolvedValue(undefined), createLocalTerminal: vi.fn(), listenLocalTerminalExit: vi.fn(), listenLocalTerminalOutput: vi.fn(), resizeLocalTerminal: vi.fn().mockResolvedValue(undefined), writeLocalTerminal: vi.fn().mockResolvedValue(undefined) }));
import LocalTerminal from '../LocalTerminal';
import * as api from '../../../services/tauri';
let host: HTMLDivElement; let root: Root;
const outputOff = vi.fn(); const exitOff = vi.fn();
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>((r) => { resolve = r; }); return { promise, resolve }; };
beforeEach(() => {
  vi.clearAllMocks(); Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  vi.mocked(api.listenLocalTerminalOutput).mockResolvedValue(outputOff);
  vi.mocked(api.listenLocalTerminalExit).mockResolvedValue(exitOff);
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); vi.unstubAllGlobals(); });
const mount = () => act(async () => root.render(createElement(LocalTerminal, { root: '/workspace', permissionMode: 'default' })));
const close = () => act(async () => root.render(null));
describe('扩展终端异步回收', () => {
  it('关闭时监听尚未就绪，晚到的监听立即取消且不创建 PTY', async () => {
    const pending = deferred<() => void>();
    vi.mocked(api.listenLocalTerminalOutput).mockReturnValue(pending.promise);
    await mount(); await close();
    await act(async () => pending.resolve(outputOff));
    expect(outputOff).toHaveBeenCalledTimes(1);
    expect(api.createLocalTerminal).not.toHaveBeenCalled();
    expect(api.listenLocalTerminalExit).not.toHaveBeenCalled();
  });
  it('关闭时 PTY 创建尚未返回，返回后立即销毁且监听已清除', async () => {
    const pending = deferred<string>();
    vi.mocked(api.createLocalTerminal).mockReturnValue(pending.promise);
    await mount();
    expect(api.createLocalTerminal).toHaveBeenCalledWith('/workspace', 80, 24);
    await close();
    expect(outputOff).toHaveBeenCalledTimes(1); expect(exitOff).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve('late-session'));
    expect(api.closeLocalTerminal).toHaveBeenCalledWith('late-session');
  });
});
