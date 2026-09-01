import { useEffect, useState } from 'react';
import { Activity, Trash2 } from 'lucide-react';
import { perfSamples, perfClear, perfSubscribe, startFpsSampler } from '../../services/notePerf';

/**
 * NB-11 性能探针面板（DocWorkspace 状态栏「性能」开关）：
 * 实时 FPS（1s 窗口）+ 最近关键路径耗时（心跳/分屏/提及扫描），宿主走查长文输入与滚动卡顿用。
 * 阈值配色：<8ms 绿（一帧内）/ <16ms 琥珀（一帧边缘）/ ≥16ms 红（掉帧风险）。
 */
export default function PerfOverlay() {
  const [fps, setFps] = useState<number | null>(null);
  const [, force] = useState(0);

  useEffect(() => perfSubscribe(() => force((n) => n + 1)), []);
  useEffect(() => startFpsSampler(setFps), []);

  const samples = perfSamples().slice(-14).reverse();
  const colorOf = (ms: number) =>
    ms < 8 ? 'text-[var(--success)]' : ms < 16 ? 'text-[var(--warning)]' : 'text-[var(--danger)]';
  const fpsColor = fps == null ? 'text-[var(--text-tertiary)]' : fps >= 55 ? 'text-[var(--success)]' : fps >= 30 ? 'text-[var(--warning)]' : 'text-[var(--danger)]';

  return (
    <div className="fixed bottom-9 right-3 z-50 w-60 rounded-lg border border-border bg-[color-mix(in_srgb,var(--bg-surface)_95%,transparent)] shadow-[var(--shadow-lg)] p-2.5 font-mono select-none">
      <div className="flex items-center gap-1.5 mb-1.5">
        <Activity size={11} className="text-[var(--accent)]" />
        <span className="text-[12px] font-semibold text-[var(--text-primary)] flex-1">性能探针</span>
        <span className={`text-[12px] font-bold ${fpsColor}`}>{fps == null ? '—' : fps} fps</span>
        <button
          onClick={perfClear}
          className="text-[var(--text-tertiary)] hover:text-[var(--danger)] transition-colors"
          title="清空记录"
        >
          <Trash2 size={11} />
        </button>
      </div>
      {samples.length === 0 ? (
        <p className="text-[12px] text-[var(--text-tertiary)] py-1">暂无记录（编辑/分屏后出现）</p>
      ) : (
        <div className="space-y-px max-h-44 overflow-y-auto">
          {samples.map((s, i) => (
            <p key={`${s.at}-${i}`} className="flex items-baseline justify-between text-[12px] leading-5">
              <span className="text-[var(--text-tertiary)] truncate mr-2">{s.label}</span>
              <span className={`shrink-0 font-bold ${colorOf(s.ms)}`}>{s.ms}ms</span>
            </p>
          ))}
        </div>
      )}
    </div>
  );
}
