import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { HermesInputRequest, type PendingHermesInput } from '../HermesInputRequest';

function renderRequest(request: PendingHermesInput): string {
  return renderToStaticMarkup(createElement(HermesInputRequest, {
    request,
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
    expect(html.indexOf('允许一次')).toBeLessThan(html.indexOf('本会话允许'));
    expect(html.indexOf('本会话允许')).toBeLessThan(html.indexOf('始终允许'));
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
