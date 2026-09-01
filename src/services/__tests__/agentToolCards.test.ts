/**
 * AG-21：工具结果卡纯函数层单测（services/agentToolCards.ts）。
 * 覆盖：callId 幂等 upsert、重放无 started 直建卡、五件套字段归约、
 * pickArtifactView allowlist 四 kind（AG-26 起含 diff）+ 未知 kind/畸形 payload → fallback。
 * 零 Tauri 依赖、零真实模型调用。
 */
import { describe, it, expect } from 'vitest';
import {
  upsertToolCard,
  reduceToolCards,
  pickArtifactView,
  isToolEvent,
  ALLOWED_ARTIFACT_KINDS,
  type ToolCard,
  type ToolEventLike,
  type UiArtifact,
} from '../agentToolCards';

/** 事件工厂（最小信封） */
function ev(payload: ToolEventLike['payload'], over: Partial<ToolEventLike> = {}): ToolEventLike {
  return { runId: 'r1', threadId: 't1', timestamp: 1000, payload, ...over };
}

const started = (callId = 'c1', name = 'get_weather'): ToolEventLike =>
  ev({ type: 'tool_started', callId, name, argumentsJson: '{}' });

const artifact = (kind: string, payload: unknown, fallback = '回退文本'): UiArtifact => ({
  kind,
  schemaVersion: 1,
  payload,
  fallbackMarkdown: fallback,
});

const completed = (over: Partial<Extract<ToolEventLike['payload'], { type: 'tool_completed' }>> = {}): ToolEventLike =>
  ev({
    type: 'tool_completed',
    callId: 'c1',
    name: 'get_weather',
    ok: true,
    error: null,
    preresolved: false,
    ...over,
  });

describe('upsertToolCard 幂等与补全', () => {
  it('started 建 running 卡；重复 started 幂等不重复', () => {
    let cards = upsertToolCard([], started());
    cards = upsertToolCard(cards, started());
    expect(cards).toHaveLength(1);
    expect(cards[0].status).toBe('running');
    expect(cards[0].name).toBe('get_weather');
  });

  it('started→completed 补全五件套字段', () => {
    let cards = upsertToolCard([], started());
    cards = upsertToolCard(
      cards,
      completed({
        structured: { city: '杭州' },
        uiArtifact: artifact('key-value', { rows: [['city', '杭州']] }),
        truncated: true,
        provenance: [{ source: 'project-document', sourceId: 'a1', title: '测试笔记' }],
      })
    );
    expect(cards).toHaveLength(1);
    const card = cards[0];
    expect(card.status).toBe('completed');
    expect(card.structured).toEqual({ city: '杭州' });
    expect(card.uiArtifact?.kind).toBe('key-value');
    expect(card.truncated).toBe(true);
    expect(card.provenance?.[0].source).toBe('project-document');
    // 卡片上不存在 model_text 通道（结构性隔离）
    expect('model_text' in card).toBe(false);
    expect('modelText' in card).toBe(false);
  });

  it('失败事件 → failed + 错误文本；preresolved 原样透传', () => {
    let cards = upsertToolCard([], started());
    cards = upsertToolCard(cards, completed({ ok: false, error: '参数无效' }));
    expect(cards[0].status).toBe('failed');
    expect(cards[0].error).toBe('参数无效');

    let pre = upsertToolCard([], completed({ callId: 'c2', preresolved: true }));
    expect(pre[0].preresolved).toBe(true);
    expect(pre[0].status).toBe('completed');
  });

  it('重放场景：只有 completed 无 started → 直建完成卡', () => {
    const cards = upsertToolCard([], completed({ structured: { k: 1 } }));
    expect(cards).toHaveLength(1);
    expect(cards[0].status).toBe('completed');
    expect(cards[0].structured).toEqual({ k: 1 });
  });

  it('旧事件（AG-21 前，无新字段）→ 缺省兜底，不报错', () => {
    const cards = upsertToolCard([], completed());
    const card = cards[0];
    expect(card.truncated).toBe(false);
    expect(card.provenance).toEqual([]);
    expect(card.uiArtifact).toBeNull();
    expect(card.structured).toBeUndefined();
  });
});

describe('reduceToolCards 全量归约', () => {
  it('多次调用 + 多 callId 按事件序归约', () => {
    const cards = reduceToolCards([
      started('c1', 'get_weather'),
      started('c2', 'calculator'),
      completed({ callId: 'c1', structured: { city: '杭州' } }),
      completed({ callId: 'c2', ok: false, error: '除数不能为零' }),
    ]);
    expect(cards.map((c) => c.callId)).toEqual(['c1', 'c2']);
    expect(cards[0].status).toBe('completed');
    expect(cards[1].status).toBe('failed');
  });

  it('Run 终态收口未收到 tool_completed 的悬空卡', () => {
    const cards = reduceToolCards([
      started('c1', 'browser_exec'),
      {
        runId: 'r1',
        threadId: 't1',
        timestamp: 5000,
        payload: { type: 'run_failed', outcome: 'interrupted', error: '恢复失败' },
      },
    ]);
    expect(cards[0]).toMatchObject({
      status: 'failed',
      completedAt: 5000,
      error: '恢复失败',
    });
  });
});

