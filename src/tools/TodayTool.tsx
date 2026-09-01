import { useMemo, useState } from 'react';
import { AlertTriangle, CalendarCheck2, CalendarClock, CheckCircle2, Plus } from 'lucide-react';
import { useAppStore } from '../stores/appStore';
import { classifyTodayTasks } from '../services/todayTasks';
import EmptyState from '../components/ui/EmptyState';
import type { Task } from '../types';

/**
 * 今日工具（DEC-041）：独立整页。逾期 / 今日到期 / 今日完成三段行动清单 + 快速添加。
 * 专注统计归番茄钟工具，本页只管「今天要做什么」。
 */

const priorityLabels = {
  1: { text: '高', color: 'text-[var(--danger)] bg-[var(--danger-subtle)]' },
  2: { text: '中', color: 'text-[var(--warning)] bg-[var(--warning-subtle)]' },
  3: { text: '低', color: 'text-[var(--text-tertiary)] bg-[var(--bg-sunken)]' },
};

function TaskRow({ task, showDue }: { task: Task; showDue?: boolean }) {
  const toggleTask = useAppStore((s) => s.toggleTask);
  const isDone = task.status === 'done';
  const pLabel = priorityLabels[task.priority] || priorityLabels[2];
  return (
    <div className="flex items-center gap-3 px-3 py-2.5 hover:bg-[var(--bg-sunken)] transition-colors">
      <button
        onClick={() => void toggleTask(task.id)}
        className={`shrink-0 w-[18px] h-[18px] rounded-full border-2 flex items-center justify-center transition-colors ${
          isDone
            ? 'bg-[var(--success)] border-[var(--success)] text-white'
            : 'border-[var(--border-strong)] hover:border-[var(--accent)]'
        }`}
        title={isDone ? '标记未完成' : '标记完成'}
      >
        {isDone && <CheckCircle2 size={11} />}
      </button>
      <span
        className={`flex-1 min-w-0 text-sm truncate ${
          isDone ? 'line-through text-[var(--text-tertiary)]' : 'text-[var(--text-primary)]'
        }`}
      >
        {task.title}
      </span>
      <span className={`text-[11px] px-1.5 py-0.5 rounded font-medium shrink-0 ${pLabel.color}`}>{pLabel.text}</span>
      {showDue && task.dueDate && (
        <span className="text-xs text-[var(--text-tertiary)] shrink-0">
          {new Date(task.dueDate).toLocaleDateString('zh-CN')}
        </span>
      )}
    </div>
  );
}

function Section({
  icon: Icon,
  title,
  tone,
  count,
  children,
}: {
  icon: typeof AlertTriangle;
  title: string;
  tone: string;
  count: number;
  children: React.ReactNode;
}) {
  if (count === 0) return null;
  return (
    <section className="mb-5">
      <div className="flex items-center gap-2 mb-2">
        <Icon size={14} className={tone} />
        <h3 className="text-sm font-semibold text-[var(--text-primary)]">{title}</h3>
        <span className="text-xs text-[var(--text-tertiary)]">{count} 项</span>
      </div>
      <div className="rounded-lg border border-[var(--border-default)] bg-[var(--bg-surface)] divide-y divide-[var(--border-default)] overflow-hidden">
        {children}
      </div>
    </section>
  );
}

export default function TodayTool() {
  const tasks = useAppStore((s) => s.tasks);
  const addTask = useAppStore((s) => s.addTask);

  const now = new Date();
  const view = useMemo(() => classifyTodayTasks(tasks, now), [tasks]);

  const [quickTitle, setQuickTitle] = useState('');
  const handleQuickAdd = () => {
    const title = quickTitle.trim();
    if (!title) return;
    const today = new Date();
    today.setHours(23, 59, 59, 0);
    void addTask({
      id: `task-${Date.now()}`,
      title,
      status: 'todo',
      priority: 2,
      dueDate: today.toISOString(),
      createdAt: new Date().toISOString(),
    });
    setQuickTitle('');
  };

  const todayLabel = now.toLocaleDateString('zh-CN', { month: 'long', day: 'numeric', weekday: 'long' });
  const isEmpty = view.overdue.length === 0 && view.dueToday.length === 0 && view.completedToday.length === 0;

  return (
    <div className="h-full overflow-y-auto p-5">
      <div className="max-w-3xl mx-auto">
        {/* 顶部：日期 + 三项统计 */}
        <div className="flex items-center justify-between mb-4">
          <h3 className="text-sm font-semibold text-[var(--text-primary)]">{todayLabel}</h3>
          <div className="flex items-center gap-2 text-xs">
            <span className="px-2 py-1 rounded-md bg-[var(--bg-surface)] border border-[var(--border-default)] text-[var(--text-secondary)]">
              今日到期 <b className="text-[var(--text-primary)]">{view.dueToday.length}</b>
            </span>
            <span className="px-2 py-1 rounded-md bg-[var(--bg-surface)] border border-[var(--border-default)] text-[var(--text-secondary)]">
              逾期{' '}
              <b className={view.overdue.length > 0 ? 'text-[var(--danger)]' : 'text-[var(--text-primary)]'}>
                {view.overdue.length}
              </b>
            </span>
            <span className="px-2 py-1 rounded-md bg-[var(--bg-surface)] border border-[var(--border-default)] text-[var(--text-secondary)]">
              已完成 <b className="text-[var(--success)]">{view.completedToday.length}</b>
            </span>
          </div>
        </div>

        {/* 快速添加今日任务 */}
        <div className="flex gap-2 mb-5">
          <input
            type="text"
            value={quickTitle}
            onChange={(e) => setQuickTitle(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleQuickAdd()}
            placeholder="添加今日任务，回车确认..."
            className="input flex-1"
          />
          <button onClick={handleQuickAdd} className="btn-primary flex items-center gap-1.5">
            <Plus size={14} />
            添加
          </button>
        </div>

        <Section icon={AlertTriangle} title="已逾期" tone="text-[var(--danger)]" count={view.overdue.length}>
          {view.overdue.map((t) => (
            <TaskRow key={t.id} task={t} showDue />
          ))}
        </Section>

        <Section icon={CalendarClock} title="今日到期" tone="text-[var(--accent)]" count={view.dueToday.length}>
          {view.dueToday.map((t) => (
            <TaskRow key={t.id} task={t} />
          ))}
        </Section>

        <Section icon={CalendarCheck2} title="今日完成" tone="text-[var(--success)]" count={view.completedToday.length}>
          {view.completedToday.map((t) => (
            <TaskRow key={t.id} task={t} />
          ))}
        </Section>

        {isEmpty && (
          <EmptyState
            icon={CalendarClock}
            title="今天没有到期任务"
            desc="用上方输入框添加今日任务，或到「任务」工具管理全部待办"
            className="py-16 border border-dashed border-[var(--border-default)] rounded-lg"
          />
        )}
      </div>
    </div>
  );
}
