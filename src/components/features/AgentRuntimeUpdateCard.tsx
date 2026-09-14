import { useEffect, useState } from 'react';
import { DownloadCloud, Loader2, RefreshCw } from 'lucide-react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { getAgentUpdateStatus, listenAgentUpdateProgress, updateAgentRuntime, type AgentUpdateProgress, type AgentUpdateStatus, type UpdatableAgent } from '../../services/tauri';
import { safeUnlisten } from '../../services/browserErrors';

export default function AgentRuntimeUpdateCard({ engine }: { engine: UpdatableAgent }) {
  const isPi = engine === 'pi';
  const label = isPi ? 'Pi' : 'Claude Code';
  const [status, setStatus] = useState<AgentUpdateStatus | null>(null);
  const [progress, setProgress] = useState<AgentUpdateProgress | null>(null);
  const [loading, setLoading] = useState(true);
  const [updating, setUpdating] = useState(false);
  const [error, setError] = useState('');
  const busy = updating || progress?.state === 'running';
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listenAgentUpdateProgress((value) => {
      if (cancelled || value.engine !== engine) return;
      setProgress(value);
      if (value.state !== 'running') {
        void getAgentUpdateStatus(engine).then((next) => { if (!cancelled) setStatus(next); }).catch(() => {});
      }
    }).then((dispose) => { if (cancelled) safeUnlisten(dispose); else unlisten = dispose; }).catch(() => {});
    void getAgentUpdateStatus(engine).then((value) => {
      if (cancelled) return;
      setStatus(value); setProgress((previous) => previous ?? value.progress);
    }).catch((e: unknown) => { if (!cancelled) setError(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; safeUnlisten(unlisten); };
  }, [engine]);
  async function check() {
    setLoading(true); setError('');
    try { const value = await getAgentUpdateStatus(engine, true); setStatus(value); setProgress(value.progress); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setLoading(false); }
  }
  async function update() {
    setUpdating(true); setError(''); setProgress(null);
    let unlisten: (() => void) | undefined;
    try {
      // Complete subscription before dispatch, including the first check/download stage.
      unlisten = await listenAgentUpdateProgress((value) => { if (value.engine === engine) setProgress(value); });
      const value = await updateAgentRuntime(engine);
      setStatus(value); setProgress(value.progress);
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      setError(message);
      setProgress((previous) => previous?.state === 'running' ? { ...previous, state: 'failed', message } : previous);
    } finally { setUpdating(false); safeUnlisten(unlisten); }
  }
  const downloaded = progress?.bytesDownloaded;
  const total = progress?.totalBytes;
  const percentage = downloaded != null && total ? Math.min(100, Math.round(downloaded / total * 100)) : null;
  return (
    <section aria-label={`${label} 更新`} className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5">
      <div className="flex items-start justify-between gap-5">
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-[var(--text-primary)]">{label}</h3>
          <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">
            {isPi ? '从 Pi 官方稳定版下载，在应用私有目录校验。下一轮使用新版，当前运行不受影响。' : '按本机安装方式调用官方更新器，会更新本机 Claude Code CLI。请先结束正在执行的 Claude Code 会话。'}
          </p>
        </div>
        <button type="button" onClick={() => void update()} disabled={loading || busy || !status?.canUpdate}
          className="inline-flex h-9 shrink-0 items-center gap-2 rounded-lg bg-[var(--accent)] px-4 text-sm font-medium text-white hover:bg-[var(--accent-strong)] disabled:cursor-not-allowed disabled:opacity-50">
          {busy ? <Loader2 size={15} className="animate-spin" /> : <DownloadCloud size={15} />}
          {busy ? '更新中…' : isPi ? '拉取更新' : '更新本机 CLI'}
        </button>
      </div>
      <dl className="mt-5 grid grid-cols-[7rem_minmax(0,1fr)] gap-x-4 gap-y-2 border-t border-[var(--border-default)] pt-4 text-xs">
        <dt className="text-[var(--text-tertiary)]">可用版本</dt><dd className="font-mono text-[var(--text-secondary)]">{status?.currentVersion ? `v${status.currentVersion}` : loading ? '读取中…' : '未就绪'}</dd>
        <dt className="text-[var(--text-tertiary)]">运行来源</dt><dd className="text-[var(--text-secondary)]">{status?.source ?? '—'}</dd>
        <dt className="text-[var(--text-tertiary)]">更新策略</dt><dd className="text-[var(--text-secondary)]">{isPi ? '官方稳定版 · 私有版本槽 · 校验失败保留原版' : '跟随原安装渠道 · 手动更新 · 下一轮生效'}</dd>
        <dt className="text-[var(--text-tertiary)]">更新源</dt><dd className="text-[var(--text-secondary)]">{isPi ? 'earendil-works/pi · stable release' : 'Anthropic 官方更新器 / 原 Homebrew cask'}</dd>
        {status?.latestVersion && <><dt className="text-[var(--text-tertiary)]">官方稳定版</dt><dd className="font-mono text-[var(--text-secondary)]">v{status.latestVersion}{status.latestVersion === status.currentVersion ? ' · 已是当前版本' : ''}</dd></>}
      </dl>
      {progress && <div role="status" className={`mt-4 rounded-lg px-3 py-2.5 text-xs leading-5 ${progress.state === 'failed' ? 'bg-[var(--danger-subtle)] text-[var(--danger)]' : progress.state === 'completed' ? 'bg-[var(--success-subtle)] text-[var(--success)]' : 'bg-[var(--accent-subtle)] text-[var(--text-secondary)]'}`}>
        <p>{progress.message}</p>
        {downloaded != null && <p className="mt-1 font-mono">{(downloaded / 1048576).toFixed(1)} MB{total ? ` / ${(total / 1048576).toFixed(1)} MB` : ''}</p>}
        {percentage != null && <progress aria-label={`${label} 下载进度`} value={percentage} max={100} className="mt-2 h-1.5 w-full" />}
      </div>}
      {(error || status?.message) && <p role={error ? 'alert' : undefined} className={`mt-3 text-xs leading-5 ${error ? 'text-[var(--danger)]' : 'text-[var(--text-tertiary)]'}`}>{error || status?.message}</p>}
      <div className="mt-4 flex gap-4 text-xs">
        <button type="button" onClick={() => void check()} disabled={loading || busy} className="inline-flex items-center gap-1 text-[var(--text-secondary)] disabled:opacity-50"><RefreshCw size={12} className={loading ? 'animate-spin' : ''} />{isPi ? '检查版本' : '重新检测'}</button>
        <button type="button" onClick={() => void openUrl(isPi ? 'https://github.com/earendil-works/pi/releases' : 'https://code.claude.com/docs/en/setup').catch((e) => setError(String(e)))} className="text-[var(--accent)]">官方更新说明</button>
      </div>
    </section>
  );
}
