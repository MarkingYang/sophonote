import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircle,
  BookOpenText,
  CalendarClock,
  Check,
  ChevronDown,
  ChevronRight,
  CirclePause,
  Clock3,
  History,
  Loader2,
  Play,
  Plus,
  RefreshCw,
  Search,
  Trash2,
  X,
} from 'lucide-react';
import type { Project } from '../../types';
import {
  hermesCronCreate,
  hermesCronDelete,
  hermesCronJobs,
  hermesCronRunResult,
  hermesCronRuns,
  hermesCronSetEnabled,
  hermesCronTrigger,
  hermesCronUpdate,
  hermesModelOptions,
  projectList,
  restartHermesRuntime,
  type HermesCronDraft,
  type HermesCronJobInfo,
  type HermesCronRunInfo,
  type HermesCronRunResult,
  type HermesCronRunStatus,
  type HermesModelOptions,
} from '../../services/tauri';
import {
  cachedScheduledJobs,
  scheduledJobsCacheHydrated,
  setCachedScheduledJobs,
  shouldFetchScheduledJobsOnMount,
} from '../../services/scheduledJobsCache';
import {
  scheduledTaskExampleDraft,
  scheduledTaskExamples,
  type ScheduledTaskExample,
} from '../../services/scheduledTaskExamples';
import EmptyState from '../ui/EmptyState';

const MarkdownView = lazy(() => import('./MarkdownView'));

export type Frequency = 'hourly' | 'daily' | 'weekdays' | 'weekly' | 'interval' | 'once' | 'custom';

export interface EditorState {
  name: string;
  prompt: string;
  projectId: string;
  frequency: Frequency;
  time: string;
  weekday: string;
  intervalValue: string;
  intervalUnit: 'm' | 'h' | 'd';
  onceAt: string;
  customSchedule: string;
  skills: string[];
  modelValue: string;
}

const WEEKDAYS: Record<string, string> = {
  '1': '周一', '2': '周二', '3': '周三', '4': '周四', '5': '周五', '6': '周六', '0': '周日',
};

const EMPTY_EDITOR: EditorState = {
  name: '',
  prompt: '',
  projectId: '',
  frequency: 'daily',
  time: '09:00',
  weekday: '1',
  intervalValue: '1',
  intervalUnit: 'h',
  onceAt: '',
  customSchedule: '',
  skills: [],
  modelValue: '',
};

function formatDate(value: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat('zh-CN', {
    month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false,
  }).format(date);
}

function formatEpoch(value: number | null): string {
  return value == null ? '—' : formatDate(new Date(value * 1000).toISOString());
}

function formatDuration(startedAt: number | null, endedAt: number | null): string {
  if (startedAt == null || endedAt == null || endedAt < startedAt) return '';
  const seconds = Math.max(1, Math.round(endedAt - startedAt));
  return seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
}

interface ParsedCron {
  minute: string;
  hour: string;
  dayOfMonth: string;
  month: string;
  dayOfWeek: string;
}

function parseCron(expr: string): ParsedCron | null {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5 || !/^\d+$/.test(parts[0]) || !/^(\d+|\*)$/.test(parts[1])) return null;
  return { minute: parts[0], hour: parts[1], dayOfMonth: parts[2], month: parts[3], dayOfWeek: parts[4] };
}

function encodeModel(provider: string | null, model: string | null): string {
  return provider && model ? JSON.stringify([provider, model]) : '';
}

function decodeModel(value: string): { provider: string | null; model: string | null } {
  if (!value) return { provider: null, model: null };
  try {
    const [provider, model] = JSON.parse(value) as [string, string];
    return { provider, model };
  } catch {
    return { provider: null, model: null };
  }
}

export function taskRule(job: HermesCronJobInfo): string {
  const spec = job.scheduleSpec ?? {};
  if (job.scheduleKind === 'interval') {
    const minutes = Number(spec.minutes ?? 0);
    if (minutes > 0 && minutes % 1440 === 0) return `每 ${minutes / 1440} 天`;
    if (minutes > 0 && minutes % 60 === 0) return `每 ${minutes / 60} 小时`;
    if (minutes > 0) return `每 ${minutes} 分钟`;
  }
  if (job.scheduleKind === 'once') {
    const runAt = typeof spec.run_at === 'string' ? spec.run_at : job.nextRunAt;
    return `执行一次 · ${formatDate(runAt)}`;
  }
  if (job.scheduleKind === 'cron' && typeof spec.expr === 'string') {
    const parsed = parseCron(spec.expr);
    if (parsed) {
      if (parsed.hour === '*' && parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') {
        return parsed.minute === '0' ? '每小时整点' : `每小时 ${parsed.minute.padStart(2, '0')} 分`;
      }
      if (parsed.hour === '*') return '自定义周期';
      const time = `${parsed.hour.padStart(2, '0')}:${parsed.minute.padStart(2, '0')}`;
      if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') return `每天 ${time}`;
      if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '1-5') return `工作日 ${time}`;
      if (WEEKDAYS[parsed.dayOfWeek]) return `每${WEEKDAYS[parsed.dayOfWeek]} ${time}`;
      if (/^\d+$/.test(parsed.dayOfMonth) && parsed.month === '*' && parsed.dayOfWeek === '*') return `每月 ${parsed.dayOfMonth} 日 ${time}`;
    }
  }
  if (/^every \d+m$/i.test(job.schedule)) {
    const minutes = Number(job.schedule.match(/\d+/)?.[0] ?? 0);
    return minutes % 60 === 0 ? `每 ${minutes / 60} 小时` : `每 ${minutes} 分钟`;
  }
  return '自定义周期';
}

function effectiveRule(job: HermesCronJobInfo): string {
  if (job.status === 'paused') return '已暂停';
  if (job.status === 'completed') return '已结束';
  if (job.status === 'running') return '正在执行';
  if (job.nextRunAt) return `下次 ${formatDate(job.nextRunAt)}`;
  return '持续生效';
}

