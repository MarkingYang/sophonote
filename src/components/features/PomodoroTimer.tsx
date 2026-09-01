import { useEffect, useRef, useState } from 'react';
import { CheckCircle2, Pause, Play, RotateCcw, Timer } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import {
  BREAK_MINUTES,
  FOCUS_MINUTES,
  createPomodoroSession,
  formatClock,
} from '../../services/pomodoro';

/**
 * 任务关联番茄钟（DEC-034 / NEXT-063）。
 * - 专注 25 分钟自然走完 → 记录 completed 会话并自动进入 5 分钟休息；
 * - 中途放弃 → 记录实际时长的未完成会话；
 * - 休息结束回到待开始状态。
 * 计时状态是组件本地的：切走页面会重置当前番茄（首版接受，见台账 NEXT-063）。
 */

type Phase = 'idle' | 'focus' | 'break';

export default function PomodoroTimer() {
  const tasks = useAppStore((s) => s.tasks);
  const addPomodoroSession = useAppStore((s) => s.addPomodoroSession);

  const activeTasks = tasks.filter((t) => t.status !== 'done' && t.status !== 'cancelled');

  const [taskId, setTaskId] = useState<string>('');
  const [phase, setPhase] = useState<Phase>('idle');
  const [running, setRunning] = useState(false);
  const [secondsLeft, setSecondsLeft] = useState(FOCUS_MINUTES * 60);

  const startedAtRef = useRef<Date | null>(null);
  const phaseRef = useRef<Phase>(phase);
  phaseRef.current = phase;

  const recordFocusSession = (completed: boolean, endedAt: Date) => {
    const startedAt = startedAtRef.current;
    startedAtRef.current = null;
    if (!startedAt || phaseRef.current !== 'focus') return;
    void addPomodoroSession(
      createPomodoroSession({
        id: `pomo-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        taskId: taskId || undefined,
        plannedMinutes: FOCUS_MINUTES,
        startedAt,
        endedAt,
        completed,
      })
    ).catch((e) => console.error('Failed to record pomodoro session:', e));
  };

  useEffect(() => {
    if (!running) return;
    const timer = setInterval(() => {
      setSecondsLeft((prev) => {
        if (prev > 1) return prev - 1;
        // 归零：由当前阶段决定落库与流转
        if (phaseRef.current === 'focus') {
          recordFocusSession(true, new Date());
          setPhase('break');
          return BREAK_MINUTES * 60;
        }
        setPhase('idle');
        setRunning(false);
        return FOCUS_MINUTES * 60;
      });
    }, 1000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running]);

  const start = () => {
    if (phase === 'idle') {
      startedAtRef.current = new Date();
      setPhase('focus');
      setSecondsLeft(FOCUS_MINUTES * 60);
    }
    setRunning(true);
  };

  const pause = () => setRunning(false);

  const abandon = () => {
    if (phase === 'focus') recordFocusSession(false, new Date());
    setPhase('idle');
    setRunning(false);
    setSecondsLeft(FOCUS_MINUTES * 60);
  };

  const phaseLabel =
    phase === 'focus' ? (running ? '专注中' : '专注 · 已暂停') : phase === 'break' ? '休息中' : '准备开始';
  const phaseColor =
    phase === 'focus' ? 'text-[var(--accent)]' : phase === 'break' ? 'text-[var(--success)]' : 'text-[var(--text-tertiary)]';

  return (
    <div className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-4 flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <Timer size={14} className="text-[var(--accent)]" />
        <h3 className="text-sm font-semibold text-[var(--text-primary)]">番茄钟</h3>
        <span className={`ml-auto text-xs font-medium ${phaseColor}`}>{phaseLabel}</span>
      </div>

      <div className="text-center py-2">
        <div className="text-4xl font-semibold tabular-nums text-[var(--text-primary)] tracking-wide">
          {formatClock(secondsLeft)}
        </div>
        <p className="text-xs text-[var(--text-tertiary)] mt-1">
          专注 {FOCUS_MINUTES} 分钟 · 休息 {BREAK_MINUTES} 分钟
        </p>
      </div>

      <label className="flex flex-col gap-1">
        <span className="text-xs text-[var(--text-tertiary)]">关联任务（可选）</span>
        <select
          value={taskId}
          onChange={(e) => setTaskId(e.target.value)}
          disabled={phase !== 'idle'}
          className="h-8 rounded-md border border-[var(--border-default)] bg-[var(--bg-base)] px-2 text-xs text-[var(--text-primary)] disabled:opacity-60"
        >
          <option value="">不关联任务</option>
          {activeTasks.map((t) => (
            <option key={t.id} value={t.id}>
              {t.title}
            </option>
          ))}
        </select>
      </label>

      <div className="flex items-center gap-2">
        {running ? (
          <button
            onClick={pause}
            className="flex-1 h-8 inline-flex items-center justify-center gap-1.5 rounded-md bg-[var(--accent)] text-white text-xs font-medium hover:opacity-90 transition-opacity"
          >
            <Pause size={13} /> 暂停
          </button>
        ) : (
          <button
            onClick={start}
            className="flex-1 h-8 inline-flex items-center justify-center gap-1.5 rounded-md bg-[var(--accent)] text-white text-xs font-medium hover:opacity-90 transition-opacity"
          >
            <Play size={13} /> {phase === 'idle' ? '开始专注' : '继续'}
          </button>
        )}
        {phase !== 'idle' && (
          <button
            onClick={abandon}
            title={phase === 'focus' ? '放弃本次专注（记录已进行时长）' : '跳过休息'}
            className="h-8 px-3 inline-flex items-center gap-1.5 rounded-md border border-[var(--border-default)] text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] transition-colors"
          >
            <RotateCcw size={13} /> 放弃
          </button>
        )}
      </div>

      {phase === 'idle' && (
        <p className="text-[11px] text-[var(--text-tertiary)] flex items-center gap-1">
          <CheckCircle2 size={11} className="text-[var(--success)]" />
          完成的专注会汇入今日专注统计
        </p>
      )}
    </div>
  );
}
