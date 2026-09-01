import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ExternalLink,
  Globe2,
  NotebookPen,
  RefreshCw,
} from 'lucide-react';
import { convertFileSrc } from '@tauri-apps/api/core';
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { openPath, openUrl } from '@tauri-apps/plugin-opener';
import { hermesBrowserManage, hermesCapabilities, type HermesCapabilities } from '../../services/tauri';
import { useAppStore } from '../../stores/appStore';
import {
  browserFileKind,
  fileNameFromPath,
  isBrowserPreviewFile,
  isPdfUrl,
  normalizeBrowserUrl,
  type BrowserFileKind,
} from '../../services/browserNavigation';
import NativeBrowserSurface from './NativeBrowserSurface';

interface AgentBrowserPanelProps {
  onAddToChat: (target: BrowserTarget) => void;
  onConnectionChange?: (connected: boolean) => void;
}

export interface BrowserTarget {
  url: string;
  display: string;
  name: string;
  kind: 'web' | Exclude<BrowserFileKind, 'unsupported'>;
  localPath?: string;
}

let browserConnectionRequest: Promise<HermesCapabilities> | null = null;

function ensureBrowserConnected(): Promise<HermesCapabilities> {
  if (!browserConnectionRequest) {
    browserConnectionRequest = hermesCapabilities()
      .then((snapshot) => snapshot.browserConnected ? snapshot : hermesBrowserManage('connect'))
      .finally(() => { browserConnectionRequest = null; });
  }
  return browserConnectionRequest;
}

