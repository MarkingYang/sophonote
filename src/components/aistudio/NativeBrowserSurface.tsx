import { useEffect, useRef, useState } from 'react';
import { Globe2, Loader2 } from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import { LogicalPosition, LogicalSize } from '@tauri-apps/api/dpi';
import { Webview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { openUrl } from '@tauri-apps/plugin-opener';
import { usePageSurfaceActive } from '../layout/KeptAlivePage';
import { nativeWebviewLayout } from '../../services/nativeWebviewLayout';

interface NativeBrowserSurfaceProps {
  url: string;
  onError: (message: string | null) => void;
  onFileDrop?: (paths: string[]) => void;
}

/**
 * 网页、localhost 与本地可浏览文件共用同一个原生子 WebView。
 * 第三方站点和本地内容不经过 iframe。
 * 页签保活隐藏时必须停泊到屏外，不能沿用上一帧矩形。
 */
export default function NativeBrowserSurface({ url, onError, onFileDrop }: NativeBrowserSurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);
  const pageActive = usePageSurfaceActive();
  const pageActiveRef = useRef(pageActive);
  const syncBoundsRef = useRef<() => void>(() => undefined);
  pageActiveRef.current = pageActive;

  useEffect(() => {
    const host = hostRef.current;
    if (!host || !isTauri()) return;

    let disposed = false;
    let created = false;
    let frameRequest = 0;
    let unlistenDrop: (() => void) | undefined;
    const label = `sophonote_browser_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`;
    const initial = nativeWebviewLayout(pageActiveRef.current, host.getBoundingClientRect());
    const webview = new Webview(getCurrentWindow(), label, {
      url,
      x: initial.x,
      y: initial.y,
      width: initial.width,
      height: initial.height,
      focus: pageActiveRef.current && !initial.parked,
      acceptFirstMouse: true,
      dragDropEnabled: true,
      zoomHotkeysEnabled: true,
    });

    const syncBounds = () => {
      cancelAnimationFrame(frameRequest);
      frameRequest = requestAnimationFrame(() => {
        if (disposed || !created || !host.isConnected) return;
        const next = nativeWebviewLayout(pageActiveRef.current, host.getBoundingClientRect());
        void Promise.all([
          webview.setPosition(new LogicalPosition(next.x, next.y)),
          webview.setSize(new LogicalSize(next.width, next.height)),
        ]).catch((boundsError) => {
          if (!disposed) onError(`浏览器区域调整失败：${String(boundsError)}`);
        });
      });
    };
    syncBoundsRef.current = syncBounds;

    void webview.once('tauri://created', () => {
      created = true;
      if (disposed) {
        void webview.close().catch(() => undefined);
        return;
      }
      setReady(true);
      onError(null);
      syncBounds();
    });
    void webview.onDragDropEvent((event) => {
      if (event.payload.type === 'drop') onFileDrop?.(event.payload.paths);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenDrop = unlisten;
    }).catch((dropError) => {
      if (!disposed) onError(`文件拖放不可用：${String(dropError)}`);
    });
    void webview.once('tauri://error', (event) => {
      if (!disposed) onError(`内容打开失败：${String(event.payload)}`);
    });

    const observer = new ResizeObserver(syncBounds);
    observer.observe(host);
    window.addEventListener('resize', syncBounds);
    window.addEventListener('scroll', syncBounds, true);

    return () => {
      disposed = true;
      syncBoundsRef.current = () => undefined;
      observer.disconnect();
      window.removeEventListener('resize', syncBounds);
      window.removeEventListener('scroll', syncBounds, true);
      cancelAnimationFrame(frameRequest);
      unlistenDrop?.();
      if (created) void webview.close().catch(() => undefined);
    };
  }, [onError, onFileDrop, url]);

  useEffect(() => {
    pageActiveRef.current = pageActive;
    syncBoundsRef.current();
  }, [pageActive]);

  if (!isTauri()) {
    return (
      <div ref={hostRef} className="flex min-h-0 flex-1 flex-col items-center justify-center bg-white px-6 text-center">
        <Globe2 size={24} className="text-[var(--text-disabled)]" />
        <p className="mt-2 text-xs text-[var(--text-tertiary)]">浏览器预览需要在 SophoNote 桌面应用中使用</p>
        <button type="button" onClick={() => void openUrl(url)} className="mt-3 rounded bg-[var(--accent)] px-3 py-1.5 text-xs text-white">在系统浏览器打开</button>
      </div>
    );
  }

  return (
    <div ref={hostRef} className="relative min-h-0 flex-1 bg-white">
      {!ready && <div className="absolute inset-0 flex items-center justify-center text-xs text-[var(--text-tertiary)]"><Loader2 size={14} className="mr-2 animate-spin" />正在打开内容…</div>}
    </div>
  );
}
