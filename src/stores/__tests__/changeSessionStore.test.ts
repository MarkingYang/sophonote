import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiffPreviewPayload } from '../../services/agentToolCards';

const mocks = vi.hoisted(() => ({
  apply: vi.fn(),
  reject: vi.fn(),
  undo: vi.fn(),
  list: vi.fn(),
}));

vi.mock('../../services/tauri', () => ({
  documentApplyPatch: mocks.apply,
  documentRejectPatch: mocks.reject,
  documentUndoPatch: mocks.undo,
  documentProjectPatches: mocks.list,
}));

import { continuationContextForChange, useChangeSessionStore } from '../changeSessionStore';
import type { ProjectPatchEntry } from '../../services/tauri';

function preview(operationId: string, created = 1): DiffPreviewPayload & { created: number } {
  return {
    operationId,
    documentId: 'doc-1',
    title: '文档',
    baseVersion: 1,
    targetVersion: 2,
    oldText: '旧一\n旧二',
    newText: '新一\n新二',
    hunks: [
      { startLine: 0, contextBefore: [], removed: ['旧一'], added: ['新一'], contextAfter: [] },
      { startLine: 1, contextBefore: [], removed: ['旧二'], added: ['新二'], contextAfter: [] },
    ],
    status: 'pending_approval',
    scope: 'selection',
    rebased: false,
    proposedTitle: null,
    created,
  };
}

function persistedPatch(operationId: string, createdAt: number): ProjectPatchEntry {
  const value = preview(operationId);
  return {
    ...value,
    approvalId: null,
    opStatus: 'proposed',
    error: null,
    appliedHunks: null,
    createdAt,
    undoable: false,
    undoUnavailableReason: null,
  };
}

