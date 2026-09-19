import { useEffect, useState } from 'react';
import { opencodeRuntimeStatus, type PiRuntimeStatus } from '../../services/tauri';

export default function OpenCodeRuntimeCard() {
  const [status, setStatus] = useState<PiRuntimeStatus | null>(null);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true;
    setStatus(null); setError('');
    void opencodeRuntimeStatus().then((value) => { if (active) setStatus(value); })
      .catch((value: unknown) => { if (active) setError(String(value)); });
    return () => { active = false; };
  }, [revision]);
  return <section aria-label="OpenCode 运行时" className="rounded-xl border border-[var(--border-default)] bg-[var(--bg-surface)] p-5">
    <div className="flex items-center justify-between gap-4">
      <h3 className="text-base font-semibold text-[var(--text-primary)]">OpenCode</h3>
      <button type="button" onClick={() => setRevision((value) => value + 1)} className="text-xs text-[var(--accent)]">重新检查</button>
    </div>
    <p className="mt-2 text-xs leading-5 text-[var(--text-tertiary)]">官方运行时随应用安装和更新，无需另外安装 OpenCode。共用 AI 模型设置，会话历史独立保存。</p>
    <p className="mt-4 text-xs text-[var(--text-secondary)]">{status ? `v${status.version} · ${status.available ? '已就绪' : '未就绪'}` : error ? '检查失败' : '正在校验运行时…'}</p>
    {(error || status?.error) && <p role="alert" className="mt-2 text-xs text-[var(--danger)]">{error || status?.error}</p>}
  </section>;
}
