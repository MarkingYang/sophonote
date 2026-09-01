import { useId, useMemo, useState } from 'react';
import { ArrowRight, Loader2, PencilLine, ShieldAlert } from 'lucide-react';
import type { AgentEvent } from '../../stores/agentStore';

export type PendingHermesInput = {
  runId: string;
  payload: Extract<AgentEvent['payload'], { type: 'approval_required' | 'clarify_required' }>;
};

type ApprovalChoiceMeta = {
  label: string;
  description: string;
  badge?: '推荐' | '高风险';
  tone: 'default' | 'danger' | 'warning';
};

function uniqueChoices(choices: string[]): string[] {
  return [...new Set(choices.map((choice) => choice.trim()).filter(Boolean))];
}

function approvalChoiceMeta(choice: string): ApprovalChoiceMeta {
  if (choice === 'once') {
    return {
      label: '允许一次',
      description: '仅允许当前这一次操作，后续仍会再次询问。',
      badge: '推荐',
      tone: 'default',
    };
  }
  if (choice === 'session') {
    return {
      label: '本会话允许',
      description: '当前会话内的同类操作不再重复询问。',
      tone: 'default',
    };
  }
  if (choice === 'always') {
    return {
      label: '始终允许',
      description: '持久允许后续同类操作，请确认来源和影响范围。',
      badge: '高风险',
      tone: 'warning',
    };
  }
  if (choice === 'deny') {
    return {
      label: '拒绝',
      description: '不执行当前操作，Agent 会收到拒绝结果。',
      tone: 'danger',
    };
  }
  return { label: choice, description: '', tone: 'default' };
}

function clarifyChoiceMeta(choice: string): { label: string; recommended: boolean } {
  const recommended = /(?:[（(\[]\s*(?:推荐|recommended)\s*[）)\]])/iu.test(choice);
  return {
    label: choice.replace(/(?:[（(\[]\s*(?:推荐|recommended)\s*[）)\]])/giu, '').trim(),
    recommended,
  };
}

function optionToneClasses(tone: ApprovalChoiceMeta['tone'], recommended: boolean): string {
  if (tone === 'danger') {
    return 'text-[var(--danger)] hover:bg-[var(--danger-subtle)] focus-visible:bg-[var(--danger-subtle)]';
  }
  if (tone === 'warning') {
    return 'text-[var(--warning)] hover:bg-[var(--warning-subtle)] focus-visible:bg-[var(--warning-subtle)]';
  }
  if (recommended) {
    return 'bg-[var(--accent-subtle)] text-[var(--text-primary)] hover:bg-[var(--accent-subtle)] focus-visible:bg-[var(--accent-subtle)]';
  }
  return 'text-[var(--text-secondary)] hover:bg-[var(--bg-sunken)] focus-visible:bg-[var(--bg-sunken)]';
}

function DecisionOption({
  index,
  label,
  description,
  badge,
  tone = 'default',
  disabled,
  sending,
  onChoose,
}: {
  index: number;
  label: string;
  description?: string;
  badge?: '推荐' | '高风险';
  tone?: ApprovalChoiceMeta['tone'];
  disabled: boolean;
  sending: boolean;
  onChoose: () => void;
}) {
  const recommended = badge === '推荐';
  return (
    <li>
      <button
        type="button"
        data-decision-option
        disabled={disabled}
        onClick={onChoose}
        className={`group flex w-full items-start gap-2.5 px-2.5 py-2 text-left outline-none transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${optionToneClasses(tone, recommended)}`}
      >
        <span className={`mt-px flex h-6 w-6 shrink-0 items-center justify-center rounded-md border text-xs font-medium ${
          recommended
            ? 'border-[var(--accent-border)] bg-[var(--bg-surface)] text-[var(--accent)]'
            : tone === 'danger'
              ? 'border-[var(--danger)] bg-[var(--bg-surface)] text-[var(--danger)]'
              : tone === 'warning'
                ? 'border-[var(--gold-border)] bg-[var(--bg-surface)] text-[var(--warning)]'
                : 'border-[var(--border-default)] bg-[var(--bg-surface)] text-[var(--text-tertiary)]'
        }`}>
          {index + 1}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex flex-wrap items-center gap-1.5 text-[12px] font-medium leading-5">
            <span>{label}</span>
            {badge && (
              <span className={`rounded-full px-1.5 py-0.5 text-xs font-medium ${
                badge === '高风险'
                  ? 'bg-[var(--warning-subtle)] text-[var(--warning)]'
                  : 'bg-[var(--accent-subtle)] text-[var(--accent)]'
              }`}>
                {badge}
              </span>
            )}
          </span>
          {description && (
            <span className="block text-xs font-normal leading-4 text-[var(--text-tertiary)]">
              {description}
            </span>
          )}
        </span>
        {sending
          ? <Loader2 className="mt-1 h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden="true" />
          : <ArrowRight className="mt-1 h-3.5 w-3.5 shrink-0 text-[var(--text-disabled)] transition-transform group-hover:translate-x-0.5 group-hover:text-[var(--text-tertiary)]" aria-hidden="true" />}
      </button>
    </li>
  );
}