export function editorFromJob(job: HermesCronJobInfo): EditorState {
  const editor = { ...EMPTY_EDITOR, name: job.name, prompt: job.prompt, projectId: job.projectId ?? '', skills: job.skills, modelValue: encodeModel(job.provider, job.model) };
  const spec = job.scheduleSpec ?? {};
  if (job.scheduleKind === 'interval') {
    const minutes = Number(spec.minutes ?? 60);
    if (minutes % 1440 === 0) return { ...editor, frequency: 'interval', intervalValue: String(minutes / 1440), intervalUnit: 'd' };
    if (minutes % 60 === 0) return { ...editor, frequency: 'interval', intervalValue: String(minutes / 60), intervalUnit: 'h' };
    return { ...editor, frequency: 'interval', intervalValue: String(minutes), intervalUnit: 'm' };
  }
  if (job.scheduleKind === 'once') {
    const value = typeof spec.run_at === 'string' ? spec.run_at : job.nextRunAt;
    const date = value ? new Date(value) : null;
    const onceAt = date && !Number.isNaN(date.getTime())
      ? new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16)
      : '';
    return { ...editor, frequency: 'once', onceAt };
  }
  if (job.scheduleKind === 'cron' && typeof spec.expr === 'string') {
    const parsed = parseCron(spec.expr);
    if (parsed) {
      if (parsed.hour === '*' && parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') return { ...editor, frequency: 'hourly' };
      if (parsed.hour === '*') return { ...editor, frequency: 'custom', customSchedule: spec.expr };
      const time = `${parsed.hour.padStart(2, '0')}:${parsed.minute.padStart(2, '0')}`;
      if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') return { ...editor, frequency: 'daily', time };
      if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '1-5') return { ...editor, frequency: 'weekdays', time };
      if (WEEKDAYS[parsed.dayOfWeek]) return { ...editor, frequency: 'weekly', time, weekday: parsed.dayOfWeek };
    }
    return { ...editor, frequency: 'custom', customSchedule: spec.expr };
  }
  return { ...editor, frequency: 'custom', customSchedule: job.schedule };
}

export function editorFromDraft(draft: HermesCronDraft): EditorState {
  const editor = {
    ...EMPTY_EDITOR,
    name: draft.name,
    prompt: draft.prompt,
    projectId: draft.projectId ?? '',
    skills: [...draft.skills],
    modelValue: encodeModel(draft.provider, draft.model),
  };
  const parsed = parseCron(draft.schedule);
  if (!parsed) return { ...editor, frequency: 'custom', customSchedule: draft.schedule };
  if (parsed.hour === '*' && parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') {
    return parsed.minute === '0'
      ? { ...editor, frequency: 'hourly' }
      : { ...editor, frequency: 'custom', customSchedule: draft.schedule };
  }
  if (parsed.hour === '*') return { ...editor, frequency: 'custom', customSchedule: draft.schedule };
  const time = `${parsed.hour.padStart(2, '0')}:${parsed.minute.padStart(2, '0')}`;
  if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '*') {
    return { ...editor, frequency: 'daily', time };
  }
  if (parsed.dayOfMonth === '*' && parsed.month === '*' && parsed.dayOfWeek === '1-5') {
    return { ...editor, frequency: 'weekdays', time };
  }
  if (WEEKDAYS[parsed.dayOfWeek]) {
    return { ...editor, frequency: 'weekly', time, weekday: parsed.dayOfWeek };
  }
  return { ...editor, frequency: 'custom', customSchedule: draft.schedule };
}

export function taskHasConfiguredModel(job: HermesCronJobInfo): boolean {
  return Boolean(job.provider?.trim() && job.model?.trim());
}

export function scheduleFromEditor(editor: EditorState): string {
  const [hour = '9', minute = '0'] = editor.time.split(':');
  if (editor.frequency === 'hourly') return '0 * * * *';
  if (editor.frequency === 'daily') return `${Number(minute)} ${Number(hour)} * * *`;
  if (editor.frequency === 'weekdays') return `${Number(minute)} ${Number(hour)} * * 1-5`;
  if (editor.frequency === 'weekly') return `${Number(minute)} ${Number(hour)} * * ${editor.weekday}`;
  if (editor.frequency === 'interval') return `every ${Math.max(1, Number(editor.intervalValue) || 1)}${editor.intervalUnit}`;
  if (editor.frequency === 'once') return editor.onceAt;
  return editor.customSchedule.trim();
}

export function editorRule(editor: EditorState): string {
  if (editor.frequency === 'hourly') return '每小时整点执行';
  if (editor.frequency === 'daily') return `每天 ${editor.time || '09:00'}`;
  if (editor.frequency === 'weekdays') return `每个工作日 ${editor.time || '09:00'}`;
  if (editor.frequency === 'weekly') return `每${WEEKDAYS[editor.weekday] ?? '周一'} ${editor.time || '09:00'}`;
  if (editor.frequency === 'interval') return `每 ${Math.max(1, Number(editor.intervalValue) || 1)} ${editor.intervalUnit === 'm' ? '分钟' : editor.intervalUnit === 'h' ? '小时' : '天'}`;
  if (editor.frequency === 'once') return editor.onceAt ? `执行一次 · ${formatDate(editor.onceAt)}` : '选择执行时间';
  return editor.customSchedule.trim() ? '高级自定义计划' : '填写 Hermes 计划表达式';
}

function runStatus(status: HermesCronRunStatus): { label: string; color: string; pill: string } {
  if (status === 'running') return { label: '执行中', color: 'text-[var(--accent)]', pill: 'bg-[var(--accent-subtle)] text-[var(--accent)]' };
  if (status === 'completed') return { label: '成功', color: 'text-[var(--success)]', pill: 'bg-[var(--success-subtle)] text-[var(--success)]' };
  if (status === 'error') return { label: '失败', color: 'text-[var(--danger)]', pill: 'bg-[var(--danger-subtle)] text-[var(--danger)]' };
  return { label: '等待中', color: 'text-[var(--text-tertiary)]', pill: 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]' };
}

export function hasActiveCronRun(runs: HermesCronRunInfo[]): boolean {
  return runs.some((run) => run.status === 'running' || run.status === 'pending');
}

const CRON_STARTUP_STALL_SECONDS = 120;

