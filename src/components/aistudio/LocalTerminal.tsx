import { useEffect, useRef } from 'react';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import {
  closeLocalTerminal,
  createLocalTerminal,
  listenLocalTerminalExit,
  listenLocalTerminalOutput,
  resizeLocalTerminal,
  writeLocalTerminal,
} from '../../services/tauri';
import type { WorkspacePermissionMode } from '../../services/workspaceBinding';

interface LocalTerminalProps {
  root: string;
  permissionMode: WorkspacePermissionMode;
  clearToken?: number;
  onError?: (message: string) => void;
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export default function LocalTerminal({ root, permissionMode, clearToken = 0, onError }: LocalTerminalProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const sessionIdRef = useRef<string | null>(null);
  const permissionRef = useRef(permissionMode);
  const onErrorRef = useRef(onError);

  useEffect(() => { permissionRef.current = permissionMode; }, [permissionMode]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);

  useEffect(() => {
    terminalRef.current?.clear();
  }, [clearToken]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let resizeFrame = 0;
    let resizeObserver: ResizeObserver | null = null;
    let inputDisposable: { dispose: () => void } | null = null;
    let unlistenOutput: (() => void) | null = null;
    let unlistenExit: (() => void) | null = null;
    const pendingOutput = new Map<string, Uint8Array[]>();

    const terminal = new Terminal({
      cursorBlink: true,
      cursorStyle: 'block',
      fontFamily: 'SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      lineHeight: 1.25,
      scrollback: 10_000,
      allowProposedApi: false,
      theme: {
        background: '#15171a',
        foreground: '#d7dce2',
        cursor: '#d7dce2',
        cursorAccent: '#15171a',
        selectionBackground: '#35536f',
        black: '#1b1d20',
        red: '#ef6b73',
        green: '#78c68b',
        yellow: '#e5c07b',
        blue: '#69a7e3',
        magenta: '#c58bd4',
        cyan: '#65c7c9',
        white: '#d7dce2',
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host);
    terminalRef.current = terminal;

    const fitAndResize = () => {
      cancelAnimationFrame(resizeFrame);
      resizeFrame = requestAnimationFrame(() => {
        if (disposed || !host.isConnected) return;
        try {
          fit.fit();
          const sessionId = sessionIdRef.current;
          if (sessionId) void resizeLocalTerminal(sessionId, terminal.cols, terminal.rows).catch(() => {});
        } catch {
          // xterm may be hidden for one layout frame while panes are rearranged.
        }
      });
    };

    const start = async () => {
      try {
        [unlistenOutput, unlistenExit] = await Promise.all([
          listenLocalTerminalOutput((payload) => {
            const bytes = decodeBase64(payload.data);
            if (payload.sessionId === sessionIdRef.current) terminal.write(bytes);
            else if (sessionIdRef.current == null) {
              const buffered = pendingOutput.get(payload.sessionId) ?? [];
              buffered.push(bytes);
              pendingOutput.set(payload.sessionId, buffered);
            }
          }),
          listenLocalTerminalExit((payload) => {
            if (payload.sessionId === sessionIdRef.current) terminal.write('\r\n\x1b[2m[进程已结束]\x1b[0m\r\n');
          }),
        ]);
        fit.fit();
        const sessionId = await createLocalTerminal(root, terminal.cols, terminal.rows);
        if (disposed) {
          await closeLocalTerminal(sessionId).catch(() => {});
          return;
        }
        sessionIdRef.current = sessionId;
        for (const bytes of pendingOutput.get(sessionId) ?? []) terminal.write(bytes);
        pendingOutput.clear();
        inputDisposable = terminal.onData((data) => {
          if (permissionRef.current === 'plan') return;
          const activeSessionId = sessionIdRef.current;
          if (!activeSessionId) return;
          void writeLocalTerminal(activeSessionId, new TextEncoder().encode(data)).catch((error) => {
            onErrorRef.current?.(error instanceof Error ? error.message : String(error));
          });
        });
        terminal.attachCustomKeyEventHandler((event) => {
          if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k' && event.type === 'keydown') {
            terminal.clear();
            return false;
          }
          return true;
        });
        resizeObserver = new ResizeObserver(fitAndResize);
        resizeObserver.observe(host);
        fitAndResize();
        terminal.focus();
      } catch (error) {
        onErrorRef.current?.(error instanceof Error ? error.message : String(error));
      }
    };

    void start();
    return () => {
      disposed = true;
      cancelAnimationFrame(resizeFrame);
      resizeObserver?.disconnect();
      inputDisposable?.dispose();
      unlistenOutput?.();
      unlistenExit?.();
      const sessionId = sessionIdRef.current;
      sessionIdRef.current = null;
      if (sessionId) void closeLocalTerminal(sessionId).catch(() => {});
      terminal.dispose();
      terminalRef.current = null;
    };
  }, [root]);

  return <div ref={hostRef} className="h-full min-h-0 w-full bg-[#15171a] p-1.5" onClick={() => terminalRef.current?.focus()} />;
}
