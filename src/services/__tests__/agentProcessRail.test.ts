/**
 * 过程轨归并与阶段文案纯函数（思维链 P0）。
 */
import { describe, it, expect } from 'vitest';
import {
  activityDurationMs,
  buildProcessActivities,
  deriveRunPhase,
  groupToolCardsByRunId,
  isRichTimelineToolCard,
  processRailSummaryLabel,
  runPhaseLabel,
  shouldShowProcessRail,
  timelineToolCards,
  toolDisplayName,
  toolRunActivityLabel,
  toolStepSummary,
} from '../agentProcessRail';
import type { ToolCard } from '../agentToolCards';
import type { AgentEvent, AgentEventPayload } from '../../stores/agentStore';

function card(over: Partial<ToolCard> & Pick<ToolCard, 'callId' | 'name'>): ToolCard {
  return {
    runId: 'r1',
    threadId: 't1',
    status: 'completed',
    startedAt: 1000,
    ...over,
  };
}

function event(seq: number, payload: AgentEventPayload, timestamp = seq * 1000): AgentEvent {
  return {
    eventId: `r1:${seq}`,
    threadId: 't1',
    runId: 'r1',
    seq,
    timestamp,
    schemaVersion: 2,
    payload,
  };
}

describe('groupToolCardsByRunId / timelineToolCards', () => {
  it('按 runId 归并，只读工具不进时间线，diff 富卡进时间线', () => {
    const cards: ToolCard[] = [
      card({ callId: 'c1', name: 'list_project_documents', startedAt: 1 }),
      card({
        callId: 'c2',
        name: 'read_document',
        runId: 'r1',
        startedAt: 2,
        argumentsJson: JSON.stringify({ articleId: 'art-1' }),
      }),
      card({
        callId: 'c3',
        name: 'propose_document_patch',
        runId: 'r1',
        startedAt: 3,
        uiArtifact: {
          kind: 'diff',
          schemaVersion: 1,
          payload: {
            operationId: 'op1',
            documentId: 'art-1',
            title: '笔记',
            baseVersion: 1,
            targetVersion: 2,
            oldText: 'a',
            newText: 'b',
            hunks: [],
            status: 'pending_approval',
            scope: null,
            rebased: false,
          },
          fallbackMarkdown: 'diff',
        },
      }),
      card({ callId: 'c4', name: 'read_document', runId: 'r2', startedAt: 4 }),
    ];
    const byRun = groupToolCardsByRunId(cards);
    expect(byRun.r1).toHaveLength(3);
    expect(byRun.r2).toHaveLength(1);
    expect(timelineToolCards(cards).map((c) => c.callId)).toEqual(['c3']);
    expect(isRichTimelineToolCard(cards[0])).toBe(false);
    expect(isRichTimelineToolCard(cards[2])).toBe(true);
  });
});

describe('deriveRunPhase / labels', () => {
  it('running tool → tooling；有正文 → answering；否则 waiting', () => {
    expect(
      deriveRunPhase({ streaming: true, hasRunningTool: true, hasAssistantContent: false })
    ).toBe('tooling');
    expect(
      deriveRunPhase({ streaming: true, hasRunningTool: false, hasAssistantContent: true })
    ).toBe('answering');
    expect(
      deriveRunPhase({ streaming: true, hasRunningTool: false, hasAssistantContent: false })
    ).toBe('waiting');
    expect(runPhaseLabel('waiting')).toBe('等待模型…');
    expect(runPhaseLabel('tooling')).toBe('调用工具中');
    expect(runPhaseLabel('answering')).toBe('生成回复中');
  });
});

describe('processRailSummaryLabel / shouldShowProcessRail', () => {
  it('流式/定稿文案与展示条件', () => {
    expect(
      processRailSummaryLabel({
        streaming: true,
        hasTools: false,
        formatDuration: () => '3 秒',
      })
    ).toBe('思考中…');
    expect(
      processRailSummaryLabel({
        streaming: true,
        hasTools: true,
        formatDuration: () => '3 秒',
      })
    ).toBe('执行过程 · 进行中');
    expect(
      processRailSummaryLabel({
        streaming: false,
        hasTools: true,
        durationMs: 3200,
        formatDuration: (ms) => (ms < 1000 ? '<1 秒' : `${Math.round(ms / 1000)} 秒`),
      })
    ).toBe('已思考 · 3 秒');
    expect(shouldShowProcessRail({ streaming: true, hasReasoning: false, hasTools: false })).toBe(
      true
    );
    expect(
      shouldShowProcessRail({
        streaming: true,
        hasReasoning: false,
        hasTools: false,
        hasAnswer: true,
      })
    ).toBe(false);
    expect(shouldShowProcessRail({ streaming: false, hasReasoning: false, hasTools: false })).toBe(
      false
    );
    expect(shouldShowProcessRail({ streaming: false, hasReasoning: false, hasTools: true })).toBe(
      true
    );
    expect(
      shouldShowProcessRail({
        streaming: true,
        hasReasoning: false,
        hasTools: true,
        hasAnswer: true,
      })
    ).toBe(true);
  });
});