export function isCronRunStartupStalled(
  run: HermesCronRunInfo,
  nowEpochSeconds = Date.now() / 1000,
): boolean {
  return (
    (run.status === 'running' || run.status === 'pending')
    && run.startedAt !== null
    && nowEpochSeconds - run.startedAt >= CRON_STARTUP_STALL_SECONDS
    && run.modelCallCount === 0
    && run.toolCallCount === 0
  );
}

const wait = (milliseconds: number) => new Promise<void>((resolve) => {
  window.setTimeout(resolve, milliseconds);
});

/**
 * 错误信息是否属于 Hermes 连接断开类（端点未配置 / 连接失败 / 重启中）。
 * 调用方可以在拿到这类错误后触发 Hermes Runtime 自动重启作为兜底。
 */
export function isHermesDisconnectedError(message: string): boolean {
  const normalized = message.toLowerCase();
  return (
    normalized.includes('hermes agent 未连接') ||
    normalized.includes('hermes 未连接') ||
    normalized.includes('hermes 未就绪') ||
    normalized.includes('endpoint not configured') ||
    normalized.includes('connection refused') ||
    normalized.includes('websocket') ||
    normalized.includes('hermes-sidecar') ||
    normalized.includes('侧载进程已退出') ||
    normalized.includes('health status')
  );
}