export default function AgentBrowserPanel({ onAddToChat, onConnectionChange }: AgentBrowserPanelProps) {
  const saveArticle = useAppStore((state) => state.saveArticle);
  const [history, setHistory] = useState<BrowserTarget[]>([]);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [draft, setDraft] = useState('');
  const [frameKey, setFrameKey] = useState(0);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const currentTarget = historyIndex >= 0 ? history[historyIndex] : null;
  const currentUrl = currentTarget?.url ?? '';

  useEffect(() => {
    let active = true;
    const connectBrowser = async () => {
      try {
        const snapshot = await ensureBrowserConnected();
        if (!active) return;
        onConnectionChange?.(snapshot.browserConnected);
        if (!snapshot.browserConnected) setMessage('Agent Browser 连接失败');
      } catch (error) {
        if (!active) return;
        onConnectionChange?.(false);
        setMessage(error instanceof Error ? error.message : String(error));
      }
    };
    void connectBrowser();
    return () => { active = false; };
  }, [onConnectionChange]);

  const host = useMemo(() => {
    if (!currentTarget) return '';
    if (currentTarget.localPath) return currentTarget.name;
    try { return new URL(currentUrl).host; } catch { return currentUrl; }
  }, [currentTarget, currentUrl]);

  const pushTarget = useCallback((target: BrowserTarget) => {
    setHistory((current) => {
      const next = [...current.slice(0, historyIndex + 1), target];
      setHistoryIndex(next.length - 1);
      return next;
    });
    setDraft(target.display);
    setFrameKey((current) => current + 1);
    setMessage(null);
  }, [historyIndex]);

  const navigate = (value: string) => {
    const url = normalizeBrowserUrl(value);
    if (!url) {
      setMessage('请输入有效的网址或 localhost 地址');
      return;
    }
    pushTarget({
      url,
      display: url,
      name: isPdfUrl(url) ? fileNameFromPath(new URL(url).pathname) : new URL(url).host,
      kind: isPdfUrl(url) ? 'pdf' : 'web',
    });
  };

  const openDroppedFiles = useCallback((paths: string[]) => {
    const selected = paths.find(isBrowserPreviewFile);
    if (!selected) {
      setMessage('该文件不能在浏览器中打开');
      return;
    }
    const kind = browserFileKind(selected);
    if (kind === 'unsupported') return;
    pushTarget({
      url: convertFileSrc(selected),
      display: selected,
      name: fileNameFromPath(selected),
      kind,
      localPath: selected,
    });
  }, [pushTarget]);

  useEffect(() => {
    if (currentUrl || !isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'drop') openDroppedFiles(event.payload.paths);
    }).then((nextUnlisten) => {
      if (disposed) nextUnlisten();
      else unlisten = nextUnlisten;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [currentUrl, openDroppedFiles]);

  const moveHistory = (next: number) => {
    if (next < 0 || next >= history.length) return;
    setHistoryIndex(next);
    setDraft(history[next].display);
    setFrameKey((current) => current + 1);
  };

  const saveAsNote = async () => {
    if (!currentUrl) return;
    setBusy(true);
    setMessage(null);
    try {
      await saveArticle({
        id: crypto.randomUUID(),
        title: host || (currentTarget?.localPath ? '文件笔记' : '网页笔记'),
        content: currentTarget?.localPath
          ? `# ${host || '文件笔记'}\n\n本地文件：\`${currentTarget.localPath}\`\n\n> 从 Agent Browser 保存。文件内容不会自动进入长期记忆。\n`
          : `# ${host || '网页笔记'}\n\n来源：[${currentUrl}](${currentUrl})\n\n> 从 Agent Browser 保存。浏览内容不会自动进入长期记忆。\n`,
        articleType: 'manual',
        edited: false,
        createdAt: new Date().toISOString(),
        blocksJson: null,
      });
      setMessage('已保存为 Markdown 笔记');
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="h-full min-h-0 flex flex-col bg-[var(--bg-canvas)]">
      <div className="h-10 shrink-0 border-b border-[var(--border-default)] bg-[var(--bg-surface)] px-2 flex items-center gap-1.5">
        <button type="button" disabled={historyIndex <= 0} onClick={() => moveHistory(historyIndex - 1)} className="h-7 w-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-30" title="后退"><ArrowLeft size={13} /></button>
        <button type="button" disabled={historyIndex < 0 || historyIndex >= history.length - 1} onClick={() => moveHistory(historyIndex + 1)} className="h-7 w-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-30" title="前进"><ArrowRight size={13} /></button>
        <button type="button" disabled={!currentUrl} onClick={() => setFrameKey((current) => current + 1)} className="h-7 w-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] disabled:opacity-30" title="刷新"><RefreshCw size={12} /></button>
        <form className="min-w-0 flex-1" onSubmit={(event) => { event.preventDefault(); navigate(draft); }}>
          <div className="relative">
            <Globe2 size={11} className="absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
            <input value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="输入网址或 localhost" className="h-7 w-full rounded-md border border-[var(--border-default)] bg-[var(--bg-canvas)] pl-6 pr-2 font-mono text-xs text-[var(--text-secondary)] outline-none focus:border-[var(--accent)]" />
          </div>
        </form>
        {currentTarget && <button type="button" onClick={() => void (currentTarget.localPath ? openPath(currentTarget.localPath) : openUrl(currentUrl))} className="h-7 w-7 rounded-md flex items-center justify-center text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)]" title="在系统应用打开"><ExternalLink size={12} /></button>}
      </div>

      {message && <p className="shrink-0 border-b border-[var(--border-default)] bg-[var(--bg-sunken)] px-3 py-2 text-xs text-[var(--text-tertiary)]">{message}</p>}

      <div className="relative flex flex-1 min-h-0 bg-white">
        {currentUrl ? (
          <NativeBrowserSurface key={frameKey} url={currentUrl} onError={setMessage} onFileDrop={openDroppedFiles} />
        ) : (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-[var(--bg-canvas)] px-8 text-center">
            <Globe2 size={24} className="text-[var(--text-disabled)]" />
            <p className="mt-3 text-sm font-medium text-[var(--text-secondary)]">浏览器</p>
            <p className="mt-1 text-xs text-[var(--text-tertiary)]">输入网址，或拖入文件</p>
          </div>
        )}
      </div>

      {currentUrl && (
        <footer className="h-10 shrink-0 border-t border-[var(--border-default)] bg-[var(--bg-surface)] px-3 flex items-center justify-end gap-2">
          <button type="button" onClick={() => { if (currentTarget) onAddToChat(currentTarget); setMessage(`${currentTarget?.localPath ? '文件' : '网址'}已加入对话，切回 AI 对话后可继续下达任务`); }} className="h-7 rounded-md px-2.5 inline-flex items-center gap-1 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)]"><Check size={11} />加入对话</button>
          <button type="button" onClick={() => void saveAsNote()} disabled={busy} className="h-7 rounded-md px-2.5 inline-flex items-center gap-1 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40"><NotebookPen size={11} />保存为笔记</button>
        </footer>
      )}
    </section>
  );
}
