import { create } from 'zustand';
import type { DiffPreviewPayload } from '../services/agentToolCards';
import {
  documentApplyPatch,
  documentProjectPatches,
  documentRejectPatch,
  documentUndoPatch,
  type ApplyResult,
  type ProjectPatchEntry,
} from '../services/tauri';
import {
  acceptedHunkIndexes,
  changePhaseFromStatus,
  initialHunkDecisions,
  resolveHunkReview,
  type ChangeSessionPhase,
  type HunkDecision,
  type HunkDecisionTarget,
} from '../services/changeSession';
import type { RunContext } from './agentStore';
import type { EditorViewCheckpoint } from '../editor/viewCheckpoint';
import { fnv1aHex } from '../editor/selection/fnv1a';

export interface ChangeSession {
  operationId: string;
  projectId: string;
  documentId: string;
  threadId: string | null;
  runId: string | null;
  preview: DiffPreviewPayload;
  phase: ChangeSessionPhase;
  decisions: HunkDecision[];
  pendingAction: 'apply' | 'reject' | 'undo' | null;
  error: string | null;
  appliedVersion: number | null;
  revisionId: string | null;
  undoable: boolean;
  undoUnavailableReason: string | null;
  checkpoint: EditorViewCheckpoint | null;
  continuationContext: RunContext | null;
  replacedByOperationId: string | null;
  createdAt: number;
  updatedAt: number;
}

export type ChangeResolution =
  | { kind: 'pending'; session: ChangeSession }
  | { kind: 'applied'; session: ChangeSession; result: ApplyResult; approvedHunks: number[] }
  | { kind: 'rejected'; session: ChangeSession }
  | { kind: 'conflict'; session: ChangeSession; error: string };

interface ProposalMeta {
  projectId: string;
  threadId?: string | null;
  runId?: string | null;
  context?: RunContext | null;
  createdAt?: number;
}

