import { invoke } from '@tauri-apps/api/core';
import { createManualSubmission } from './agentSubmission';
import type { AgentThread } from '../stores/agentStore';

export interface ResponseTurn {
  id: string; conversationId: string; message: string; answer: string | null;
  state: 'preparing' | 'running' | 'completed' | 'failed' | 'cancelled' | 'interrupted';
  provider: string; model: string; error: string | null; createdAt: number;
  usage: { prompt_tokens: number; completion_tokens: number; total_tokens: number } | null;
}
export interface TextRequest {
  message: string; threadId?: string | null; projectId?: string | null;
  hermesProvider: string; hermesModel: string; workspaceRoot?: string | null;
}
class ResponseRejected extends Error {}
interface Result<T> { success: boolean; data: T; error: string | null }
export async function responseCall<T>(command: string, args: Record<string, unknown>): Promise<T> {
  const result = await invoke<Result<T>>(command, args);
  if (!result) throw new Error('无法读取文本响应，请重试');
  if (!result.success) throw new ResponseRejected(result.error ?? '文本响应提交被拒绝');
  return result.data;
}
export const isResponseConversation = (id?: string | null) => id?.startsWith('response-') === true;
export const responseActive = (turn: ResponseTurn) => turn.state === 'preparing' || turn.state === 'running';

/** Persist only a fingerprint and UUID before IPC; an ambiguous return must not produce a second request. */
export async function startTextResponse(request: TextRequest): Promise<ResponseTurn | null> {
  const canonical = JSON.stringify({ ...request, threadId: request.threadId ?? null, projectId: request.projectId ?? null });
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(canonical));
  const key = `sophonote.response.pending.${Array.from(new Uint8Array(digest), v => v.toString(16).padStart(2, '0')).join('')}`;
  let id = localStorage.getItem(key);
  if (!id) {
    id = createManualSubmission({ engine: 'pi', message: '' }).clientRequestId;
    localStorage.setItem(key, id);
  }
  // No catch-and-fallback: an unknown IPC outcome may already have called the model.
  try {
    const turn = await responseCall<ResponseTurn | null>('light_response_start', { clientRequestId: id, request });
    localStorage.removeItem(key);
    return turn;
  } catch (error) {
    if (error instanceof ResponseRejected) localStorage.removeItem(key);
    throw error;
  }
}
export function listTextResponses(projectId: string | null, scope: string) {
  return responseCall<AgentThread[]>('light_response_list', { projectId, scope });
}
export function responseAction(conversationId: string, projectId: string | null, action: string, value: string | null = null) {
  return responseCall<boolean>('light_response_action', { conversationId, projectId, action, value });
}
