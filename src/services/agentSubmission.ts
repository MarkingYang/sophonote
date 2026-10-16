import { invoke, type Channel } from '@tauri-apps/api/core';
import type { AgentAttachmentInput, AgentEngine, AgentEvent, FocusDocumentInput, RunContext } from '../stores/agentStore';
import type { WorkspacePermissionMode } from './workspaceBinding';

export type SubmissionState = 'accepted' | 'preparing' | 'waiting_credentials' | 'prepared'
  | 'dispatching' | 'running' | 'recovering' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted';

export interface SubmissionSnapshot {
  submissionId: string;
  clientRequestId: string;
  threadId: string | null;
  runId: string | null;
  requestedEngine: AgentEngine | 'auto';
  selectedEngine?: AgentEngine | null;
  state: SubmissionState;
  generation: number;
  cancelRequested: boolean;
  message: string | null;
}

/** Explicit engine only. Uses the existing Host model, scope, and permission checks. */
export interface ManualSubmissionRequest {
  engine: AgentEngine;
  message: string;
  threadId?: string | null;
  projectId?: string | null;
  selection?: RunContext | null;
  focusDocument?: FocusDocumentInput | null;
  attachments?: Omit<AgentAttachmentInput, 'id'>[];
  skill?: string | null;
  hermesModel?: string | null;
  hermesProvider?: string | null;
  hermesCommand?: string | null;
  includeProjectContext?: boolean;
  workspaceRoot?: string | null;
  workspacePermissionMode?: WorkspacePermissionMode;
  maxTurns?: number | null;
  readOnlyCollaboration?: boolean;
}

export interface ManualSubmission {
  readonly clientRequestId: string;
  readonly request: ManualSubmissionRequest;
}

/** Generate once per user submission; retries must reuse the returned envelope. */
export function createManualSubmission(request: ManualSubmissionRequest): ManualSubmission {
  const timestamp = Date.now();
  if (!Number.isSafeInteger(timestamp) || timestamp < 0 || timestamp >= 2 ** 48) {
    throw new Error('系统时钟无效，请校准后再发送');
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  let value = timestamp;
  for (let index = 5; index >= 0; index -= 1) {
    bytes[index] = value % 256;
    value = Math.floor(value / 256);
  }
  bytes[6] = (bytes[6] & 0x0f) | 0x70;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  const clientRequestId = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  const envelope = { clientRequestId, request: structuredClone(request) };
  // Keep the ID tied to the original intent even if Composer data is later edited.
  const freeze = (item: unknown): void => {
    if (item && typeof item === 'object') {
      Object.values(item).forEach(freeze);
      Object.freeze(item);
    }
  };
  freeze(envelope);
  return envelope;
}

interface Response<T> { success: boolean; data: T | null; error: string | null }
export class SubmissionRejected extends Error {}
async function call(command: string, args: Record<string, unknown>): Promise<SubmissionSnapshot> {
  const result = await invoke<Response<SubmissionSnapshot>>(command, args);
  if (!result.success) throw new SubmissionRejected(result.error ?? '提交被拒绝');
  if (result.data == null) throw new Error('无法读取提交状态');
  return result.data;
}

/** Replaying a request never reattaches its Channel. Use runId and existing Run event replay. */
export function startManualSubmission(submission: ManualSubmission, onEvent: Channel<AgentEvent>): Promise<SubmissionSnapshot> {
  return call('agent_submission_start', { request: submission, onEvent });
}
export function getSubmission(submissionId: string): Promise<SubmissionSnapshot> {
  return call('agent_submission_get', { submissionId });
}
/** cancelRequested alone is not a terminal acknowledgement. Keep observing the returned state. */
export function cancelSubmission(submissionId: string): Promise<SubmissionSnapshot> {
  return call('agent_submission_cancel', { submissionId });
}

export interface AutomaticSubmission {
  readonly clientRequestId: string;
  readonly request: Omit<ManualSubmissionRequest, 'engine'>;
}
export function createAutomaticSubmission(request: AutomaticSubmission['request']): AutomaticSubmission {
  // Shared UUID implementation; engine is removed before the intent is frozen.
  const manual = createManualSubmission({ ...request, engine: 'hermes' });
  const { engine: _engine, ...automatic } = manual.request;
  return Object.freeze({ clientRequestId: manual.clientRequestId, request: Object.freeze(automatic) });
}
export function startAutomaticSubmission(submission: AutomaticSubmission, onEvent: Channel<AgentEvent>): Promise<SubmissionSnapshot> {
  return call('agent_submission_auto_start', { request: submission, onEvent });
}
export function findSubmission(clientRequestId: string): Promise<SubmissionSnapshot> {
  return call('agent_submission_find', { clientRequestId });
}
export const submissionScopeKey = (threadId: string | null, projectId?: string | null, _articleId?: string | null) =>
  threadId ?? `draft:${projectId ?? ''}`;
export const submissionTerminal = (state: SubmissionState) =>
  ['succeeded', 'failed', 'cancelled', 'interrupted'].includes(state);

export interface PendingSubmission {
  clientRequestId: string;
  snapshot: SubmissionSnapshot | null;
  cancelRequested: boolean;
  error?: string;
}
const pendingKey = 'sophonote.pending-submissions.v1';
export function readPendingSubmissions(): Record<string, PendingSubmission> {
  try {
    const data = JSON.parse(localStorage.getItem(pendingKey) ?? '{}');
    return Object.fromEntries(Object.entries(data).filter(([, value]) => {
      const entry = value as PendingSubmission;
      return typeof entry?.clientRequestId === 'string' && /^[0-9a-f-]{36}$/.test(entry.clientRequestId);
    })) as Record<string, PendingSubmission>;
  } catch { return {}; }
}
export function savePendingSubmissions(pending: Record<string, PendingSubmission>): void {
  // Persist only recovery identities, never input, credentials, or attachment bodies.
  localStorage.setItem(pendingKey, JSON.stringify(pending));
}

/** Only a Host-confirmed no-Run terminal can be offered for editing/resubmission. */
export const recoverableSubmission = (pending?: PendingSubmission) => Boolean(
  pending?.snapshot && !pending.snapshot.runId && ['failed', 'cancelled', 'interrupted'].includes(pending.snapshot.state),
);
export async function readSubmissionDraft(clientRequestId: string, discard = false): Promise<AutomaticSubmission['request'] | null> {
  const result = await invoke<Response<AutomaticSubmission['request']>>('agent_submission_draft', { clientRequestId, discard });
  if (!result.success) throw new Error(result.error ?? '无法读取草稿');
  return result.data;
}

/** Attachment IDs are view-local; compare the actual user-selected references. */
export function canRestoreDraftAttachments(current: AgentAttachmentInput[], saved: AutomaticSubmission['request']['attachments']): boolean {
  const key = (item: Omit<AgentAttachmentInput, 'id'>) => JSON.stringify([item.kind, item.path ?? null, item.url ?? null, item.name]);
  const original = new Set((saved ?? []).map(key));
  return current.every((item) => original.has(key(item)));
}
