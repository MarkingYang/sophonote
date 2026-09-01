import { beforeEach, describe, expect, it, vi } from 'vitest';

const { channels } = vi.hoisted(() => ({
  channels: [] as Array<{ onmessage: ((event: unknown) => void) | null }>,
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
    constructor() {
      channels.push(this);
    }
  },
}));

import { invoke } from '@tauri-apps/api/core';
import { useAgentStore, type AgentAttachmentInput } from '../agentStore';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  channels.length = 0;
  invokeMock.mockReset();
  useAgentStore.setState({
    threads: [],
    selectedThreadId: null,
    activeRuns: {},
    eventsByRunId: {},
    runIdsByThreadId: {},
    messagesByThreadId: {},
    toolCardsByThreadId: {},
    runningRunByThreadId: {},
    historyLoadingByThreadId: {},
    resumingRunByThreadId: {},
    resumeInFlight: {},
    recoveryInFlight: {},
    degraded: {},
    loading: false,
  });
  invokeMock.mockImplementation(async (command) => {
    if (command === 'agent_run_start') {
      return { success: true, data: { threadId: 'thread-1', runId: 'run-1' }, error: null };
    }
    if (command === 'agent_thread_list') {
      return { success: true, data: [], error: null };
    }
    return { success: true, data: null, error: null };
  });
});

describe('DEC-014 Hermes 能力透传', () => {
  it('移除 UI id 后原样传递图片、文件、文件夹、URL 与模型', async () => {
    const attachments: AgentAttachmentInput[] = [
      { id: 'ui-image', kind: 'image', name: 'screen.png', path: '/tmp/screen.png' },
      { id: 'ui-file', kind: 'file', name: 'brief.md', path: '/tmp/brief.md' },
      { id: 'ui-folder', kind: 'folder', name: 'src', path: '/tmp/src' },
      { id: 'ui-url', kind: 'url', name: 'Docs', url: 'https://example.com/docs' },
      { id: 'ui-paste', kind: 'image', name: '粘贴图片', dataUrl: 'data:image/png;base64,iVBORw0KGgo=' },
    ];

    const result = await useAgentStore.getState().startRun(
      null,
      '分析这些资料',
      'project-1',
      null,
      null,
      null,
      attachments,
      'hermes-3-large',
    );

    expect(result).toEqual({ threadId: 'thread-1', runId: 'run-1' });
    const runCall = invokeMock.mock.calls.find(([command]) => command === 'agent_run_start');
    expect(runCall).toBeDefined();
    const request = (runCall![1] as { request: Record<string, unknown> }).request;
    expect(request.hermesModel).toBe('hermes-3-large');
    expect(request.attachments).toEqual(attachments.map(({ id: _id, ...attachment }) => attachment));
    expect(JSON.stringify(request.attachments)).not.toContain('ui-image');
  });

  it('当前文档草稿原样透传给 Rust 原生附件适配层', async () => {
    const focusDocument = {
      articleId: 'article-1',
      title: '未保存草稿',
      baseVersion: 7,
      markdown: '# 草稿\n\n**保持原样**',
    };

    await useAgentStore.getState().startRun(
      null,
      'format',
      'project-1',
      null,
      'sophonote-markdown-writing',
      focusDocument,
    );

    const runCall = invokeMock.mock.calls.find(([command]) => command === 'agent_run_start');
    const request = (runCall![1] as { request: Record<string, unknown> }).request;
    expect(request.focusDocument).toEqual(focusDocument);
  });

  it('显式项目范围作为独立标志透传，不伪造成文档正文', async () => {
    await useAgentStore.getState().startRun(
      null,
      '把刚才的文章保存到这个项目',
      'project-1',
      null,
      'sophonote-note-persistence',
      null,
      [],
      null,
      null,
      null,
      true,
    );

    const runCall = invokeMock.mock.calls.find(([command]) => command === 'agent_run_start');
    const request = (runCall![1] as { request: Record<string, unknown> }).request;
    expect(request.includeProjectContext).toBe(true);
    expect(request.focusDocument).toBeNull();
  });
});