/** Hermes 阻塞式输入卡：回答直接回到同一 Session，不转换成下一轮用户消息。 */
export function HermesInputRequest({
  request,
  onApproval,
  onClarify,
}: {
  request: PendingHermesInput;
  onApproval: (choice: string) => Promise<boolean>;
  onClarify: (requestId: string, answer: string) => Promise<boolean>;
}) {
  const titleId = useId();
  const [answer, setAnswer] = useState('');
  const [sendingValue, setSendingValue] = useState<string | null>(null);
  const [resolved, setResolved] = useState(false);
  const [submitError, setSubmitError] = useState(false);
  const [confirmingAlways, setConfirmingAlways] = useState(false);
  const payload = request.payload;
  const approvalChoices = useMemo(
    () => payload.type === 'approval_required'
      ? uniqueChoices(payload.choices.length > 0 ? payload.choices : ['once', 'session', 'deny'])
      : [],
    [payload]
  );
  const clarifyChoices = useMemo(
    () => payload.type === 'clarify_required' ? uniqueChoices(payload.choices) : [],
    [payload]
  );

  const submitClarify = async (value: string) => {
    const trimmed = value.trim();
    if (sendingValue || resolved || !trimmed || payload.type !== 'clarify_required') return;
    setSubmitError(false);
    setSendingValue(trimmed);
    const ok = await onClarify(payload.requestId, trimmed);
    setSendingValue(null);
    if (ok) setResolved(true);
    else setSubmitError(true);
  };

  const submitApproval = async (choice: string) => {
    if (sendingValue || resolved || payload.type !== 'approval_required') return;
    if (choice === 'always' && !confirmingAlways) {
      setConfirmingAlways(true);
      return;
    }
    setSubmitError(false);
    setSendingValue(choice);
    const ok = await onApproval(choice);
    setSendingValue(null);
    if (ok) setResolved(true);
    else setSubmitError(true);
  };

  if (resolved) {
    return (
      <div className="mb-2 rounded-lg border border-[var(--success)] bg-[var(--success-subtle)] px-3 py-2 text-xs text-[var(--success)]" role="status">
        已提交给 Hermes，Agent 正在继续执行。
      </div>
    );
  }

  if (payload.type === 'approval_required') {
    return (
      <section
        className="mb-2 overflow-hidden rounded-xl border border-[var(--gold-border)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]"
        aria-labelledby={titleId}
        aria-busy={sendingValue != null}
        data-layout="vertical"
        data-slot="hermes-approval-request"
      >
        <div className="flex items-start gap-2.5 border-b border-[var(--gold-border)] bg-[var(--warning-subtle)] px-3 py-2.5">
          <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0 text-[var(--warning)]" aria-hidden="true" />
          <div className="min-w-0">
            <p id={titleId} className="text-[12px] font-semibold leading-5 text-[var(--text-primary)]">
              Hermes 正在等待授权
            </p>
            <p className="font-mono text-xs leading-4 text-[var(--text-secondary)]">{payload.toolName}</p>
          </div>
        </div>
        {payload.argumentsJson && payload.argumentsJson !== '{}' && (
          <code className="mx-2.5 mt-2 block max-h-20 overflow-auto rounded-[6px] bg-[var(--bg-sunken)] px-2.5 py-2 font-mono text-[13px] leading-relaxed text-[var(--text-secondary)]">
            {payload.argumentsJson.replace(/^"|"$/g, '')}
          </code>
        )}
        <ol className="m-2.5 divide-y divide-[var(--border-default)] overflow-hidden rounded-lg border border-[var(--border-default)]" aria-label="授权选项">
          {approvalChoices.map((choice, index) => {
            const meta = approvalChoiceMeta(choice);
            return (
              <DecisionOption
                key={choice}
                index={index}
                label={meta.label}
                description={meta.description}
                badge={meta.badge}
                tone={meta.tone}
                disabled={sendingValue != null}
                sending={sendingValue === choice}
                onChoose={() => {
                  setConfirmingAlways(false);
                  void submitApproval(choice);
                }}
              />
            );
          })}
        </ol>
        {confirmingAlways && (
          <div className="mx-2.5 mb-2.5 rounded-lg border border-[var(--gold-border)] bg-[var(--warning-subtle)] px-2.5 py-2" role="alertdialog" aria-label="确认始终允许">
            <p className="text-xs leading-4 text-[var(--warning)]">
              “始终允许”会持久影响后续同类操作。确认你信任该工具及其当前作用范围。
            </p>
            <div className="mt-2 flex justify-end gap-1.5">
              <button
                type="button"
                disabled={sendingValue != null}
                onClick={() => setConfirmingAlways(false)}
                className="rounded-md px-2 py-1 text-xs text-[var(--text-secondary)] hover:bg-[var(--bg-surface)]"
              >
                取消
              </button>
              <button
                type="button"
                disabled={sendingValue != null}
                onClick={() => void submitApproval('always')}
                className="rounded-md bg-[var(--warning)] px-2 py-1 text-xs text-white hover:opacity-90 disabled:opacity-50"
              >
                确认始终允许
              </button>
            </div>
          </div>
        )}
        {submitError && (
          <p className="px-3 pb-2.5 text-xs text-[var(--danger)]" role="alert">
            提交失败，请检查 Hermes 连接后重试。
          </p>
        )}
      </section>
    );
  }

  return (
    <section
      className="mb-2 overflow-hidden rounded-xl border border-[var(--accent-border)] bg-[var(--bg-surface)] shadow-[var(--shadow-sm)]"
      aria-labelledby={titleId}
      aria-busy={sendingValue != null}
      data-layout="vertical"
      data-slot="hermes-clarify-request"
    >
      <div className="border-b border-[var(--accent-border)] bg-[var(--accent-subtle)] px-3 py-2.5">
        <p id={titleId} className="text-[12px] font-semibold leading-5 text-[var(--text-primary)]">
          Hermes 需要你的决定
        </p>
        <p className="mt-0.5 text-xs leading-relaxed text-[var(--text-secondary)]">{payload.question}</p>
      </div>
      {clarifyChoices.length > 0 && (
        <ol className="m-2.5 mb-0 divide-y divide-[var(--border-default)] overflow-hidden rounded-lg border border-[var(--border-default)]" aria-label="可选建议">
          {clarifyChoices.map((choice, index) => {
            const meta = clarifyChoiceMeta(choice);
            return (
              <DecisionOption
                key={choice}
                index={index}
                label={meta.label}
                badge={meta.recommended ? '推荐' : undefined}
                disabled={sendingValue != null}
                sending={sendingValue === choice}
                onChoose={() => void submitClarify(choice)}
              />
            );
          })}
        </ol>
      )}
      <div className="m-2.5 flex items-center gap-2 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-surface)] px-2.5 py-1.5 transition-all focus-within:border-[var(--accent)] focus-within:shadow-[0_0_0_3px_var(--accent-subtle)]">
        <PencilLine className="h-3.5 w-3.5 shrink-0 text-[var(--text-tertiary)]" aria-hidden="true" />
        <input
          value={answer}
          disabled={sendingValue != null}
          onChange={(event) => setAnswer(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void submitClarify(answer);
            }
          }}
          placeholder="其他补充…"
          aria-label="其他补充"
          className="min-w-0 flex-1 bg-transparent text-sm text-[var(--text-primary)] outline-none placeholder:text-[var(--text-disabled)]"
        />
        <button
          type="button"
          aria-label="提交补充回答"
          disabled={sendingValue != null || !answer.trim()}
          onClick={() => void submitClarify(answer)}
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-[var(--accent)] text-white transition-colors hover:bg-[var(--accent-strong)] disabled:bg-[var(--border-default)]"
        >
          {sendingValue === answer.trim()
            ? <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
            : <ArrowRight className="h-3.5 w-3.5" aria-hidden="true" />}
        </button>
      </div>
      {submitError && (
        <p className="px-3 pb-2.5 text-xs text-[var(--danger)]" role="alert">
          提交失败，请检查 Hermes 连接后重试。
        </p>
      )}
    </section>
  );
}