describe('toolDisplayName / toolStepSummary', () => {
  it('脱敏短摘要，不展示 localhost', () => {
    expect(toolDisplayName('mcp_sophonote-bridge_read_document')).toBe('读取文档');
    expect(toolDisplayName('mcp__sophonote_bridge__list_project_documents')).toBe('查看项目文档');
    expect(toolDisplayName('sophonote_project_tree')).toBe('整理项目目录');
    expect(
      toolStepSummary(
        card({
          callId: 'c1',
          name: 'read_document',
          argumentsJson: JSON.stringify({ articleId: 'doc-42' }),
        })
      )
    ).toBe('doc-42');
    expect(
      toolStepSummary(
        card({
          callId: 'c2',
          name: 'read_document',
          argumentsJson: JSON.stringify({ url: 'http://127.0.0.1:18765/mcp' }),
        })
      )
    ).toBeNull();
  });

  it('为 Browser 与代码工具显示可读步骤，并只展示安全网址摘要', () => {
    expect(toolDisplayName('browser_console')).toBe('检查控制台');
    expect(toolDisplayName('write_file')).toBe('修改文件');
    expect(
      toolStepSummary(
        card({
          callId: 'browser-1',
          name: 'browser_navigate',
          argumentsJson: JSON.stringify({ url: 'http://localhost:3000/login?token=secret' }),
        })
      )
    ).toBe('localhost:3000/login');
  });
});

describe('Hermes Desktop process activities', () => {
  it('按 seq 保留 Thought → Explored → Thought 的真实交替顺序', () => {
    const cards = [
      card({
        callId: 'c1',
        name: 'read_document',
        startedAt: 3000,
        completedAt: 3900,
        argumentsJson: JSON.stringify({ title: '架构.md' }),
      }),
    ];
    const activities = buildProcessActivities(
      [
        event(1, { type: 'reasoning_delta', text: '先定位' }, 1000),
        event(2, { type: 'reasoning_delta', text: '文件' }, 1200),
        event(3, { type: 'tool_started', callId: 'c1', name: 'read_document', argumentsJson: '{}' }, 3000),
        event(4, { type: 'tool_completed', callId: 'c1', name: 'read_document', ok: true, error: null, preresolved: false }, 3900),
        event(5, { type: 'reasoning_delta', text: '整理结论' }, 5000),
        event(6, { type: 'run_completed', outcome: 'completed', finalAnswer: '答案', modelCalls: 1 }, 7000),
      ],
      cards,
      'done'
    );

    expect(activities.map((item) => item.kind)).toEqual(['reasoning', 'tools', 'reasoning']);
    expect(activities[0]).toMatchObject({ text: '先定位文件', endedAt: 3000 });
    expect(activities[1]).toMatchObject({ cards, endedAt: 5000 });
    expect(activityDurationMs(activities[2], 9000)).toBe(2000);
    expect(toolRunActivityLabel(cards, false)).toBe('Explored 《架构.md》');
  });

  it('活动中的最后一个 reasoning block 保持 running', () => {
    const activities = buildProcessActivities(
      [event(1, { type: 'reasoning_delta', text: '正在检查' }, 1000)],
      [],
      'thinking'
    );
    expect(activities[0]).toMatchObject({ kind: 'reasoning', running: true, endedAt: undefined });
  });

  it('Run 已终态时收口缺少 tool_completed 的悬空工具', () => {
    const dangling = [
      card({
        callId: 'c1',
        name: 'browser_exec',
        status: 'running',
        startedAt: 1000,
        completedAt: undefined,
      }),
    ];
    const activities = buildProcessActivities(
      [
        event(1, { type: 'tool_started', callId: 'c1', name: 'browser_exec', argumentsJson: '{}' }, 1000),
        event(2, { type: 'run_failed', outcome: 'interrupted', error: '恢复失败' }, 5000),
      ],
      dangling,
      'error'
    );
    expect(activities[0]).toMatchObject({ kind: 'tools', running: false, endedAt: 5000 });
    expect(activityDurationMs(activities[0], 9000)).toBe(4000);
  });

  it('把 Hermes MCP 下划线命名空间识别为 Explored', () => {
    const cards = [
      card({
        callId: 'c1',
        name: 'mcp__sophonote_bridge__list_project_documents',
        startedAt: 1000,
        completedAt: 1200,
      }),
    ];
    expect(toolRunActivityLabel(cards, false)).toBe('Explored 1 file');
  });
});
