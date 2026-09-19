import { useEffect, useRef, useState } from 'react';
import { Loader2, Monitor, RefreshCw, X } from 'lucide-react';
import {
  computerUseActionStart, computerUseActionStatus, computerUseStatus, hermesToolsetSetEnabled,
  type ComputerUseAction, type ComputerUseStatus,
} from '../../services/tauri';

export const COMPUTER_NOTE_SKILL = 'sophonote-computer-note';
// Keep the action identity across panel close/reopen; no background UI polling.
let pendingSetup: { action: ComputerUseAction; pid: number | null } | null = null;

export function computerReadiness(status: ComputerUseStatus | null): string {
  if (!status) return '尚未检测';
  if (!status.platformSupported) return '当前系统暂不支持';
  if (!status.installed) return status.embedded ? '内置组件缺失' : '尚未安装驱动';
  if (status.ready === true) return status.embedded ? '电脑操作已就绪' : '驱动已就绪';
  if (status.accessibility === false || status.screenRecording === false) return '等待系统授权';
  return '需要重新检测';
}

function permissionLabel(value: boolean | null) {
  return value === true ? '已授权' : value === false ? '未授权' : '未知';
}

export function ComputerUsePanel({ enabled, supported, locked = false, running = false, hasSession = false,
  hasNote = false, skillAvailable = false, onRefresh, onPrepare, onNewSession, onStop, onClose,
}: {
  enabled: boolean; supported: boolean | null; locked?: boolean; running?: boolean; hasSession?: boolean;
  hasNote?: boolean; skillAvailable?: boolean; onRefresh: () => void;
  onPrepare?: (text: string) => void; onNewSession?: () => Promise<void>;
  onStop?: () => Promise<void>; onClose?: () => void;
}) {
  const [status, setStatus] = useState<ComputerUseStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState(pendingSetup);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState('');
  const [target, setTarget] = useState('');
  const [stopping, setStopping] = useState(false);
  const alive = useRef(true);
  const requestId = useRef(0);

  const refresh = async () => {
    const id = ++requestId.current;
    setBusy(true); setError(null); setStatus(null);
    try {
      const next = await computerUseStatus();
      if (alive.current && id === requestId.current) setStatus(next);
      return next;
    } catch (reason) {
      if (alive.current && id === requestId.current) setError(String(reason));
    } finally {
      if (alive.current && id === requestId.current) setBusy(false);
    }
  };

  useEffect(() => {
    alive.current = true;
    void refresh();
    return () => { alive.current = false; requestId.current += 1; };
  }, []);

  useEffect(() => {
    if (!pending) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const result = await computerUseActionStatus(pending.action);
        if (cancelled) return;
        if (result.pid !== pending.pid) throw new Error('配置进程已变化，请重新检测驱动状态。');
        if (result.running) { timer = setTimeout(() => void poll(), 1500); return; }
        pendingSetup = null; setPending(null);
        if (result.exitCode !== 0) {
          setError(result.exitCode == null ? '配置结果未知，请重新检测后重试。' : pending.action === 'grant'
            ? '系统授权未完成。请在「系统设置 → 隐私与安全」为 SophoNote 开启辅助功能和屏幕录制，再重新检测。'
            : `配置未完成（退出码 ${result.exitCode}），请重新检测后重试。`);
          return;
        }
        setNotice(pending.action === 'install' ? '安装流程已结束，正在检测驱动与权限。' : '授权流程已结束，正在确认权限。');
        const next = await refresh();
        if (alive.current && next && pending.action === 'install' && !next.installed) {
          setError('安装流程已结束，但未检测到驱动。请检查网络连接后重试；Hermes 安装器退出成功不代表驱动已安装。');
        }
      } catch (reason) {
        if (!cancelled) {
          setError(`${String(reason)} 关闭后重新打开面板可继续查询。`);
          // Stop on transport failure. Preserve identity for explicit retry/reopen.
        }
      }
    };
    timer = setTimeout(() => void poll(), 1000);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [pending]);

  const start = async (action: ComputerUseAction) => {
    if (busy || pending || locked) return;
    setBusy(true); setError(null); setNotice('');
    try {
      const result = await computerUseActionStart(action);
      if (!result.running) {
        if (result.exitCode !== 0) throw new Error('配置未完成，请重新检测。');
        setNotice('请在「系统设置 → 隐私与安全」为 SophoNote 开启辅助功能和屏幕录制，然后重新检测；若系统提示，请退出并重新打开应用。');
        await refresh();
        return;
      }
      pendingSetup = { action, pid: result.pid };
      if (alive.current) setPending(pendingSetup);
    } catch (reason) { if (alive.current) setError(String(reason)); }
    finally { if (alive.current) setBusy(false); }
  };

  const toggle = async () => {
    setBusy(true); setError(null);
    try {
      await hermesToolsetSetEnabled('computer_use', !enabled);
      if (alive.current) {
        setNotice(enabled ? '已关闭新任务的电脑工具。已有任务请停止后新建。' : '电脑工具已启用。已有任务请新建后使用，以保留原任务历史。');
        onRefresh();
      }
    } catch (reason) { if (alive.current) setError(String(reason)); }
    finally { if (alive.current) setBusy(false); }
  };

  const ready = status?.ready === true && enabled && supported && !busy && !pending;
  const buttonClass = 'rounded-lg border border-[var(--border-default)] px-3 py-2 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] disabled:opacity-40';
  return (
    <section aria-label="电脑操作" className="flex min-h-0 flex-col bg-[var(--bg-surface)]">
      <header className="flex items-center gap-2 border-b border-[var(--border-default)] px-4 py-3">
        <Monitor size={16} className="text-[var(--accent)]" /><h3 className="flex-1 text-sm font-semibold">电脑操作</h3>
        {onClose && <button aria-label="关闭电脑操作" onClick={onClose} className="text-[var(--text-tertiary)]"><X size={16} /></button>}
      </header>
      <div className="space-y-4 overflow-y-auto p-4 text-xs text-[var(--text-secondary)]">
        <p className="leading-relaxed">读取你指定的应用窗口，按指令操作电脑，并将结果整理到任务或当前笔记。</p>
        <div className="rounded-xl bg-[var(--bg-sunken)] p-3">
          <div className="flex items-center gap-2"><span className="flex-1 font-medium" role="status">{busy ? '正在检测或配置…' : computerReadiness(status)}</span><button aria-label="重新检测电脑操作" disabled={busy || !!pending} onClick={() => void refresh()}><RefreshCw size={13} /></button></div>
          {status?.embedded && <p className="mt-2">组件已随应用提供，系统权限归属：{status.permissionOwner || '待验证'}。</p>}
          {status?.version && <p className="mt-1 text-[var(--text-tertiary)]">{status.version}</p>}
          {status?.canGrant && <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1"><span>辅助功能：{permissionLabel(status.accessibility)}</span><span>屏幕录制：{permissionLabel(status.screenRecording)}</span></div>}
          <p className="mt-2">电脑工具：{supported == null ? '正在读取能力目录…' : enabled ? '已启用' : '未启用'}{supported === false && ' · 当前 Runtime 未提供该工具'}</p>
        </div>
        {(error || status?.error) && <p role="alert" className="break-words rounded-lg bg-[var(--danger-subtle)] p-3 text-[var(--danger)]">{error || status?.error}</p>}
        {notice && <p role="status" className="leading-relaxed">{notice}</p>}
        {pending && <div role="status" className="flex items-center gap-2"><Loader2 size={13} className="animate-spin" /><span>{pending.action === 'install' ? '正在安装电脑驱动…' : '等待你在 macOS 中完成授权…'}</span>{error && <button className={buttonClass} onClick={() => { setError(null); setPending({ ...pending }); }}>继续查询</button>}</div>}
        <div className="flex flex-wrap gap-2">
          {status && !status.embedded && !status.installed && status.platformSupported && <button className={buttonClass} disabled={busy || !!pending || locked} onClick={() => void start('install')}>安装电脑驱动</button>}
          {status?.installed && status.canGrant && status.ready !== true && <button className={buttonClass} disabled={busy || !!pending || locked} onClick={() => void start('grant')}>前往系统授权</button>}
          <button className={buttonClass} disabled={busy || !!pending || locked || !supported || (!enabled && status?.ready !== true)} onClick={() => void toggle()}>{enabled ? '关闭电脑工具' : '启用电脑工具'}</button>
          {hasSession && onNewSession && <button className={buttonClass} disabled={locked || busy || !!pending} onClick={() => void onNewSession().catch((reason) => setError(String(reason)))}>新建任务使用</button>}
        </div>
        {hasSession && <p className="text-[var(--text-tertiary)]">首次安装或启用后请新建任务，原任务历史会保留。关闭工具也只对新任务生效。</p>}
        {onPrepare && <div className="space-y-2 border-t border-[var(--border-default)] pt-4">
          <label htmlFor="computer-use-target" className="block font-medium">目标应用或窗口</label>
          <input id="computer-use-target" value={target} onChange={(event) => setTarget(event.target.value)} maxLength={200} placeholder="例如：预览中的项目方案.pdf" disabled={locked} className="w-full rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] px-3 py-2 outline-none focus:border-[var(--accent-border)]" />
          <button className={buttonClass} disabled={!ready || locked || !target.trim() || !skillAvailable} onClick={() => onPrepare(`读取「${target.trim()}」窗口的内容，先确认目标再操作，整理要点${hasNote ? '并追加到当前笔记，生成待审阅修改' : '并在当前任务中回答'}。`)}>准备整理{hasNote ? '到当前笔记' : '到任务'}</button>
          {!skillAvailable && <p className="text-[var(--warning)]">笔记整理 Skill 尚未加载，请刷新 Hermes 能力或重启应用。</p>}
          <p className="text-[var(--text-tertiary)]">任务会填入输入框，你可以修改后发送。动作与审批显示在任务过程里。</p>
        </div>}
        {running && onStop && <button className="w-full rounded-lg bg-[var(--danger-subtle)] px-3 py-2 font-medium text-[var(--danger)] disabled:opacity-40" disabled={stopping} onClick={async () => {
          setStopping(true); setError(null);
          try { await onStop(); if (alive.current) setNotice('已请求停止，等待当前运行结束后可接管。'); }
          catch (reason) { if (alive.current) setError(String(reason)); }
          finally { if (alive.current) setStopping(false); }
        }}>{stopping ? '正在请求停止…' : '停止并接管'}</button>}
        <p className="leading-relaxed text-[var(--text-tertiary)]">所选窗口的截图与文字可能发送给当前模型。系统权限需要你手动授权；笔记内容通过变更预览确认后保存。</p>
        {status && status.checks.length > 0 && <details><summary className="cursor-pointer">查看诊断详情</summary><ul className="mt-2 space-y-2">{status.checks.map((check, index) => <li key={index} className="break-words"><span className="font-medium">{check.label} · {check.status}</span><p className="text-[var(--text-tertiary)]">{check.message}</p></li>)}</ul></details>}
      </div>
    </section>
  );
}
