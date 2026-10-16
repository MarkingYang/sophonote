import { ArrowDown, ArrowUp, Square } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import AnswerCopyButton from './AnswerCopyButton';
import { isTimelineNearBottom } from '../../services/agentTimelineFollow';
import MarkdownView from '../features/MarkdownView';
import { useAppStore } from '../../stores/appStore';
import { configuredAgentModels } from '../../services/agentModelRouting';
import { useAgentStore } from '../../stores/agentStore';
import { responseActive, responseCall, startTextResponse, type ResponseTurn } from '../../services/lightResponse';

interface Props {
  conversationId: string; projectId: string | null;
  onUpgrade: (text: string) => void; onNew: () => void;
}
/** Independent history and cancellation; never subscribes to Agent events or invents a Run. */
export default function LightResponsePanel({ conversationId, projectId, onUpgrade, onNew }: Props) {
  const [turns, setTurns] = useState<ResponseTurn[]>([]);
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [ready, setReady] = useState(false);
  const [modelChoice, setModelChoice] = useState<{ provider: string; model: string } | null>(null);
  const config = useAppStore(state => state.settings.aiConfig);
  const mounted = useRef(true);
  const sendingRef = useRef(false);
  const editRevision = useRef(0);
  const refreshRevision = useRef(0);
  const lastState = useRef('');
  const viewportRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const followingRef = useRef(true);
  const [showLatest, setShowLatest] = useState(false);
  const followLatest = () => {
    const viewport = viewportRef.current;
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  };
  const jumpToLatest = () => {
    followingRef.current = true;
    setShowLatest(false);
    followLatest();
  };
  useLayoutEffect(() => {
    if (followingRef.current) followLatest();
  }, [turns]);
  useEffect(() => {
    const content = contentRef.current;
    if (!content || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      if (followingRef.current) followLatest();
    });
    observer.observe(content);
    return () => observer.disconnect();
  }, []);
  const refresh = async () => {
    const revision = ++refreshRevision.current;
    const next = await responseCall<ResponseTurn[]>('light_response_history', { conversationId, projectId });
    if (!mounted.current || revision !== refreshRevision.current) return;
    setTurns(next); setReady(true);
    const state = next.map(t => `${t.id}:${t.state}`).join('|');
    if (lastState.current !== state) {
      lastState.current = state;
      void useAgentStore.getState().loadThreads(projectId ?? undefined);
    }
  };
  const active = turns.find(responseActive);
  useEffect(() => {
    mounted.current = true;
    void refresh().catch(e => { if (mounted.current) setError(String(e)); });
    return () => { mounted.current = false; refreshRevision.current++; };
    // The parent keys this panel by project and conversation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    if (!active) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { await refresh(); } catch (e) { if (live) setError(String(e)); }
      if (live) timer = setTimeout(() => { void poll(); }, 1000);
    };
    timer = setTimeout(() => { void poll(); }, 500);
    return () => { live = false; clearTimeout(timer); };
    // Stop polling as soon as a durable terminal state is observed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active?.id]);
  const latest = turns[turns.length - 1];
  const choice = modelChoice ?? (latest ? { provider: latest.provider, model: latest.model } : null);
  const models = Object.entries(config?.providers ?? {}).filter(([, provider]) => provider.protocol === 'openai')
    .flatMap(([provider, settings]) => configuredAgentModels(settings).map(model => ({ provider, model })));
  const modelAvailable = !!choice && models.some(item => item.provider === choice.provider && item.model === choice.model);
  const submit = async () => {
    if (!ready || active || sendingRef.current || !text.trim() || !choice || !modelAvailable) return;
    sendingRef.current = true; setSending(true); setError(null);
    const revision = editRevision.current;
    try {
      const accepted = await startTextResponse({ message: text, threadId: conversationId, projectId, hermesProvider: choice.provider, hermesModel: choice.model });
      if (!mounted.current) return;
      if (!accepted) throw new Error('文本响应未接纳，请保留输入并刷新状态');
      if (editRevision.current === revision) setText('');
      followingRef.current = true;
      setShowLatest(false);
      // Render the durable receipt even if the following history refresh fails.
      setTurns(previous => previous.some(turn => turn.id === accepted.id)
        ? previous.map(turn => turn.id === accepted.id ? accepted : turn) : [...previous, accepted]);
      await refresh();
    } catch (e) { if (mounted.current) setError(String(e)); }
    finally { sendingRef.current = false; if (mounted.current) setSending(false); }
  };
  const stop = async () => {
    if (!active) return;
    try { await responseCall('light_response_cancel', { id: active.id, projectId }); await refresh(); }
    catch (e) { if (mounted.current) setError(String(e)); }
  };
  const upgrade = async () => {
    try {
      const excerpt = await responseCall<string>('light_response_handoff', { conversationId, projectId });
      if (mounted.current) onUpgrade(`${excerpt}\n${text}`);
    } catch (e) { if (mounted.current) setError(String(e)); }
  };
  return <section className="sn-chat-surface flex h-full min-h-0 flex-col" aria-label="文本对话">
    <div className="relative min-h-0 flex-1">
    <div ref={viewportRef} className="sn-chat-viewport absolute inset-0 overflow-y-auto" aria-live="polite"
      onScroll={() => {
        if (!viewportRef.current) return;
        const following = isTimelineNearBottom(viewportRef.current);
        followingRef.current = following;
        setShowLatest(!following);
      }}>
      <div ref={contentRef} className="sn-reading-column sn-chat-timeline">
        {!ready && <p className="text-xs text-[var(--text-tertiary)]">正在读取对话…</p>}
        {turns.map(turn => <article key={turn.id} className="space-y-6">
          <div className="sn-chat-user"><div className="sn-chat-user-bubble">{turn.message}</div></div>
          {turn.answer && <div className="sn-chat-answer" role="region" aria-label="回复">
            <MarkdownView content={turn.answer} className="hb-chat-agent-markdown" copySpecialBlocks={false} lite />
          </div>}
          {responseActive(turn) && <p role="status" className="text-xs text-[var(--text-tertiary)]">{turn.state === 'preparing' ? '正在准备…' : '正在回答…'}</p>}
          {turn.error && <p role="status" className="text-xs text-[var(--warning)]">{turn.error}</p>}
          {!responseActive(turn) && <div className="flex flex-wrap items-start gap-x-4 gap-y-1">
            {turn.state === 'completed' && turn.answer && <AnswerCopyButton content={turn.answer} />}
            <details className="sn-task-details">
              <summary>回答详情</summary>
              <p className="mt-2">模型：{turn.model}</p>
              <p>{turn.usage ? `用量 ${turn.usage.total_tokens} tokens` : '用量未返回'}</p>
            </details>
          </div>}
        </article>)}
      </div>
    </div>
    {showLatest && <button type="button" onClick={jumpToLatest} className="sn-jump-latest" aria-label="回到最新消息"><ArrowDown size={13} />回到最新</button>}
    </div>
    <div className="sn-chat-composer-dock shrink-0"><div className="sn-reading-column">
      <form className="sn-response-composer" onSubmit={e => { e.preventDefault(); void submit(); }}>
        <textarea aria-label="继续文本对话" value={text} onChange={e => { editRevision.current++; setText(e.target.value); }}
          onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing && e.keyCode !== 229) { e.preventDefault(); void submit(); } }}
          placeholder="继续提问…" rows={2} className="sn-response-input" />
        {error && <p role="alert" className="px-4 pb-2 text-xs text-[var(--danger)]">{error} <button type="button" className="sn-inline-action" onClick={() => { setError(null); void refresh().catch(e => setError(String(e))); }}>刷新状态</button></p>}
        <div className="flex min-w-0 items-center justify-between gap-3 px-3 pb-2.5">
          <select aria-label="响应模型" disabled={!!active || sending} value={choice ? JSON.stringify(choice) : ''}
            onChange={event => setModelChoice(JSON.parse(event.target.value) as { provider: string; model: string })}
            className="sn-response-model">
            {!modelAvailable && <option value={choice ? JSON.stringify(choice) : ''}>请选择可用模型</option>}
            {models.map(item => <option key={JSON.stringify(item)} value={JSON.stringify(item)}>{item.model}</option>)}
          </select>
          {active ? <button type="button" aria-label="停止" title="停止" onClick={() => void stop()} className="sn-response-stop"><Square size={12} fill="currentColor" /></button>
            : <button type="submit" aria-label={sending ? '提交中…' : '发送'} title="发送" disabled={!ready || sending || !text.trim() || !modelAvailable} className="sn-response-send"><ArrowUp size={16} /></button>}
        </div>
      </form>
      <details className="sn-task-details sn-response-more mt-2">
        <summary>更多操作</summary>
        <div className="mt-1 flex flex-wrap justify-end gap-3">
          <button type="button" disabled={!ready || !!active || sending} onClick={() => void upgrade()} className="sn-inline-action" title="先核对对话摘录与资料范围，再发送">使用资料与工具继续</button>
          <button type="button" onClick={onNew} className="sn-inline-action">新建对话</button>
        </div>
      </details>
    </div></div>
  </section>;
}
