import { useEffect, useMemo, useState } from 'react';
import { CalendarDays, CalendarRange, CalendarClock, Loader2 } from 'lucide-react';
import MarkdownView from './MarkdownView';
import * as tauri from '../../services/tauri';

const PERIODS: { id: tauri.DiscoveryReportPeriod; label: string; icon: typeof CalendarDays }[] = [
  { id: 'daily', label: '日报', icon: CalendarDays },
  { id: 'weekly', label: '周报', icon: CalendarRange },
  { id: 'monthly', label: '月报', icon: CalendarClock },
];

export default function DiscoverReport() {
  const [period, setPeriod] = useState<tauri.DiscoveryReportPeriod>('daily');
  const [reports, setReports] = useState<tauri.DiscoveryReportView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    tauri.getDiscoveryReports()
      .then((rows) => { if (alive) setReports(rows); })
      .catch((error) => console.error('Failed to load discovery reports:', error))
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, []);

  const visible = useMemo(() => reports.filter((report) => report.period === period), [reports, period]);
  const selected = visible.find((report) => report.id === selectedId) ?? visible[0] ?? null;

  return (
    <div className="hb-d-section-shell hb-d-report-shell hb-d-report-wide">
      <div className="hb-d-segment">
        {PERIODS.map((item) => (
          <button
            key={item.id}
            onClick={() => { setPeriod(item.id); setSelectedId(null); }}
            className={`hb-d-segment-btn ${period === item.id ? 'is-active' : ''}`}
            title={item.label}
            aria-label={item.label}
          >
            <span className="sr-only">{item.label}</span>
            <item.icon size={14} />
          </button>
        ))}
      </div>

      {loading ? (
        <div className="hb-d-state-wrap py-24 flex justify-center"><Loader2 className="animate-spin" size={18} /></div>
      ) : !selected ? (
        <div className="hb-d-empty-wrap py-24 text-center">
          <CalendarDays size={28} className="mx-auto mb-3 text-[var(--d-ink-faint)]" />
          <p className="text-sm font-bold text-[var(--d-ink)]">暂无{PERIODS.find((item) => item.id === period)?.label}</p>
        </div>
      ) : (
        <div className="hb-d-report-layout">
          <aside className="hb-d-report-index">
            {visible.map((report) => (
              <button
                key={report.id}
                onClick={() => setSelectedId(report.id)}
                className={`hb-d-report-index-item ${selected.id === report.id ? 'is-active' : ''}`}
              >
                <span className="hb-d-report-index-key">{report.periodKey}</span>
                <span className="hb-d-report-index-title">{report.title}</span>
              </button>
            ))}
          </aside>
          <article className="hb-d-report-card min-w-0">
            <div className="hb-d-report-masthead">
              <span>{selected.periodKey}</span>
            </div>
            <MarkdownView content={selected.content} className="md-body" />
          </article>
        </div>
      )}
    </div>
  );
}
