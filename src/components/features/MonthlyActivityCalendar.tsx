import { CalendarDays, ChevronLeft, ChevronRight } from 'lucide-react';
import { activityHeatLevel, monthDateCells } from '../../services/noteActivityCalendar';
import { todayStr } from '../../services/journal';

interface MonthlyActivityCalendarProps {
  month: Date;
  selectedDate: string | null;
  activityCounts: Map<string, number>;
  selectedNoteCount: number;
  onMonthChange: (month: Date) => void;
  onSelectDate: (date: string) => void;
  onClearDate: () => void;
  onToday: () => void;
}

const weekdayLabels = ['一', '二', '三', '四', '五', '六', '日'];
// 活跃度梯度：accent-subtle → accent（color-mix 梯度，颜色统一走 var(--accent)）
const heatClasses = [
  'bg-[var(--bg-sunken)] text-[var(--text-tertiary)]',
  'bg-[color:color-mix(in_srgb,var(--accent)_15%,transparent)] text-[var(--accent)]',
  'bg-[color:color-mix(in_srgb,var(--accent)_35%,transparent)] text-[var(--accent-strong)]',
  'bg-[color:color-mix(in_srgb,var(--accent)_65%,transparent)] text-white',
  'bg-[var(--accent)] text-white',
];

export default function MonthlyActivityCalendar({
  month,
  selectedDate,
  activityCounts,
  selectedNoteCount,
  onMonthChange,
  onSelectDate,
  onClearDate,
  onToday,
}: MonthlyActivityCalendarProps) {
  const year = month.getFullYear();
  const monthIndex = month.getMonth();
  const cells = monthDateCells(year, monthIndex);
  const today = todayStr();
  const monthMaximum = Math.max(
    0,
    ...cells.map((date) => date ? activityCounts.get(todayStr(date)) ?? 0 : 0)
  );
  const changeMonth = (delta: number) => onMonthChange(new Date(year, monthIndex + delta, 1));

  return (
    <section
      className="rounded-[var(--radius-lg)] border border-[var(--border-default)] bg-[var(--bg-surface)] p-2.5 shadow-[var(--shadow-sm)]"
      aria-label="月度编码阅读热力日历"
    >
      <div className="mb-2 flex items-center gap-1">
        <CalendarDays size={13} className="text-[var(--accent)]" />
        <h4 className="flex-1 text-xs font-semibold text-[var(--text-primary)]">
          {year} 年 {monthIndex + 1} 月
        </h4>
        <button
          type="button"
          onClick={() => changeMonth(-1)}
          className="flex h-6 w-6 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]"
          title="上个月"
          aria-label="上个月"
        >
          <ChevronLeft size={13} />
        </button>
        <button
          type="button"
          onClick={() => changeMonth(1)}
          className="flex h-6 w-6 items-center justify-center rounded-md text-[var(--text-tertiary)] hover:bg-[var(--bg-sunken)] hover:text-[var(--text-secondary)]"
          title="下个月"
          aria-label="下个月"
        >
          <ChevronRight size={13} />
        </button>
      </div>

      <div className="mb-1 grid grid-cols-7 gap-1" aria-hidden="true">
        {weekdayLabels.map((label) => (
          <span key={label} className="text-center text-[12px] font-medium text-[var(--text-tertiary)]">{label}</span>
        ))}
      </div>
      <div className="grid grid-cols-7 gap-1">
        {cells.map((date, index) => {
          if (!date) return <span key={`empty-${index}`} className="h-6" aria-hidden="true" />;
          const dateKey = todayStr(date);
          const activityCount = activityCounts.get(dateKey) ?? 0;
          const selected = selectedDate === dateKey;
          const isToday = dateKey === today;
          return (
            <button
              type="button"
              key={dateKey}
              onClick={() => onSelectDate(dateKey)}
              className={`relative h-6 rounded-md text-[12px] font-medium transition-transform hover:scale-105 focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)] ${heatClasses[activityHeatLevel(activityCount, monthMaximum)]} ${
                selected ? 'ring-2 ring-[var(--accent)] ring-offset-1 ring-offset-[var(--bg-surface)]' : ''
              } ${isToday ? 'font-extrabold outline outline-2 -outline-offset-1 outline-[var(--gold)]' : ''}`}
              aria-label={`${dateKey}，${activityCount} 次编码阅读活动`}
              aria-pressed={selected}
              title={`${dateKey} · ${activityCount} 次活动`}
            >
              {date.getDate()}
            </button>
          );
        })}
      </div>

      <div className="mt-2 flex items-center gap-2 border-t border-[var(--border-default)] pt-2">
        <button
          type="button"
          onClick={onToday}
          className="text-[12px] font-medium text-[var(--warning)] hover:text-[var(--gold)]"
        >
          回到今天
        </button>
        <span className="min-w-0 flex-1 truncate text-right text-[12px] text-[var(--text-tertiary)]">
          {selectedDate ? `${selectedDate} · ${selectedNoteCount} 篇` : '全部日期'}
        </span>
        {selectedDate && (
          <button
            type="button"
            onClick={onClearDate}
            className="text-[12px] font-medium text-[var(--text-tertiary)] hover:text-[var(--accent)]"
          >
            显示全部
          </button>
        )}
      </div>
    </section>
  );
}
