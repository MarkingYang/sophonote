import { useEffect, useMemo, useState } from 'react';
import { Gauge, Trash2, X } from 'lucide-react';
import { useAppStore } from '../../stores/appStore';
import {
  clearFixtureCorpus,
  FIXTURE_DOC_COUNT,
  isFixtureArticle,
  seedFixtureCorpus,
} from '../../services/perfFixture';
import { formatReportMarkdown, runAllScenarios, type PerfReport } from '../../services/perfRunner';

/**
 * NEXT-001 性能夹具面板（DocWorkspace 状态栏「夹具」入口，App 级浮层跨页签常驻）。
 * 工作流：播种 200 篇确定性语料 → 运行全部场景 → 复制台账表格/JSON 登记基线 → 清空语料。
 * 场景定义与统计口径见 services/perfRunner.ts / perfFixture.ts。
 */
export default function PerfFixturePanel() {
  const open = useAppStore((s) => s.perfFixtureOpen);
  const articles = useAppStore((s) => s.articles);
  const [busy, setBusy] = useState<string | null>(null);
  const [progress, setProgress] = useState('');
  const [report, setReport] = useState<PerfReport | null>(null);
  const [notice, setNotice] = useState('');

  const fixtureCount = useMemo(() => articles.filter(isFixtureArticle).length, [articles]);

  useEffect(() => {
    if (!open) return;
    setNotice('');
  }, [open]);

  if (!open) return null;

  const runSeed = async () => {
    setBusy('seed');
    setNotice('');
    try {
      const res = await seedFixtureCorpus(useAppStore.getState().articles);
      await useAppStore.getState().loadArticles();
      setNotice(`语料就绪：新增 ${res.created} / 更新 ${res.updated} / 不变 ${res.unchanged}`);
    } catch (e) {
      setNotice(`播种失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const runClear = async () => {
    setBusy('clear');
    setNotice('');
    try {
      const n = await clearFixtureCorpus(useAppStore.getState().articles);
      await useAppStore.getState().loadArticles();
      setNotice(`已清理 ${n} 篇夹具文档`);
    } catch (e) {
      setNotice(`清理失败：${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const runAll = async () => {
    setBusy('run');
    setNotice('');
    setProgress('准备…');
    try {
      const r = await runAllScenarios(setProgress);
      setReport(r);
      setProgress('');
    } catch (e) {
      setNotice(`场景运行失败：${e instanceof Error ? e.message : String(e)}`);
      setProgress('');
    } finally {
      setBusy(null);
    }
  };

  const copyText = async (text: string, what: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setNotice(`已复制${what}，粘贴到台账即可`);
    } catch {
      setNotice('剪贴板不可用，请手动选择文本复制');
    }
  };

  return (
    <div className="fixed bottom-9 right-3 z-50 w-[26rem] max-h-[80vh] overflow-y-auto rounded-lg border border-border bg-[color-mix(in_srgb,var(--bg-surface)_95%,transparent)] shadow-[var(--shadow-lg)] p-3 font-mono select-none">
      <div className="flex items-center gap-1.5 mb-2">
        <Gauge size={12} className="text-[var(--accent)]" />
        <span className="text-[12px] font-semibold text-[var(--text-primary)] flex-1">
          性能夹具（NEXT-001）
        </span>
        <span className="text-[12px] text-[var(--text-tertiary)]">
          语料 {fixtureCount}/{FIXTURE_DOC_COUNT}
        </span>
        <button
          onClick={() => useAppStore.getState().setPerfFixtureOpen(false)}
          className="text-[var(--text-tertiary)] hover:text-[var(--danger)] transition-colors"
          title="关闭"
        >
          <X size={12} />
        </button>
      </div>

      <div className="flex gap-1.5 mb-2">
        <button
          onClick={() => void runSeed()}
          disabled={busy !== null}
          className="flex-1 rounded border border-border px-2 py-1 text-[12px] text-[var(--text-secondary)] hover:border-[var(--accent)] disabled:opacity-50"
        >
          {busy === 'seed' ? '播种中…' : '播种语料'}
        </button>
        <button
          onClick={() => void runClear()}
          disabled={busy !== null || fixtureCount === 0}
          className="rounded border border-border px-2 py-1 text-[12px] text-[var(--text-tertiary)] hover:border-[var(--danger)] hover:text-[var(--danger)] disabled:opacity-50 flex items-center gap-1"
        >
          <Trash2 size={10} /> 清空
        </button>
      </div>

      <button
        onClick={() => void runAll()}
        disabled={busy !== null}
        className="w-full rounded bg-[var(--accent)] px-2 py-1.5 text-[12px] font-bold text-white hover:bg-[var(--accent-strong)] disabled:opacity-50 mb-2"
      >
        {busy === 'run' ? progress || '运行中…' : '运行全部场景（约 40s，勿操作窗口）'}
      </button>

      {notice && (
        <p className="text-[12px] text-[var(--warning)] mb-2 break-all">{notice}</p>
      )}

      {report && (
        <div className="border-t border-border pt-2">
          <table className="w-full text-[12px] mb-2">
            <thead>
              <tr className="text-[var(--text-tertiary)] text-left">
                <th className="py-0.5 font-normal">场景</th>
                <th className="py-0.5 font-normal text-right">n</th>
                <th className="py-0.5 font-normal text-right">P50</th>
                <th className="py-0.5 font-normal text-right">P95</th>
                <th className="py-0.5 font-normal text-right">max</th>
              </tr>
            </thead>
            <tbody>
              {report.scenarios.map((s) => (
                <tr key={s.id} className="text-[var(--text-secondary)]">
                  <td className="py-0.5 pr-1">
                    {s.label.split('（')[0]}
                    {s.error && <span className="text-[var(--danger)]">（失败）</span>}
                  </td>
                  <td className="py-0.5 text-right">{s.stats?.n ?? 0}</td>
                  <td className="py-0.5 text-right">{s.stats?.p50 ?? '—'}</td>
                  <td className="py-0.5 text-right">{s.stats?.p95 ?? '—'}</td>
                  <td className="py-0.5 text-right">{s.stats?.max ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="flex gap-1.5">
            <button
              onClick={() => void copyText(formatReportMarkdown(report), '台账表格')}
              className="flex-1 rounded border border-border px-2 py-1 text-[12px] text-[var(--text-secondary)] hover:border-[var(--accent)]"
            >
              复制台账表格
            </button>
            <button
              onClick={() => void copyText(JSON.stringify(report, null, 2), 'JSON')}
              className="flex-1 rounded border border-border px-2 py-1 text-[12px] text-[var(--text-secondary)] hover:border-[var(--accent)]"
            >
              复制 JSON
            </button>
          </div>
        </div>
      )}

      {!report && (
        <p className="text-[12px] text-[var(--text-tertiary)] leading-5">
          步骤：播种语料 → 运行全部场景 → 复制台账表格粘贴给台账 →（可选）清空语料。
          改前/改后必须使用同一份语料。
        </p>
      )}
    </div>
  );
}
