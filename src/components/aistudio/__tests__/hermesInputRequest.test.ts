import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { HermesInputRequest, type PendingHermesInput } from '../HermesInputRequest';

function renderRequest(request: PendingHermesInput, engineLabel = 'Hermes'): string {
  return renderToStaticMarkup(createElement(HermesInputRequest, {
    request,
    engineLabel,
    onApproval: vi.fn(async () => true),
    onClarify: vi.fn(async () => true),
  }));
}

describe('HermesInputRequest', () => {
  it('renders approval choices as full-width vertical rows in gateway order', () => {
    const html = renderRequest({
      runId: 'run-1',
      payload: {
        type: 'approval_required',
        approvalId: 'approval-1',
        toolName: 'browser_exec',
        argumentsJson: '{"url":"https://example.com"}',
        choices: ['once', 'session', 'always', 'deny'],
      },
    });

    expect(html).toContain('data-slot="hermes-approval-request"');
    expect(html).toContain('data-layout="vertical"');
    expect(html.match(/data-decision-option/g)).toHaveLength(4);
    expect(html.indexOf('允许一次')).toBeLessThan(html.indexOf('本任务允许'));
    expect(html.indexOf('本任务允许')).toBeLessThan(html.indexOf('始终允许'));
    expect(html.indexOf('始终允许')).toBeLessThan(html.indexOf('拒绝'));
    expect(html).toContain('推荐');
    expect(html).toContain('高风险');
  });

  it('renders clarify suggestions vertically and keeps other input below them', () => {
    const html = renderRequest({
      runId: 'run-2',
      payload: {
        type: 'clarify_required',
        requestId: 'clarify-1',
        question: '选择处理范围',
        choices: ['只处理当前文档（推荐）', '处理整个项目', '暂不处理'],
      },
    });

    expect(html).toContain('data-slot="hermes-clarify-request"');
    expect(html).toContain('data-layout="vertical"');
    expect(html.match(/data-decision-option/g)).toHaveLength(3);
    expect(html.indexOf('只处理当前文档')).toBeLessThan(html.indexOf('处理整个项目'));
    expect(html.indexOf('处理整个项目')).toBeLessThan(html.indexOf('其他补充'));
    expect(html).toContain('推荐');
  });
});


// 同一审批组件曾把四个引擎全部标为 Hermes；覆盖标题及提交后的两条分支。
describe.each(['Pi', 'Claude Code', 'OpenCode', 'Hermes'])('%s 输入请求归属', (engineLabel) => {
  it.each(['approval_required', 'clarify_required'] as const)('%s 保持引擎名称、失败重试与回传参数', async (type) => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
    const host = document.createElement('div');
    document.body.append(host);
    const root = createRoot(host);
    const respond = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    const request: PendingHermesInput = {
      runId: 'run-bound',
      payload: type === 'approval_required'
        ? { type, approvalId: 'approval-bound', toolName: 'write', argumentsJson: '{}', choices: ['once', 'deny'] }
        : { type, requestId: 'clarify-bound', question: '选择范围', choices: ['当前项目'] },
    };
    try {
      await act(async () => root.render(createElement(HermesInputRequest, {
        request, engineLabel, onApproval: respond, onClarify: respond,
      })));
      expect(host.textContent).toContain(`${engineLabel} ${type === 'approval_required' ? '正在等待授权' : '需要你的决定'}`);
      await act(async () => host.querySelector<HTMLButtonElement>('[data-decision-option]')!.click());
      expect(host.querySelector('[role="alert"]')?.textContent).toContain(`提交失败，请检查 ${engineLabel} 连接后重试。`);
      await act(async () => host.querySelector<HTMLButtonElement>('[data-decision-option]')!.click());
      expect(host.querySelector('[role="status"]')?.textContent).toContain(`已提交给 ${engineLabel}，正在等待后续进展。`);
      expect(respond).toHaveBeenCalledTimes(2);
      expect(respond.mock.calls[1]).toEqual(type === 'approval_required' ? ['once'] : ['clarify-bound', '当前项目']);
      if (engineLabel !== 'Hermes') expect(host.textContent).not.toContain('Hermes');
    } finally {
      await act(async () => root.unmount());
      host.remove();
    }
  });
});