function TaskEditor({
  job,
  example,
  projects,
  modelOptions,
  onClose,
  onSaved,
  onDeleted,
}: {
  job: HermesCronJobInfo | null;
  example: ScheduledTaskExample | null;
  projects: Project[];
  modelOptions: HermesModelOptions | null;
  onClose: () => void;
  onSaved: (job: HermesCronJobInfo) => void;
  onDeleted: (job: HermesCronJobInfo) => void;
}) {
  const [editor, setEditor] = useState<EditorState>(() => {
    if (job) return editorFromJob(job);
    if (example) return editorFromDraft(scheduledTaskExampleDraft(example));
    return { ...EMPTY_EDITOR };
  });
  const [runs, setRuns] = useState<HermesCronRunInfo[]>([]);
  const [selectedRun, setSelectedRun] = useState<HermesCronRunInfo | null>(null);
  const [runResult, setRunResult] = useState<HermesCronRunResult | null>(null);
  const [runResultError, setRunResultError] = useState('');
  const [historyLoading, setHistoryLoading] = useState(Boolean(job));
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const mountedRef = useRef(true);
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone || '本地时区';

  const loadRuns = useCallback(async (quiet = false): Promise<HermesCronRunInfo[]> => {
    if (!job) return [];
    if (!quiet) setHistoryLoading(true);
    try {
      const nextRuns = await hermesCronRuns(job);
      if (mountedRef.current) setRuns(nextRuns);
      return nextRuns;
    } catch (cause) {
      if (mountedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
      return [];
    } finally {
      if (!quiet && mountedRef.current) setHistoryLoading(false);
    }
  }, [job]);

  useEffect(() => { void loadRuns(); }, [loadRuns]);
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const activeRun = hasActiveCronRun(runs);

  useEffect(() => {
    if (!job || !activeRun || busy === 'trigger') return;
    const timer = window.setInterval(() => void loadRuns(true), 2_500);
    return () => window.clearInterval(timer);
  }, [activeRun, busy, job, loadRuns]);

  const save = async () => {
    setError('');
    const modelSelection = decodeModel(editor.modelValue);
    const draft: HermesCronDraft = {
      name: editor.name.trim(),
      prompt: editor.prompt.trim(),
      projectId: editor.projectId || null,
      skills: editor.skills,
      schedule: scheduleFromEditor(editor),
      provider: modelSelection.provider,
      model: modelSelection.model,
      startPaused: Boolean(example?.startPaused),
    };
    if (!draft.name || !draft.prompt || !draft.schedule) {
      setError('请填写任务名称、任务内容和完整的执行时间。');
      return;
    }
    setBusy('save');
    try {
      const saved = job ? await hermesCronUpdate(job, draft) : await hermesCronCreate(draft);
      onSaved(saved);
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const trigger = async () => {
    if (!job || activeRun || busy === 'trigger') return;
    setBusy('trigger');
    setError('');
    try {
      const knownSessionIds = new Set(runs.map((run) => run.sessionId));
      const updated = await hermesCronTrigger(job);
      onSaved(updated);
      let observedRun: HermesCronRunInfo | undefined;
      for (let attempt = 0; attempt < 300 && mountedRef.current; attempt += 1) {
        const nextRuns = await loadRuns(true);
        observedRun = nextRuns.find((run) => !knownSessionIds.has(run.sessionId))
          ?? nextRuns.find((run) => run.status === 'running');
        if (observedRun && (observedRun.status === 'completed' || observedRun.status === 'error')) {
          break;
        }
        await wait(2_000);
      }
      if (mountedRef.current && (!observedRun || observedRun.status === 'running' || observedRun.status === 'pending')) {
        setError('任务仍在 Hermes 中执行，运行完成前不会再次触发。可在运行历史中查看最新状态。');
      }
    } catch (cause) {
      if (mountedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (mountedRef.current) setBusy('');
    }
  };

  const toggle = async () => {
    if (!job) return;
    setBusy('toggle');
    setError('');
    try {
      onSaved(await hermesCronSetEnabled(job, !job.enabled));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const remove = async () => {
    if (!job) return;
    const confirmed = await confirmDialog(`删除计划任务“${job.name}”？运行历史也将由 Hermes 一并清理。`, {
      title: '删除计划任务', kind: 'warning', okLabel: '删除', cancelLabel: '取消',
    });
    if (!confirmed) return;
    setBusy('delete');
    try {
      await hermesCronDelete(job);
      onDeleted(job);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const showResult = async (run: HermesCronRunInfo) => {
    setSelectedRun(run);
    setRunResult(null);
    setRunResultError('');
    try {
      setRunResult(await hermesCronRunResult(run));
    } catch (cause) {
      setRunResultError(`读取结果失败：${cause instanceof Error ? cause.message : String(cause)}`);
    }
  };

  const displayedRun = selectedRun
    ? runs.find((run) => run.sessionId === selectedRun.sessionId) ?? selectedRun
    : null;

  useEffect(() => {
    if (!displayedRun || (displayedRun.status !== 'running' && displayedRun.status !== 'pending')) return;
    const timer = window.setInterval(() => {
      void hermesCronRunResult(displayedRun).then((result) => {
        if (mountedRef.current) setRunResult(result);
      }).catch((cause) => {
        if (mountedRef.current) setRunResultError(`读取执行链路失败：${cause instanceof Error ? cause.message : String(cause)}`);
      });
    }, 2_000);
    return () => window.clearInterval(timer);
  }, [displayedRun]);

  const selectableProviders = modelOptions?.providers.filter((provider) => provider.authenticated === true && provider.models.length > 0) ?? [];
  const currentModelAvailable = !editor.modelValue || selectableProviders.some((provider) => provider.models.some((model) => encodeModel(provider.slug, model) === editor.modelValue));
  const jobHasModel = job ? taskHasConfiguredModel(job) : false;
  const jobModelRunnable = jobHasModel && (modelOptions === null || currentModelAvailable);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-[var(--overlay-scrim)] p-6 backdrop-blur-[1px]" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="flex max-h-[88vh] w-full max-w-6xl flex-col overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]" style={{ boxShadow: 'var(--shadow-lg)' }}>
        <header className="flex h-16 shrink-0 items-center justify-between border-b border-[var(--border-default)] px-6">
          <div className="min-w-0">
            <h2 className="truncate text-lg font-semibold text-[var(--text-primary)]">{job ? job.name : example ? `使用范例：${example.name}` : '添加计划任务'}</h2>
            {job && <p className="mt-0.5 text-xs text-[var(--text-tertiary)]">{taskRule(job)} · {effectiveRule(job)}</p>}
          </div>
          <div className="flex items-center gap-2">
            {job && (
              <>
                <button type="button" onClick={() => void trigger()} disabled={Boolean(busy) || activeRun || !jobModelRunnable} aria-busy={busy === 'trigger' || activeRun} title={!jobModelRunnable ? '先在运行设置中选择并保存可用模型' : undefined} className="inline-flex h-9 items-center gap-1.5 rounded-lg border border-[var(--border-strong)] px-3 text-sm text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)] disabled:cursor-not-allowed disabled:opacity-60">
                  {busy === 'trigger' || activeRun ? <Loader2 size={15} className="animate-spin" /> : <Play size={15} />} {busy === 'trigger' || activeRun ? '运行中' : '立即运行'}
                </button>
                <button type="button" onClick={() => void remove()} disabled={Boolean(busy)} className="grid h-9 w-9 place-items-center rounded-lg border border-[var(--border-strong)] text-[var(--text-tertiary)] transition-colors hover:border-[var(--danger)] hover:bg-[var(--danger-subtle)] hover:text-[var(--danger)] disabled:opacity-50" aria-label="删除计划任务"><Trash2 size={16} /></button>
              </>
            )}
            <button type="button" onClick={onClose} className="grid h-9 w-9 place-items-center rounded-lg text-[var(--text-tertiary)] transition-colors hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]" aria-label="关闭"><X size={19} /></button>
          </div>
        </header>

        <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)] lg:grid-cols-[minmax(0,1fr)_340px]">
          <div className="min-h-0 overflow-y-auto p-6">
            <div className="space-y-5">
              {example && (
                <div className="rounded-xl border border-[var(--accent-border)] bg-[var(--accent-subtle)] px-4 py-3 text-sm leading-6 text-[var(--text-secondary)]">
                  <p className="font-medium text-[var(--text-primary)]">公开脱敏范例</p>
                  <p>{example.description} 保存后默认暂停；配置模型后可点“立即运行”单次执行，只有手动启用后才会按计划运行。</p>
                </div>
              )}
              <label className="block">
                <span className="mb-2 block text-[13px] font-medium text-[var(--text-secondary)]">名称</span>
                <input value={editor.name} onChange={(event) => setEditor((value) => ({ ...value, name: event.target.value }))} placeholder="例如：每日模型新闻" className="input" />
              </label>
              <label className="block">
                <span className="mb-2 block text-[13px] font-medium text-[var(--text-secondary)]">任务内容</span>
                <textarea value={editor.prompt} onChange={(event) => setEditor((value) => ({ ...value, prompt: event.target.value }))} placeholder="告诉 Hermes 每次触发时要完成什么…" rows={7} className="input resize-none leading-6" />
              </label>
              <div>
                <div className="mb-2 flex items-center justify-between">
                  <span className="text-[13px] font-medium text-[var(--text-secondary)]">执行计划</span>
                  <span className="text-xs text-[var(--text-tertiary)]">本地时间 · {timezone}</span>
                </div>
                <div className="rounded-xl border border-[var(--border-default)] p-3">
                  <div className="flex min-h-10 flex-wrap items-center gap-3">
                    <select value={editor.frequency} onChange={(event) => setEditor((state) => ({ ...state, frequency: event.target.value as Frequency }))} className="h-10 min-w-32 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm font-medium text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]">
                      <option value="hourly">每小时</option>
                      <option value="daily">每天</option>
                      <option value="weekdays">工作日</option>
                      <option value="weekly">每周</option>
                      <option value="interval">按间隔</option>
                      <option value="once">单次执行</option>
                      <option value="custom">高级计划</option>
                    </select>
                    {editor.frequency === 'hourly' && <span className="text-sm text-[var(--text-tertiary)]">每小时整点</span>}
                    {(editor.frequency === 'daily' || editor.frequency === 'weekdays') && <input type="time" value={editor.time} onChange={(event) => setEditor((state) => ({ ...state, time: event.target.value }))} className="h-10 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />}
                    {editor.frequency === 'weekly' && <><select value={editor.weekday} onChange={(event) => setEditor((state) => ({ ...state, weekday: event.target.value }))} className="h-10 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]">{Object.entries(WEEKDAYS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select><input type="time" value={editor.time} onChange={(event) => setEditor((state) => ({ ...state, time: event.target.value }))} className="h-10 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" /></>}
                    {editor.frequency === 'interval' && <><span className="text-sm text-[var(--text-tertiary)]">每</span><input type="number" min="1" value={editor.intervalValue} onChange={(event) => setEditor((state) => ({ ...state, intervalValue: event.target.value }))} className="h-10 w-24 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" /><select value={editor.intervalUnit} onChange={(event) => setEditor((state) => ({ ...state, intervalUnit: event.target.value as EditorState['intervalUnit'] }))} className="h-10 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"><option value="m">分钟</option><option value="h">小时</option><option value="d">天</option></select></>}
                    {editor.frequency === 'once' && <input type="datetime-local" value={editor.onceAt} onChange={(event) => setEditor((state) => ({ ...state, onceAt: event.target.value }))} className="h-10 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />}
                    {editor.frequency === 'custom' && <input value={editor.customSchedule} onChange={(event) => setEditor((state) => ({ ...state, customSchedule: event.target.value }))} placeholder="例如 every 90m 或 0 9 * * 1-5" className="h-10 min-w-64 flex-1 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 font-mono text-[13px] text-[var(--text-primary)] outline-none transition-shadow focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />}
                  </div>
                  <p className="mt-2 text-xs text-[var(--text-tertiary)]">{editorRule(editor)}</p>
                </div>
              </div>

              <div>
                <span className="mb-2 block text-[13px] font-medium text-[var(--text-secondary)]">运行设置</span>
                <div className="grid gap-3 rounded-xl border border-[var(--border-default)] p-3 sm:grid-cols-2">
                  <label className="block">
                    <span className="mb-1.5 block text-xs text-[var(--text-tertiary)]">所属项目</span>
                    <select value={editor.projectId} onChange={(event) => setEditor((value) => ({ ...value, projectId: event.target.value }))} className="input h-10">
                      <option value="">不关联项目</option>
                      {projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
                    </select>
                  </label>
                  <label className="block">
                    <span className="mb-1.5 block text-xs text-[var(--text-tertiary)]">模型</span>
                    <select value={editor.modelValue} onChange={(event) => setEditor((value) => ({ ...value, modelValue: event.target.value }))} className="input h-10">
                      <option value="">未配置（保存后自动暂停）</option>
                      {!currentModelAvailable && <option value={editor.modelValue}>当前任务模型（配置已不可用）</option>}
                      {selectableProviders.map((provider) => <optgroup key={provider.slug} label={provider.name}>{provider.models.map((model) => <option key={`${provider.slug}:${model}`} value={encodeModel(provider.slug, model)}>{model}</option>)}</optgroup>)}
                    </select>
                  </label>
                </div>
                {!editor.modelValue && <p className="mt-2 text-xs text-[var(--warning)]">未选择模型。任务会保留，但保存后保持暂停，选择模型后才能启用或立即运行。</p>}
              </div>

              {job && (
                <div className="rounded-xl border border-[var(--border-default)] px-4 py-3">
                  <div className="flex items-center justify-between">
                    <div>
                      <div className="flex items-center gap-2">
                        <p className="text-[13px] font-medium text-[var(--text-secondary)]">任务状态</p>
                        <span className={`inline-flex items-center rounded-full px-2 py-0.5 text-xs font-semibold ${job.enabled ? 'bg-[var(--success-subtle)] text-[var(--success)]' : 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]'}`}>{job.enabled ? '已启用' : '已停用'}</span>
                      </div>
                      <p className="mt-0.5 text-xs text-[var(--text-tertiary)]">暂停后 Hermes 不再按计划触发</p>
                    </div>
                    <button type="button" onClick={() => void toggle()} disabled={Boolean(busy) || (!job.enabled && !jobModelRunnable)} title={!job.enabled && !jobModelRunnable ? '先选择并保存可用模型' : undefined} className={`relative h-6 w-11 rounded-full transition-colors ${job.enabled ? 'bg-[var(--success)]' : 'bg-[var(--border-strong)]'}`} aria-label={job.enabled ? '暂停任务' : '启用任务'}><span className={`absolute top-1 h-4 w-4 rounded-full bg-white shadow transition-transform ${job.enabled ? 'left-6' : 'left-1'}`} /></button>
                  </div>
                  <div className="mt-3 grid grid-cols-2 gap-3 border-t border-[var(--border-default)] pt-3 text-xs"><div><span className="text-[var(--text-tertiary)]">创建时间</span><span className="ml-2 text-[var(--text-secondary)]">{formatDate(job.createdAt)}</span></div><div><span className="text-[var(--text-tertiary)]">上次运行</span><span className="ml-2 text-[var(--text-secondary)]">{formatDate(job.lastRunAt)}</span></div></div>
                </div>
              )}
              {job?.lastError && <p className="flex items-start gap-2 rounded-lg bg-[var(--danger-subtle)] p-3 text-xs leading-5 text-[var(--danger)]"><AlertCircle size={15} className="mt-0.5 shrink-0" />{job.lastError}</p>}
              {error && <p className="flex items-start gap-2 rounded-lg bg-[var(--danger-subtle)] p-3 text-sm text-[var(--danger)]"><AlertCircle size={16} className="mt-0.5 shrink-0" />{error}</p>}
            </div>
          </div>

          <aside className="flex min-h-0 flex-col border-l border-[var(--border-default)] bg-[var(--bg-sunken)]">
            <div className="flex h-12 items-center justify-between border-b border-[var(--border-default)] px-4">
              <span className="flex items-center gap-2 text-sm font-medium text-[var(--text-secondary)]"><History size={15} />运行历史{job ? ` (${runs.length})` : ''}</span>
              {job && <button type="button" onClick={() => void loadRuns()} className="text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-secondary)]" aria-label="刷新运行历史"><RefreshCw size={14} /></button>}
            </div>
            {!job ? <div className="p-6 text-center text-xs leading-5 text-[var(--text-tertiary)]">保存任务后，这里会显示每次运行状态与结果。</div> : historyLoading ? <div className="flex justify-center p-8"><Loader2 size={18} className="animate-spin text-[var(--text-tertiary)]" /></div> : runs.length === 0 ? <div className="p-6 text-center text-xs text-[var(--text-tertiary)]">尚无运行记录</div> : (
              <div className="min-h-0 flex-1 overflow-y-auto p-2">
                {runs.map((run) => {
                  const startupStalled = isCronRunStartupStalled(run);
                  const meta = startupStalled
                    ? { label: '启动异常', color: 'text-[var(--warning)]', pill: 'bg-[var(--warning-subtle)] text-[var(--warning)]' }
                    : runStatus(run.status);
                  const duration = formatDuration(run.startedAt, run.endedAt);
                  return <button key={run.sessionId} type="button" onClick={() => void showResult(run)} className="mb-1 flex w-full items-center justify-between rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-[var(--bg-surface)]"><div className="min-w-0"><p className="text-xs font-medium text-[var(--text-secondary)]">{formatEpoch(run.startedAt)}</p><p className="mt-1.5 flex items-center gap-1.5"><span className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-semibold ${meta.pill}`}>{(run.status === 'error' || startupStalled) && <AlertCircle size={12} />}{meta.label}</span>{duration && <span className="text-xs text-[var(--text-tertiary)]">{duration}</span>}</p>{startupStalled ? <p className="mt-1 max-w-64 truncate text-xs text-[var(--warning)]">首轮模型请求长时间未返回</p> : run.status === 'running' && <p className="mt-1 max-w-64 truncate text-xs text-[var(--text-tertiary)]">正在等待 Hermes 完成本轮执行</p>}</div><ChevronRight size={14} className="shrink-0 text-[var(--text-disabled)]" /></button>;
                })}
              </div>
            )}
          </aside>
        </div>

        <footer className="flex h-16 shrink-0 items-center justify-end gap-2 border-t border-[var(--border-default)] px-6">
          <button type="button" onClick={onClose} className="btn-secondary">取消</button>
          <button type="button" onClick={() => void save()} disabled={Boolean(busy)} className="btn-primary inline-flex items-center gap-1.5 disabled:opacity-50">{busy === 'save' ? <Loader2 size={15} className="animate-spin" /> : <Check size={15} />}保存</button>
        </footer>
      </section>

      {displayedRun && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-[var(--overlay-scrim-strong)] p-8" onMouseDown={(event) => event.target === event.currentTarget && setSelectedRun(null)}>
          <section className="flex max-h-[84vh] w-full max-w-5xl flex-col overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)]" style={{ boxShadow: 'var(--shadow-lg)' }}>
            <header className="flex h-16 shrink-0 items-center justify-between border-b border-[var(--border-default)] px-6"><div><p className="text-base font-semibold text-[var(--text-primary)]">运行详情</p><p className="mt-0.5 text-xs text-[var(--text-tertiary)]">{formatEpoch(displayedRun.startedAt)}{formatDuration(displayedRun.startedAt, displayedRun.endedAt) ? ` · ${formatDuration(displayedRun.startedAt, displayedRun.endedAt)}` : ''}</p></div><button type="button" onClick={() => setSelectedRun(null)} className="grid h-8 w-8 place-items-center rounded-lg text-[var(--text-tertiary)] transition-colors hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]"><X size={17} /></button></header>
            <div className="overflow-y-auto p-6">
              {isCronRunStartupStalled(displayedRun) && (
                <p className="mb-4 flex items-start gap-2 rounded-xl bg-[var(--warning-subtle)] px-4 py-3 text-sm leading-6 text-[var(--warning)]">
                  <AlertCircle size={16} className="mt-1 shrink-0" />
                  首轮模型请求超过 2 分钟仍未返回，且尚无工具调用。当前不是 OpenRouter 数据抓取阶段，请检查模型服务或稍后重试。
                </p>
              )}
              {runResultError ? (
                <p className="flex items-start gap-2 rounded-lg bg-[var(--danger-subtle)] p-3 text-sm text-[var(--danger)]"><AlertCircle size={16} className="mt-0.5 shrink-0" />{runResultError}</p>
              ) : !runResult ? (
                <span className="flex items-center gap-2 text-sm text-[var(--text-tertiary)]"><Loader2 size={15} className="animate-spin" />正在读取运行详情…</span>
              ) : (
                <div className="space-y-7">
                  <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                    <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-sunken)] p-3"><p className="text-xs text-[var(--text-tertiary)]">运行状态</p><p className={`mt-1 text-sm font-medium ${runStatus(displayedRun.status).color}`}>{runStatus(displayedRun.status).label}</p></div>
                    <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-sunken)] p-3"><p className="text-xs text-[var(--text-tertiary)]">使用模型</p><p className="mt-1 truncate text-sm font-medium text-[var(--text-secondary)]" title={displayedRun.model ?? ''}>{displayedRun.model ?? '未配置'}</p></div>
                    <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-sunken)] p-3"><p className="text-xs text-[var(--text-tertiary)]">模型调用</p><p className="mt-1 text-sm font-medium text-[var(--text-secondary)]">{displayedRun.modelCallCount} 次</p></div>
                    <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-sunken)] p-3"><p className="text-xs text-[var(--text-tertiary)]">工具调用</p><p className="mt-1 text-sm font-medium text-[var(--text-secondary)]">{displayedRun.toolCallCount} 次</p></div>
                  </div>
                  {displayedRun.status === 'error' && displayedRun.endReason && (
                    <p className="flex items-start gap-2 rounded-xl bg-[var(--danger-subtle)] px-4 py-3 text-sm leading-6 text-[var(--danger)]">
                      <AlertCircle size={16} className="mt-1 shrink-0" />
                      {displayedRun.endReason}
                    </p>
                  )}

                  <section>
                    <div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold text-[var(--text-primary)]">执行链路</h3><span className="text-xs text-[var(--text-tertiary)]">{runResult.steps.length} 个业务步骤</span></div>
                    {runResult.steps.length === 0 ? <p className="rounded-xl border border-dashed border-[var(--border-default)] p-4 text-sm text-[var(--text-tertiary)]">本次运行没有产生可识别的工具调用。</p> : (
                      <ol className="space-y-2">
                        {runResult.steps.map((step) => (
                          <li key={`${step.index}-${step.toolName}`} className="rounded-xl border border-[var(--border-default)] px-4 py-3">
                            <div className="flex items-start gap-3">
                              <span className={`mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full text-xs font-semibold ${step.status === 'error' ? 'bg-[var(--danger-subtle)] text-[var(--danger)]' : step.status === 'running' ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'bg-[var(--success-subtle)] text-[var(--success)]'}`}>{step.status === 'running' ? <Loader2 size={13} className="animate-spin" /> : step.status === 'error' ? <AlertCircle size={13} /> : <Check size={13} />}</span>
                              <div className="min-w-0 flex-1">
                                <div className="flex flex-wrap items-center gap-2"><span className="hb-chip">{step.phase}</span><p className="text-sm font-medium text-[var(--text-secondary)]">{step.title}</p><span className="text-xs text-[var(--text-tertiary)]">#{step.index}</span></div>
                                <p className="mt-1.5 text-xs leading-5 text-[var(--text-tertiary)]">{step.input}</p>
                                <details className="mt-2 text-xs"><summary className="cursor-pointer select-none text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-secondary)]">查看返回摘要</summary><p className={`mt-2 rounded-lg px-3 py-2 leading-5 ${step.status === 'error' ? 'bg-[var(--danger-subtle)] text-[var(--danger)]' : 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]'}`}>{step.output}</p></details>
                              </div>
                            </div>
                          </li>
                        ))}
                      </ol>
                    )}
                  </section>

                  <section>
                    <h3 className="mb-3 text-sm font-semibold text-[var(--text-primary)]">运行结果</h3>
                    <div className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] px-5 py-4">
                      <Suspense fallback={<p className="whitespace-pre-wrap text-sm text-[var(--text-secondary)]">{runResult.markdown}</p>}>
                        <MarkdownView content={runResult.markdown} className="hb-chat-agent-markdown text-sm text-[var(--text-secondary)]" copySpecialBlocks />
                      </Suspense>
                    </div>
                  </section>
                </div>
              )}
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

export default function ScheduledTasksPanel() {
  const [jobs, setJobs] = useState<HermesCronJobInfo[]>(() => cachedScheduledJobs());
  const [projects, setProjects] = useState<Project[]>([]);
  const [modelOptions, setModelOptions] = useState<HermesModelOptions | null>(null);
  const [query, setQuery] = useState('');
  const [loading, setLoading] = useState(() => !scheduledJobsCacheHydrated());
  const [error, setError] = useState('');
  const [retrying, setRetrying] = useState(false);
  const [editorOpen, setEditorOpen] = useState(false);
  const [selectedJob, setSelectedJob] = useState<HermesCronJobInfo | null>(null);
  const [selectedExample, setSelectedExample] = useState<ScheduledTaskExample | null>(null);
  const [examplesOpen, setExamplesOpen] = useState(false);

  // 自动兜底：检测错误是否为连接问题；若是，尝试重启 Hermes 后再重试一次。
  // 仅在前端重试时触发一次，避免与 health_supervisor 后台自动恢复重复叠加。
  const retryWithHermesRecovery = useCallback(async () => {
    setError('');
    let firstError = '';
    try {
      const nextJobs = await hermesCronJobs();
      setJobs(nextJobs);
      setCachedScheduledJobs(nextJobs);
      setSelectedJob((current) => current ? nextJobs.find((job) => job.id === current.id && job.profile === current.profile) ?? null : null);
      return;
    } catch (cause) {
      firstError = cause instanceof Error ? cause.message : String(cause);
    }
    if (!isHermesDisconnectedError(firstError)) {
      setError(firstError);
      return;
    }
    // 兜底：尝试重启 Hermes 后再加载一次。
    try {
      await restartHermesRuntime();
    } catch (cause) {
      setError(`${firstError}（自动重启失败：${cause instanceof Error ? cause.message : String(cause)}）`);
      return;
    }
    try {
      const nextJobs = await hermesCronJobs();
      setJobs(nextJobs);
      setCachedScheduledJobs(nextJobs);
      setSelectedJob((current) => current ? nextJobs.find((job) => job.id === current.id && job.profile === current.profile) ?? null : null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  const load = useCallback(async (quiet = false) => {
    const showSpinner = !quiet && !scheduledJobsCacheHydrated();
    if (showSpinner) setLoading(true);
    if (!quiet) setError((current) => (current ? '' : current));
    try {
      const nextJobs = await hermesCronJobs();
      setJobs((current) => (JSON.stringify(current) === JSON.stringify(nextJobs) ? current : nextJobs));
      setCachedScheduledJobs(nextJobs);
      if (!quiet) {
        const [nextProjects, nextModelOptions] = await Promise.all([
          projectList(),
          hermesModelOptions().catch(() => null),
        ]);
        setProjects((current) => (JSON.stringify(current) === JSON.stringify(nextProjects) ? current : nextProjects));
        setModelOptions((current) => (
          JSON.stringify(current) === JSON.stringify(nextModelOptions) ? current : nextModelOptions
        ));
      }
      setSelectedJob((current) => current ? nextJobs.find((job) => job.id === current.id && job.profile === current.profile) ?? null : null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (showSpinner) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (shouldFetchScheduledJobsOnMount()) void load(false);
    const timer = globalThis.setInterval(() => void load(true), 15_000);
    return () => globalThis.clearInterval(timer);
  }, [load]);

  const filtered = useMemo(() => {
    const keyword = query.trim().toLocaleLowerCase();
    if (!keyword) return jobs;
    return jobs.filter((job) => [job.name, job.projectName, job.prompt, taskRule(job)].some((value) => value?.toLocaleLowerCase().includes(keyword)));
  }, [jobs, query]);

  const openCreate = (example: ScheduledTaskExample | null = null) => {
    setSelectedJob(null);
    setSelectedExample(example);
    setExamplesOpen(false);
    setEditorOpen(true);
  };
  const openEdit = (job: HermesCronJobInfo) => {
    setSelectedExample(null);
    setSelectedJob(job);
    setEditorOpen(true);
  };
  const upsert = (job: HermesCronJobInfo) => {
    setJobs((current) => [...current.filter((item) => item.id !== job.id || item.profile !== job.profile), job]);
    setSelectedJob(job);
  };

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col">
      <div className="flex items-center gap-3 border-b border-[var(--border-default)] pb-4">
        <label className="relative min-w-0 flex-1">
          <Search size={16} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-[var(--text-tertiary)]" />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索计划任务" className="h-10 w-full rounded-lg border border-[var(--border-strong)] bg-[var(--bg-sunken)] pl-9 pr-3 text-sm outline-none transition-all focus:border-[var(--accent)] focus:bg-[var(--bg-surface)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]" />
        </label>
        <div className="relative shrink-0">
          <button type="button" onClick={() => setExamplesOpen((value) => !value)} className="inline-flex h-10 items-center gap-2 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-3 text-sm font-medium text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-sunken)]"><BookOpenText size={16} />使用范例<ChevronDown size={14} /></button>
          {examplesOpen && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setExamplesOpen(false)} />
              <div className="absolute right-0 top-full z-50 mt-1 w-[360px] overflow-hidden rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] shadow-[var(--shadow-lg)]">
                <div className="border-b border-[var(--border-default)] px-4 py-3">
                  <p className="text-sm font-semibold text-[var(--text-primary)]">计划任务范例</p>
                  <p className="mt-1 text-xs leading-5 text-[var(--text-tertiary)]">来自旧版任务意图的脱敏参考。选取后仍需保存，且默认暂停。</p>
                </div>
                <div className="max-h-[420px] overflow-y-auto py-1">
                  {scheduledTaskExamples.map((example) => (
                    <button key={example.id} type="button" onClick={() => openCreate(example)} className="w-full px-4 py-3 text-left transition-colors hover:bg-[var(--bg-sunken)]">
                      <span className="block text-sm font-medium text-[var(--text-primary)]">{example.name}</span>
                      <span className="mt-1 block text-xs leading-5 text-[var(--text-tertiary)]">{example.description}</span>
                    </button>
                  ))}
                </div>
              </div>
            </>
          )}
        </div>
        <button type="button" onClick={() => openCreate()} className="inline-flex h-10 shrink-0 items-center gap-2 rounded-lg bg-[var(--accent)] px-4 text-sm font-medium text-white transition-colors hover:bg-[var(--accent-strong)]"><Plus size={16} />添加计划任务</button>
      </div>

      {loading ? (
        <div className="flex min-h-72 items-center justify-center gap-2 text-sm text-[var(--text-tertiary)]"><Loader2 size={16} className="animate-spin" />正在读取 Hermes 计划任务…</div>
      ) : error ? (
        <div className="mt-4 flex items-start justify-between gap-3 rounded-lg border border-[var(--danger)] bg-[var(--danger-subtle)] p-3 text-sm text-[var(--danger)]">
          <span className="flex items-start gap-2">
            <AlertCircle size={16} className="mt-0.5 shrink-0" />
            无法读取计划任务：{error}
          </span>
          <button
            type="button"
            onClick={async () => {
              if (retrying) return;
              setRetrying(true);
              try {
                await retryWithHermesRecovery();
              } finally {
                setRetrying(false);
              }
            }}
            disabled={retrying}
            className="inline-flex shrink-0 items-center gap-1 underline disabled:no-underline disabled:opacity-50"
          >
            {retrying ? <Loader2 size={12} className="animate-spin" /> : null}
            {retrying ? '恢复中…' : '重试'}
          </button>
        </div>
      ) : jobs.length === 0 ? (
        <EmptyState
          icon={CalendarClock}
          title="还没有计划任务"
          className="min-h-72"
          action={<div className="flex items-center gap-4"><button type="button" onClick={() => openCreate()} className="text-sm text-[var(--accent)] hover:underline">添加第一个计划任务</button><button type="button" onClick={() => setExamplesOpen(true)} className="text-sm text-[var(--text-secondary)] hover:text-[var(--accent)]">查看公开范例</button></div>}
        />
      ) : filtered.length === 0 ? (
        <EmptyState icon={Search} title="没有匹配的计划任务" className="min-h-56" />
      ) : (
        <div className="divide-y divide-[var(--border-default)]">
          {filtered.map((job) => (
            <button key={`${job.profile}:${job.id}`} type="button" onClick={() => openEdit(job)} className="group flex w-full items-center gap-4 px-1 py-4 text-left transition-colors hover:bg-[var(--bg-sunken)]">
              <div className={`grid h-8 w-8 shrink-0 place-items-center rounded-lg ${job.status === 'error' ? 'bg-[var(--danger-subtle)] text-[var(--danger)]' : job.status === 'paused' ? 'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]' : 'bg-[var(--accent-subtle)] text-[var(--accent)]'}`}>{job.status === 'paused' ? <CirclePause size={16} /> : <Clock3 size={16} />}</div>
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2"><h3 className="truncate text-sm font-semibold text-[var(--text-primary)]">{job.name}</h3>{job.projectName && <span className="shrink-0 text-xs text-[var(--text-tertiary)]">{job.projectName}</span>}</div>
                <p className="mt-1 truncate text-xs text-[var(--text-tertiary)]"><span>{taskRule(job)}</span><span className="mx-2 text-[var(--text-disabled)]">·</span><span className={job.status === 'error' ? 'font-medium text-[var(--danger)]' : ''}>{job.status === 'error' ? '上次执行失败' : effectiveRule(job)}</span></p>
              </div>
              <ChevronRight size={16} className="shrink-0 text-[var(--text-disabled)] transition-transform group-hover:translate-x-0.5 group-hover:text-[var(--text-tertiary)]" />
            </button>
          ))}
        </div>
      )}

      {editorOpen && <TaskEditor job={selectedJob} example={selectedExample} projects={projects} modelOptions={modelOptions} onClose={() => setEditorOpen(false)} onSaved={upsert} onDeleted={(job) => { setJobs((current) => current.filter((item) => item.id !== job.id || item.profile !== job.profile)); setEditorOpen(false); }} />}
    </div>
  );
}
