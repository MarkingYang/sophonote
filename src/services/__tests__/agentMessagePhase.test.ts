/**
 * 助手消息阶段状态机（thinking / answering / done / error）
 */
import { describe, it, expect } from 'vitest';
import {
  areaADefaultOpen,
  areaASummaryLabel,
  deriveAssistantPhase,
  deriveContentStatus,
  deriveThinkingStatus,
  reduceAssistantPhase,
  shouldRenderAreaBContent,
  shouldShowAreaA,
} from '../agentMessagePhase';

describe('deriveAssistantPhase', () => {
  it('仅 reasoning/tool → thinking', () => {
    expect(
      deriveAssistantPhase(['run_started', 'reasoning_delta', 'tool_started', 'tool_completed'])
    ).toBe('thinking');
  });

  it('首条 message_delta 合成 thinking_end → answering', () => {
    expect(
      deriveAssistantPhase(['run_started', 'reasoning_delta', 'message_delta', 'message_delta'])
    ).toBe('answering');
  });

  it('显式 reasoning_completed → answering', () => {
    expect(deriveAssistantPhase(['run_started', 'reasoning_delta', 'reasoning_completed'])).toBe(
      'answering'
    );
  });

  it('run_completed → done；failed → error', () => {
    expect(deriveAssistantPhase(['run_started', 'message_delta', 'run_completed'])).toBe('done');
    expect(deriveAssistantPhase(['run_started', 'reasoning_delta', 'run_failed'])).toBe('error');
    expect(deriveAssistantPhase(['run_started', 'run_cancelled'])).toBe('error');
  });
});

describe('Area A / Area B 可见性', () => {
  it('thinking 始终展示 Area A；无过程内容的 done 隐藏', () => {
    expect(shouldShowAreaA({ phase: 'thinking', hasReasoning: false, hasTools: false })).toBe(
      true
    );
    expect(shouldShowAreaA({ phase: 'done', hasReasoning: false, hasTools: false })).toBe(false);
    expect(shouldShowAreaA({ phase: 'done', hasReasoning: true, hasTools: false })).toBe(true);
  });

  it('thinking 不渲染 Area B 正文；answering/done 有内容才渲染', () => {
    expect(shouldRenderAreaBContent({ phase: 'thinking', hasContent: true })).toBe(false);
    expect(shouldRenderAreaBContent({ phase: 'answering', hasContent: true })).toBe(true);
    expect(shouldRenderAreaBContent({ phase: 'answering', hasContent: false })).toBe(false);
  });

  it('默认展开仅 thinking', () => {
    expect(areaADefaultOpen('thinking')).toBe(true);
    expect(areaADefaultOpen('answering')).toBe(false);
    expect(areaADefaultOpen('done')).toBe(false);
  });
});

describe('status / labels', () => {
  it('thinkingStatus / contentStatus', () => {
    expect(
      deriveThinkingStatus({ phase: 'thinking', hasReasoning: false, hasTools: false })
    ).toBe('streaming');
    expect(deriveThinkingStatus({ phase: 'done', hasReasoning: false, hasTools: false })).toBe(
      'none'
    );
    expect(deriveContentStatus({ phase: 'thinking', hasContent: false })).toBe('pending');
    expect(deriveContentStatus({ phase: 'answering', hasContent: true })).toBe('streaming');
    expect(deriveContentStatus({ phase: 'done', hasContent: true })).toBe('done');
  });

  it('Area A 摘要文案', () => {
    expect(
      areaASummaryLabel({
        phase: 'thinking',
        hasTools: false,
        formatDuration: () => '1 秒',
      })
    ).toBe('正在思考…');
    expect(
      areaASummaryLabel({
        phase: 'done',
        hasTools: false,
        durationMs: 3200,
        formatDuration: (ms) => `${Math.round(ms / 1000)} 秒`,
      })
    ).toBe('已思考 · 3 秒');
  });

  it('reduceAssistantPhase 幂等终态', () => {
    expect(reduceAssistantPhase('done', 'message_delta')).toBe('done');
    expect(reduceAssistantPhase('error', 'run_completed')).toBe('error');
  });
});