interface ChangeSessionState {
  sessions: Record<string, ChangeSession>;
  activeOperationByDocument: Record<string, string>;
  loadingProjects: Record<string, boolean>;
  loadProject: (projectId: string) => Promise<void>;
  hydrateProject: (projectId: string, patches: ProjectPatchEntry[]) => void;
  adoptProposal: (preview: DiffPreviewPayload, meta: ProposalMeta) => Promise<void>;
  decideHunk: (
    operationId: string,
    target: HunkDecisionTarget,
    decision: Exclude<HunkDecision, 'pending'>,
    checkpoint?: EditorViewCheckpoint | null
  ) => Promise<ChangeResolution | null>;
  undoOperation: (operationId: string) => Promise<ApplyResult | null>;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function previewOfPatch(patch: ProjectPatchEntry): DiffPreviewPayload {
  return {
    operationId: patch.operationId,
    documentId: patch.documentId,
    title: patch.title,
    baseVersion: patch.baseVersion,
    targetVersion: patch.targetVersion,
    oldText: patch.oldText,
    newText: patch.newText,
    hunks: patch.hunks,
    status: patch.status,
    scope: patch.scope,
    rebased: patch.rebased,
    proposedTitle: patch.proposedTitle ?? null,
  };
}

/** 单篇文档当前仍可操作的会话（排除 rejected）中最近更新者；无则 null。 */
function activeOperationForDocument(
  sessions: Record<string, ChangeSession>,
  documentId: string
): string | null {
  let best: ChangeSession | null = null;
  for (const session of Object.values(sessions)) {
    if (session.documentId !== documentId || session.phase === 'rejected') continue;
    if (!best || session.updatedAt > best.updatedAt) best = session;
  }
  return best ? best.operationId : null;
}

/** 重算单篇文档的 active 映射项；无可操作会话时删除键（状态条随之消失）。 */
function withActiveForDocument(
  active: Record<string, string>,
  sessions: Record<string, ChangeSession>,
  documentId: string
): Record<string, string> {
  const next = { ...active };
  const operationId = activeOperationForDocument(sessions, documentId);
  if (operationId) next[documentId] = operationId;
  else delete next[documentId];
  return next;
}

function newestActiveByDocument(sessions: Record<string, ChangeSession>): Record<string, string> {
  const active: Record<string, string> = {};
  const documentIds = new Set(Object.values(sessions).map((session) => session.documentId));
  for (const documentId of documentIds) {
    const operationId = activeOperationForDocument(sessions, documentId);
    if (operationId) active[documentId] = operationId;
  }
  return active;
}

export const useChangeSessionStore = create<ChangeSessionState>()((set, get) => ({
  sessions: {},
  activeOperationByDocument: {},
  loadingProjects: {},

  loadProject: async (projectId) => {
    if (get().loadingProjects[projectId]) return;
    set((state) => ({ loadingProjects: { ...state.loadingProjects, [projectId]: true } }));
    try {
      get().hydrateProject(projectId, await documentProjectPatches(projectId));
      // 异常退出可能发生在“新提案已落库、旧提案尚未来得及 reject”之间。
      // 重启恢复时每篇文档只保留最新会话，其余 pending 立即从统一状态源关闭，
      // 并尽力同步后端，避免历史审批卡重新出现。
      const snapshot = get();
      const superseded = Object.values(snapshot.sessions).filter((session) =>
        session.projectId === projectId &&
        session.phase === 'proposed' &&
        snapshot.activeOperationByDocument[session.documentId] !== session.operationId
      );
      if (superseded.length > 0) {
        set((state) => {
          const sessions = { ...state.sessions };
          for (const session of superseded) {
            sessions[session.operationId] = {
              ...sessions[session.operationId],
              phase: 'rejected',
              decisions: sessions[session.operationId].decisions.map(() => 'rejected'),
              replacedByOperationId: state.activeOperationByDocument[session.documentId] ?? null,
              updatedAt: Date.now(),
            };
          }
          return { sessions };
        });
        await Promise.allSettled(superseded.map((session) => documentRejectPatch(session.operationId)));
      }
    } catch (error) {
      console.warn('[changes] failed to hydrate project patches:', error);
    } finally {
      set((state) => ({ loadingProjects: { ...state.loadingProjects, [projectId]: false } }));
    }
  },

  hydrateProject: (projectId, patches) => {
    set((state) => {
      const sessions = { ...state.sessions };
      for (const patch of patches) {
        const persistedPhase = changePhaseFromStatus(patch.opStatus || patch.status);
        const existing = sessions[patch.operationId];
        // 本地 applying 是点击到后端审计刷新之间的真实过渡态，不能被旧查询结果倒退。
        const phase = existing?.phase === 'applying' ? existing.phase : persistedPhase;
        sessions[patch.operationId] = {
          operationId: patch.operationId,
          projectId,
          documentId: patch.documentId,
          threadId: existing?.threadId ?? null,
          runId: existing?.runId ?? null,
          preview: previewOfPatch(patch),
          phase,
          decisions: existing?.phase === 'applying'
            ? existing.decisions
            : initialHunkDecisions(patch.hunks.length, persistedPhase, patch.appliedHunks),
          pendingAction: existing?.phase === 'applying' ? existing.pendingAction : null,
          error: patch.error ?? existing?.error ?? null,
          appliedVersion: persistedPhase === 'applied' ? patch.targetVersion : existing?.appliedVersion ?? null,
          revisionId: existing?.revisionId ?? null,
          undoable: patch.undoable ?? existing?.undoable ?? false,
          undoUnavailableReason: patch.undoUnavailableReason ?? null,
          checkpoint: existing?.checkpoint ?? null,
          continuationContext: existing?.continuationContext ?? null,
          replacedByOperationId: existing?.replacedByOperationId ?? null,
          createdAt: patch.createdAt,
          updatedAt: Math.max(existing?.updatedAt ?? 0, patch.createdAt),
        };
      }
      return { sessions, activeOperationByDocument: newestActiveByDocument(sessions) };
    });
  },

  adoptProposal: async (preview, meta) => {
    set((state) => {
      const now = meta.createdAt ?? Date.now();
      const previousId = state.activeOperationByDocument[preview.documentId];
      const previous = previousId ? state.sessions[previousId] : null;
      const existing = state.sessions[preview.operationId];
      const persistedPhase = changePhaseFromStatus(preview.status);
      const keepsTerminal = existing && ['applied', 'rejected', 'undone'].includes(existing.phase);
      const phase = keepsTerminal ? existing.phase : persistedPhase;
      const sessions = { ...state.sessions };

      const isNewerThanActive = !previous || now >= previous.createdAt;
      if (
        previous &&
        previous.operationId !== preview.operationId &&
        previous.phase === 'proposed' &&
        isNewerThanActive
      ) {
        sessions[previous.operationId] = {
          ...previous,
          phase: 'rejected',
          decisions: previous.decisions.map(() => 'rejected'),
          replacedByOperationId: preview.operationId,
          updatedAt: now,
        };
      }

      sessions[preview.operationId] = {
        operationId: preview.operationId,
        projectId: meta.projectId,
        documentId: preview.documentId,
        threadId: meta.threadId ?? existing?.threadId ?? null,
        runId: meta.runId ?? existing?.runId ?? null,
        preview,
        phase,
        decisions: existing?.decisions ?? initialHunkDecisions(preview.hunks.length, phase),
        pendingAction: existing?.pendingAction ?? null,
        error: existing?.error ?? null,
        appliedVersion: existing?.appliedVersion ?? null,
        revisionId: existing?.revisionId ?? null,
        undoable: existing?.undoable ?? false,
        undoUnavailableReason: existing?.undoUnavailableReason ?? null,
        checkpoint: existing?.checkpoint ?? null,
        continuationContext: meta.context ?? existing?.continuationContext ?? null,
        replacedByOperationId: existing?.replacedByOperationId ?? null,
        createdAt: existing?.createdAt ?? now,
        updatedAt: now,
      };
      return {
        sessions,
        activeOperationByDocument: isNewerThanActive || previous?.operationId === preview.operationId
          ? { ...state.activeOperationByDocument, [preview.documentId]: preview.operationId }
          : state.activeOperationByDocument,
      };
    });

    const replaced = Object.values(get().sessions).find(
      (session) => session.replacedByOperationId === preview.operationId
    );
    if (replaced) {
      try {
        await documentRejectPatch(replaced.operationId);
      } catch (error) {
        // 新提案仍是当前事实；旧提案清理由后续审计刷新重试，不让它重新占据编辑器。
        console.warn('[changes] failed to close replaced proposal:', error);
      }
    }
  },

  decideHunk: async (operationId, target, decision, checkpoint = null) => {
    const before = get().sessions[operationId];
    if (
      !before ||
      before.phase !== 'proposed' ||
      (target !== 'all' && (target < 0 || target >= before.decisions.length))
    ) return null;
    set((state) => {
      const session = state.sessions[operationId];
      if (
        !session ||
        session.phase !== 'proposed' ||
        (target !== 'all' && (target < 0 || target >= session.decisions.length))
      ) {
        return state;
      }
      const decisions = resolveHunkReview(session.decisions, target, decision).decisions;
      const shouldResolve = decisions.every((value) => value !== 'pending');
      const approvedHunks = acceptedHunkIndexes(decisions);
      const snapshot: ChangeSession = {
        ...session,
        decisions,
        phase: shouldResolve ? 'applying' : 'proposed',
        pendingAction: shouldResolve ? (approvedHunks.length > 0 ? 'apply' : 'reject') : null,
        checkpoint: checkpoint ?? session.checkpoint,
        error: null,
        updatedAt: Date.now(),
      };
      return { sessions: { ...state.sessions, [operationId]: snapshot } };
    });
    const snapshot = get().sessions[operationId];
    if (!snapshot) return null;
    const shouldResolve = snapshot.decisions.every((value) => value !== 'pending');
    const approvedHunks = acceptedHunkIndexes(snapshot.decisions);
    if (!shouldResolve) return { kind: 'pending', session: snapshot };

    try {
      if (approvedHunks.length === 0) {
        await documentRejectPatch(operationId);
        let resolved!: ChangeSession;
        set((state) => {
          resolved = {
            ...state.sessions[operationId],
            phase: 'rejected',
            pendingAction: null,
            updatedAt: Date.now(),
          };
          const sessions = { ...state.sessions, [operationId]: resolved };
          // AG-32R2：拒绝关闭当前提案后重算该文档的可操作会话——
          // 上一条仍处于 applied 的 operation 重新成为状态条当前项，撤销入口不丢失。
          // 是否真的可撤销由后端版本闸门最终裁定（undo_patch 复检版本），前端只负责暴露入口。
          return {
            sessions,
            activeOperationByDocument: withActiveForDocument(
              state.activeOperationByDocument,
              sessions,
              resolved.documentId
            ),
          };
        });
        return { kind: 'rejected', session: resolved };
      }

      const count = snapshot.preview.hunks.length;
      const result = await documentApplyPatch(
        operationId,
        approvedHunks.length === count ? undefined : approvedHunks
      );
      let resolved!: ChangeSession;
      set((state) => {
        resolved = {
          ...state.sessions[operationId],
          phase: 'applied',
          pendingAction: null,
          appliedVersion: result.version,
          revisionId: result.revisionId,
          undoable: true,
          undoUnavailableReason: null,
          updatedAt: Date.now(),
        };
        return { sessions: { ...state.sessions, [operationId]: resolved } };
      });
      return { kind: 'applied', session: resolved, result, approvedHunks };
    } catch (error) {
      const reason = messageOf(error);
      let resolved!: ChangeSession;
      set((state) => {
        resolved = {
          ...state.sessions[operationId],
          phase: 'conflict',
          pendingAction: null,
          error: reason,
          undoable: false,
          undoUnavailableReason: reason,
          updatedAt: Date.now(),
        };
        return { sessions: { ...state.sessions, [operationId]: resolved } };
      });
      return { kind: 'conflict', session: resolved, error: reason };
    }
  },

  undoOperation: async (operationId) => {
    const current = get().sessions[operationId];
    if (!current || current.phase !== 'applied' || !current.undoable) return null;
    set((state) => ({
      sessions: {
        ...state.sessions,
        [operationId]: {
          ...state.sessions[operationId],
          phase: 'applying',
          pendingAction: 'undo',
          error: null,
        },
      },
    }));
    try {
      const result = await documentUndoPatch(operationId);
      set((state) => ({
        sessions: {
          ...state.sessions,
          [operationId]: {
            ...state.sessions[operationId],
            phase: 'undone',
            pendingAction: null,
            appliedVersion: result.version,
            undoable: false,
            undoUnavailableReason: '本次 Agent 修改已经撤销',
            updatedAt: Date.now(),
          },
        },
      }));
      return result;
    } catch (error) {
      const reason = messageOf(error);
      set((state) => ({
        sessions: {
          ...state.sessions,
          [operationId]: {
            ...state.sessions[operationId],
            phase: 'applied',
            pendingAction: null,
            error: reason,
            undoable: false,
            undoUnavailableReason: reason,
            updatedAt: Date.now(),
          },
        },
      }));
      return null;
    }
  },
}));

export function activeChangeSession(
  state: Pick<ChangeSessionState, 'sessions' | 'activeOperationByDocument'>,
  documentId: string | null | undefined
): ChangeSession | null {
  if (!documentId) return null;
  const operationId = state.activeOperationByDocument[documentId];
  return operationId ? state.sessions[operationId] ?? null : null;
}

/** 后续 Query 继续绑定原文档与原 TextAnchor；没有运行快照时才从审计载荷安全回退。 */
export function continuationContextForChange(session: ChangeSession | null): RunContext | null {
  if (!session || (session.phase !== 'proposed' && session.phase !== 'conflict')) return null;
  if (session.continuationContext) return session.continuationContext;
  const selectedMarkdown = session.preview.oldText;
  if (!selectedMarkdown) return null;
  return {
    articleId: session.documentId,
    title: session.preview.title,
    baseVersion: session.preview.baseVersion,
    selectedMarkdown,
    selectedTextHash: fnv1aHex(selectedMarkdown),
    beforeContext: '',
    afterContext: '',
  };
}
