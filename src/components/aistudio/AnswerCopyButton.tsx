import { Check, Copy } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { copyMarkdownSource } from '../../services/clipboard';

/** Copy only the answer source, without progress, routing or usage metadata. */
export default function AnswerCopyButton({ content }: { content: string }) {
  const [state, setState] = useState<'idle' | 'copying' | 'copied' | 'failed'>('idle');
  const generation = useRef(0);
  const busy = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    setState('idle');
    busy.current = false;
    return () => { generation.current++; clearTimeout(timer.current); };
  }, [content]);
  const copy = async () => {
    if (busy.current) return;
    busy.current = true;
    const current = generation.current;
    const answer = content;
    setState('copying');
    clearTimeout(timer.current);
    const copied = await copyMarkdownSource(answer);
    if (current !== generation.current) return;
    busy.current = false;
    setState(copied ? 'copied' : 'failed');
    if (copied) timer.current = setTimeout(() => setState('idle'), 2000);
  };
  return <span className="inline-flex items-center gap-2 text-xs text-[var(--text-tertiary)]">
    <button type="button" className="sn-inline-action inline-flex items-center gap-1.5 py-1"
      onClick={() => void copy()} disabled={state === 'copying'} aria-label="复制回答" title="复制完整回答（Markdown）">
      {state === 'copied' ? <Check size={12} /> : <Copy size={12} />}
      <span>{state === 'copied' ? '已复制' : '复制回答'}</span>
    </button>
    <span role="status">{state === 'failed' ? '复制失败，请重试或选中文字复制' : state === 'copied' ? <span className="sr-only">回答已复制</span> : null}</span>
  </span>;
}