describe('operation change session store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.reject.mockResolvedValue(undefined);
    mocks.apply.mockResolvedValue({ documentId: 'doc-1', version: 2, revisionId: 'rev-1', alreadyCommitted: false });
    mocks.undo.mockResolvedValue({ documentId: 'doc-1', version: 3, revisionId: 'rev-2', alreadyCommitted: false });
    mocks.list.mockResolvedValue([]);
    useChangeSessionStore.setState({
      sessions: {},
      activeOperationByDocument: {},
      loadingProjects: {},
    });
  });

  it('new proposal replaces and closes the previous proposal for the same document', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1', createdAt: 10 });
    await useChangeSessionStore.getState().adoptProposal(preview('op-2'), { projectId: 'p1', createdAt: 20 });

    expect(mocks.reject).toHaveBeenCalledWith('op-1');
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'rejected',
      replacedByOperationId: 'op-2',
    });
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-2');
  });

  it('restart recovery closes superseded pending proposals and keeps only the newest active', async () => {
    mocks.list.mockResolvedValue([
      persistedPatch('op-new', 20),
      persistedPatch('op-old', 10),
    ]);

    await useChangeSessionStore.getState().loadProject('p1');

    expect(mocks.reject).toHaveBeenCalledWith('op-old');
    expect(useChangeSessionStore.getState().sessions['op-old']).toMatchObject({
      phase: 'rejected',
      replacedByOperationId: 'op-new',
    });
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-new');
  });

  it('submits the accepted subset automatically after every hunk has a decision', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    const first = await useChangeSessionStore.getState().decideHunk('op-1', 0, 'accepted');
    expect(first?.kind).toBe('pending');
    expect(mocks.apply).not.toHaveBeenCalled();

    const second = await useChangeSessionStore.getState().decideHunk('op-1', 1, 'rejected');
    expect(second?.kind).toBe('applied');
    expect(mocks.apply).toHaveBeenCalledWith('op-1', [0]);
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'applied',
      decisions: ['accepted', 'rejected'],
      undoable: true,
      revisionId: 'rev-1',
    });
  });

  it('bulk-decides only the remaining pending regions', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'rejected');
    const result = await useChangeSessionStore.getState().decideHunk('op-1', 'all', 'accepted');

    expect(result?.kind).toBe('applied');
    expect(mocks.apply).toHaveBeenCalledWith('op-1', [1]);
    expect(useChangeSessionStore.getState().sessions['op-1'].decisions)
      .toEqual(['rejected', 'accepted']);
  });

  it('bulk-rejects every remaining region without writing', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    const result = await useChangeSessionStore.getState().decideHunk('op-1', 'all', 'rejected');

    expect(result?.kind).toBe('rejected');
    expect(mocks.reject).toHaveBeenCalledWith('op-1');
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(useChangeSessionStore.getState().sessions['op-1'].decisions)
      .toEqual(['rejected', 'rejected']);
  });

  it('rejects without writing when every region is declined', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'rejected');
    const result = await useChangeSessionStore.getState().decideHunk('op-1', 1, 'rejected');

    expect(result?.kind).toBe('rejected');
    expect(mocks.reject).toHaveBeenCalledWith('op-1');
    expect(mocks.apply).not.toHaveBeenCalled();
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'rejected',
      decisions: ['rejected', 'rejected'],
    });
  });

  it('clears the document active entry when its only proposal is rejected', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-1');

    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'rejected');
    await useChangeSessionStore.getState().decideHunk('op-1', 1, 'rejected');

    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBeUndefined();
  });

  it('re-exposes the previous applied operation after rejecting its successor (AG-32R2)', async () => {
    // 第一条提案被完整接受 → applied + undoable
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1', createdAt: 10 });
    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'accepted');
    await useChangeSessionStore.getState().decideHunk('op-1', 1, 'accepted');
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'applied',
      undoable: true,
    });

    // 连续追问产生新提案 → 成为文档 active operation，旧 applied 不再是当前状态条
    await useChangeSessionStore.getState().adoptProposal(preview('op-2'), { projectId: 'p1', createdAt: 20 });
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-2');

    // 全 × 拒绝新提案 → 上一条仍可撤销的 applied operation 重新成为当前会话（撤销入口不丢失）
    await useChangeSessionStore.getState().decideHunk('op-2', 0, 'rejected');
    await useChangeSessionStore.getState().decideHunk('op-2', 1, 'rejected');

    expect(useChangeSessionStore.getState().sessions['op-2'].phase).toBe('rejected');
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-1');
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'applied',
      undoable: true,
    });
  });

  it('keeps the newest actionable session active when several remain after a rejection (AG-32R2)', async () => {
    // op-1 applied（较早），op-2 applied（较晚，updatedAt 更大）
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1', createdAt: 10 });
    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'accepted');
    await useChangeSessionStore.getState().decideHunk('op-1', 1, 'accepted');
    await useChangeSessionStore.getState().adoptProposal(preview('op-2'), { projectId: 'p1', createdAt: 20 });
    await useChangeSessionStore.getState().decideHunk('op-2', 0, 'accepted');
    await useChangeSessionStore.getState().decideHunk('op-2', 1, 'accepted');
    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-2');

    // 钉死两条 applied 的更新时序（避免同毫秒抖动）：op-2 晚于 op-1
    useChangeSessionStore.setState((state) => ({
      sessions: {
        ...state.sessions,
        'op-1': { ...state.sessions['op-1'], updatedAt: 100 },
        'op-2': { ...state.sessions['op-2'], updatedAt: 200 },
      },
    }));

    // 新提案 op-3 被全拒 → active 回到最近的 applied（op-2），而不是更早的 op-1
    await useChangeSessionStore.getState().adoptProposal(preview('op-3'), { projectId: 'p1', createdAt: 30 });
    await useChangeSessionStore.getState().decideHunk('op-3', 0, 'rejected');
    await useChangeSessionStore.getState().decideHunk('op-3', 1, 'rejected');

    expect(useChangeSessionStore.getState().activeOperationByDocument['doc-1']).toBe('op-2');
  });

  it('undoes the exact operation checkpoint and exposes the terminal state', async () => {
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), { projectId: 'p1' });
    await useChangeSessionStore.getState().decideHunk('op-1', 0, 'accepted');
    await useChangeSessionStore.getState().decideHunk('op-1', 1, 'accepted');
    const result = await useChangeSessionStore.getState().undoOperation('op-1');

    expect(result?.version).toBe(3);
    expect(mocks.undo).toHaveBeenCalledWith('op-1');
    expect(useChangeSessionStore.getState().sessions['op-1']).toMatchObject({
      phase: 'undone',
      undoable: false,
    });
  });

  it('keeps the original selection anchor for a follow-up query', async () => {
    const context = {
      articleId: 'doc-1',
      title: '文档',
      baseVersion: 1,
      selectedMarkdown: '旧一',
      selectedTextHash: 'anchor-hash',
      beforeContext: '之前',
      afterContext: '之后',
    };
    await useChangeSessionStore.getState().adoptProposal(preview('op-1'), {
      projectId: 'p1',
      context,
    });

    expect(continuationContextForChange(useChangeSessionStore.getState().sessions['op-1'])).toEqual(context);
  });
});
