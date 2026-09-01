/**
 * AG-26：diff 审批卡纯函数层单测（parseDiff + pickArtifactView diff 分支）。
 * 覆盖：合法 PatchPreview 投影 → diff 视图；关键字段缺失/畸形 → fallback；
 * hunk 数组元素缺 startLine → fallback（不渲染半残卡）；可选字段缺省值。
 * 零 Tauri 依赖、零真实模型调用。
 */
import { describe, it, expect } from 'vitest';
import {
  parseDiff,
  pickArtifactView,
  type ToolCard,
  type UiArtifact,
} from '../agentToolCards';

const artifact = (payload: unknown, fallback = '回退文本'): UiArtifact => ({
  kind: 'diff',
  schemaVersion: 1,
  payload,
  fallbackMarkdown: fallback,
});

const cardWith = (artifactValue: UiArtifact | null): ToolCard => ({
  callId: 'c1',
  runId: 'r1',
  threadId: 't1',
  name: 'propose_document_patch',
  status: 'completed',
  startedAt: 1000,
  completedAt: 1100,
  uiArtifact: artifactValue,
});

const validPayload = {
  operationId: 'op-1',
  documentId: 'doc-1',
  title: '测试笔记',
  baseVersion: 1,
  targetVersion: 2,
  hunks: [
    {
      startLine: 0,
      contextBefore: [],
      removed: ['旧段落'],
      added: ['新段落'],
      contextAfter: ['后文'],
    },
  ],
  status: 'pending_approval',
  scope: 'selection',
  rebased: false,
};

describe('parseDiff', () => {
  it('合法 PatchPreview 投影 → 完整视图模型', () => {
    const parsed = parseDiff(validPayload);
    expect(parsed).not.toBeNull();
    expect(parsed!.operationId).toBe('op-1');
    expect(parsed!.documentId).toBe('doc-1');
    expect(parsed!.baseVersion).toBe(1);
    expect(parsed!.hunks).toHaveLength(1);
    expect(parsed!.hunks[0].removed).toEqual(['旧段落']);
    expect(parsed!.scope).toBe('selection');
    expect(parsed!.rebased).toBe(false);
  });

  it('NEXT-042：proposedTitle 存在则解析、缺省回落 null', () => {
    expect(parseDiff(validPayload)!.proposedTitle).toBeNull();
    expect(
      parseDiff({ ...validPayload, proposedTitle: 'AI圈动态' })!.proposedTitle
    ).toBe('AI圈动态');
  });

  it('operationId / documentId 缺失或空 → null', () => {
    expect(parseDiff({ ...validPayload, operationId: '' })).toBeNull();
    expect(parseDiff({ ...validPayload, documentId: 123 })).toBeNull();
    const noOp = { ...validPayload } as Record<string, unknown>;
    delete noOp.operationId;
    expect(parseDiff(noOp)).toBeNull();
  });

  it('hunks 非数组 / 元素缺 startLine → null', () => {
    expect(parseDiff({ ...validPayload, hunks: 'none' })).toBeNull();
    expect(
      parseDiff({ ...validPayload, hunks: [{ removed: ['x'], added: ['y'] }] })
    ).toBeNull();
  });

  it('可选字段缺失 → 安全缺省（title 空串 / status pending_approval / scope null）', () => {
    const parsed = parseDiff({
      operationId: 'op-2',
      documentId: 'doc-2',
      hunks: [{ startLine: 3, removed: [], added: ['插入行'] }],
    });
    expect(parsed).not.toBeNull();
    expect(parsed!.title).toBe('');
    expect(parsed!.status).toBe('pending_approval');
    expect(parsed!.scope).toBeNull();
    expect(parsed!.hunks[0].contextBefore).toEqual([]);
  });

  it('非对象载荷 → null', () => {
    expect(parseDiff(null)).toBeNull();
    expect(parseDiff('diff')).toBeNull();
    expect(parseDiff(42)).toBeNull();
  });
});

describe('pickArtifactView diff 分支', () => {
  it('kind=diff 且载荷合法 → diff 视图（审批卡数据源）', () => {
    const view = pickArtifactView(cardWith(artifact(validPayload)));
    expect(view.mode).toBe('diff');
    if (view.mode === 'diff') {
      expect(view.preview.operationId).toBe('op-1');
      expect(view.preview.hunks).toHaveLength(1);
    }
  });

  it('kind=diff 但载荷畸形 → fallbackMarkdown（不空白、不渲染半残卡）', () => {
    const view = pickArtifactView(cardWith(artifact({ broken: true }, '提案预览回退')));
    expect(view.mode).toBe('fallback');
    if (view.mode === 'fallback') expect(view.markdown).toBe('提案预览回退');
  });
});