describe('isToolEvent 判别', () => {
  it('只认 tool_started/tool_completed', () => {
    expect(isToolEvent(ev({ type: 'tool_started', callId: 'c', name: 'n' }))).toBe(true);
    expect(
      isToolEvent(
        ev({ type: 'tool_completed', callId: 'c', name: 'n', ok: true, error: null, preresolved: false })
      )
    ).toBe(true);
    expect(
      isToolEvent({ runId: 'r', threadId: 't', timestamp: 1, payload: { type: 'run_started' } })
    ).toBe(false);
  });
});

describe('pickArtifactView allowlist 与回退', () => {
  const cardWith = (uiArtifact: UiArtifact | null, structured?: unknown): ToolCard => ({
    callId: 'c1',
    runId: 'r1',
    threadId: 't1',
    name: 't',
    status: 'completed',
    startedAt: 1,
    uiArtifact,
    structured,
  });

  it('allowlist 恰为五种 kind（AG-26 起含 diff 审批卡；含 rename 改名提案卡）', () => {
    expect([...ALLOWED_ARTIFACT_KINDS].sort()).toEqual([
      'diff',
      'key-value',
      'markdown',
      'rename',
      'table',
    ]);
  });

  it('key-value → rows 视图', () => {
    const view = pickArtifactView(
      cardWith(artifact('key-value', { rows: [['city', '杭州'], ['temperature_c', 26]] }))
    );
    expect(view).toEqual({
      mode: 'keyValue',
      rows: [['city', '杭州'], ['temperature_c', 26]],
    });
  });

  it('table → columns/rows 视图', () => {
    const view = pickArtifactView(
      cardWith(artifact('table', { columns: ['a', 'b'], rows: [[1, 2], [3, 4]] }))
    );
    expect(view).toEqual({ mode: 'table', columns: ['a', 'b'], rows: [[1, 2], [3, 4]] });
  });

  it('markdown → markdown 视图', () => {
    const view = pickArtifactView(cardWith(artifact('markdown', { markdown: '# 正文' })));
    expect(view).toEqual({ mode: 'markdown', markdown: '# 正文' });
  });

  it('未知 kind → fallbackMarkdown（纵深防御，不识别不执行）', () => {
    const view = pickArtifactView(cardWith(artifact('evil-html', { html: '<script>' })));
    expect(view).toEqual({ mode: 'fallback', markdown: '回退文本' });
  });

  it('allowlist kind 但 payload 畸形 → 也走 fallback', () => {
    expect(pickArtifactView(cardWith(artifact('key-value', { rows: 'bad' })))).toEqual({
      mode: 'fallback',
      markdown: '回退文本',
    });
    expect(pickArtifactView(cardWith(artifact('table', { columns: [] })))).toEqual({
      mode: 'fallback',
      markdown: '回退文本',
    });
    expect(pickArtifactView(cardWith(artifact('markdown', { markdown: '' })))).toEqual({
      mode: 'fallback',
      markdown: '回退文本',
    });
  });

  it('无 envelope → none（structured 只读预览由组件承担）', () => {
    expect(pickArtifactView(cardWith(null, { k: 1 }))).toEqual({ mode: 'none' });
  });

  it('rename → 改名提案视图', () => {
    const view = pickArtifactView(
      cardWith(
        artifact('rename', {
          operationId: 'rename-1',
          documentId: 'doc-1',
          oldTitle: '旧标题',
          newTitle: '新标题',
          wikilinkAffectedCount: 3,
          status: 'pending_approval',
        })
      )
    );
    expect(view).toEqual({
      mode: 'rename',
      preview: {
        operationId: 'rename-1',
        documentId: 'doc-1',
        oldTitle: '旧标题',
        newTitle: '新标题',
        wikilinkAffectedCount: 3,
        status: 'pending_approval',
      },
    });
  });

  it('rename kind 但 payload 畸形 → 回退 fallback（不渲染半残卡）', () => {
    expect(pickArtifactView(cardWith(artifact('rename', { newTitle: '只有新标题' })))).toEqual({
      mode: 'fallback',
      markdown: '回退文本',
    });
    expect(pickArtifactView(cardWith(artifact('rename', { newTitle: '' })))).toEqual({
      mode: 'fallback',
      markdown: '回退文本',
    });
  });
});
