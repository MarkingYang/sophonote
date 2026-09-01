import { useMemo } from 'react';
import { CheckCircle2, XCircle } from 'lucide-react';
import { useAppStore } from '../stores/appStore';
import { formatClock, summarizeFocus } from '../services/pomodoro';
import { isSameLocalDay } from '../services/todayTasks';
import PomodoroTimer from '../components/features/PomodoroTimer';

/**
 * 番茄钟工具（DEC-041）：独立整页。计时器 + 今日专注统计 + 今日会话记录。
 * 会话关联任务名从 tasks 反查；任务已删除时显示「任务已删除」。
 */
export default function PomodoroTool() {
  const pomodoroSessions = useAppStore((s) => s.pomodoroSessions);
  const tasks = useAppStore((s) => s.tasks);

  const now = new Date();
  const focus = useMemo(() => summarizeFocus(pomodoroSessions, now), [pomodoroSessions]);

  const todaySessions = useMemo(
    () => pomodoroSessions.filter((s) => isSameLocalDay(s.startedAt, now)),
    [pomodoroSessions]
  );

  const taskTitle = (taskId?: string) => {
    if (!taskId) return null;
    return tasks.find((t) => t.id === taskId)?.title ?? '任务已删除';
  };

  return (
    <div className="h-full overflow-y-auto p-5">
      <div className="max-w-md mx-auto flex flex-col gap-4">
        <PomodoroTimer />

        {/* 今日专注统计 */}
        <div className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] p-4">
          <h4 className="text-xs font-semibold text-[var(--text-primary)] mb-2">今日专注</h4>
          <p className="text-2xl font-semibold tabular-nums text-[var(--text-primary)]">
            {formatClock(focus.minutes * 60)}
          </p>
          <p className="text-xs text-[var(--text-tertiary)] mt-1">
            完成 {focus.completedCount} 个番茄（单个 25 分钟）
          </p>
        </div>

        {/* 今日会话记录 */}
        <section>
          <div className="flex items-center gap-2 mb-2">
            <h4 className="text-xs font-semibold text-[var(--text-primary)]">今日记录</h4>
            <span className="text-xs text-[var(--text-tertiary)]">{todaySessions.length} 条</span>
          </div>
          {todaySessions.length === 0 ? (
            <p className="text-xs text-[var(--text-tertiary)] py-6 text-center border border-dashed border-[var(--border-default)] rounded-lg">
              今天还没有专注记录，点击上方「开始专注」
            </p>
          ) : (
            <div className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] divide-y divide-[var(--border-default)] overflow-hidden">
              {todaySessions.map((s) => {
                const start = new Date(s.startedAt);
                const title = taskTitle(s.taskId);
                return (
                  <div key={s.id} className="flex items-center gap-3 px-3 py-2.5">
                    {s.completed ? (
                      <CheckCircle2 size={14} className="text-[var(--success)] shrink-0" />
                    ) : (
                      <XCircle size={14} className="text-[var(--text-tertiary)] shrink-0" />
                    )}
                    <div className="flex-1 min-w-0">
                      <p className="text-sm text-[var(--text-primary)] truncate">
                        {title ?? '未关联任务'}
                      </p>
                      <p className="text-xs text-[var(--text-tertiary)]">
                        {start.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}
                        {' · '}
                        {s.completed ? `完整 ${s.plannedMinutes} 分钟` : '中途放弃'}
                      </p>
                    </div>
                    <span
                      className={`text-[11px] px-1.5 py-0.5 rounded font-medium shrink-0 ${
                        s.completed
                          ? 'text-[var(--success)] bg-[var(--success-subtle)]'
                          : 'text-[var(--text-tertiary)] bg-[var(--bg-sunken)]'
                      }`}
                    >
                      {s.completed ? '完成' : '放弃'}
                    </span>
                  </div>
                );
              })}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
