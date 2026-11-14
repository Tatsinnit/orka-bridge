import { invoke, isTauri } from '@tauri-apps/api/core';
import { isDocumentId } from './agent-api';
export const chatStates = ['ready', 'preparing', 'working', 'waitingForInput', 'waitingForSession', 'observationLost', 'requestingStop', 'stopRequested', 'stopUnconfirmed', 'contextUnavailable'] as const;
export const chatErrors = {
  invalidMessage: 'Write a shorter message before trying again.',
  busy: 'The local cluster is busy. Wait for the current test or conversation, then try again.',
  source: 'Check your saved agent file before trying again.',
  model: 'Connect the AI in Clusters and check the model in your agent file.',
  connection: 'The connection was lost. Reconnect to check what happened; do not resend the message.',
  identity: 'The conversation could not be verified. Start a new conversation when available.',
  response: 'The conversation could not be read. Reconnect to check what happened.',
  contextUnavailable: 'Earlier messages could not be confirmed. Start a new conversation when available.',
  workspaceChanged: 'The local cluster changed. Your next message starts a new conversation.',
  interrupted: 'The turn was interrupted. Execution may still be running.',
} as const;
export type ChatTurn = { message: string; answer: string | null; phase: string; tools: { name: string; callId: string; phase: string }[]; contextRetained: boolean; terminationUnconfirmed: boolean; stopUnconfirmed: boolean };
export type ChatSnapshot = { documentId: string; state: typeof chatStates[number]; busy: boolean; canSend: boolean; versions: { turns: ChatTurn[] }[]; error: keyof typeof chatErrors | null };
export type ChatCommand = 'agent_chat_status' | 'agent_chat_send' | 'agent_chat_reconnect' | 'agent_chat_stop' | 'agent_chat_new';
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
export function isChatSnapshot(value: unknown): value is ChatSnapshot {
  return object(value) && isDocumentId(value.documentId) && chatStates.includes(value.state as ChatSnapshot['state'])
    && typeof value.busy === 'boolean' && typeof value.canSend === 'boolean'
    && (value.error === null || typeof value.error === 'string' && Object.hasOwn(chatErrors, value.error))
    && Array.isArray(value.versions) && value.versions.every(version => object(version) && Array.isArray(version.turns) && version.turns.every(turn =>
      object(turn) && typeof turn.message === 'string' && (turn.answer === null || typeof turn.answer === 'string') && typeof turn.phase === 'string'
      && ['contextRetained', 'terminationUnconfirmed', 'stopUnconfirmed'].every(key => typeof turn[key] === 'boolean')
      && Array.isArray(turn.tools) && turn.tools.every(tool => object(tool) && typeof tool.name === 'string' && typeof tool.callId === 'string' && typeof tool.phase === 'string')));
}
export async function chatCall(command: ChatCommand, documentId: string, message?: string): Promise<{ ok: true; snapshot: ChatSnapshot } | { ok: false; message: string; uncertain?: true }> {
  if (!isTauri()) return { ok: false, message: 'Chat is available only in the native app.' };
  try {
    const value = await invoke<unknown>(command, { documentId, ...(command === 'agent_chat_send' ? { message } : {}) });
    if (isChatSnapshot(value) && value.documentId === documentId) return { ok: true, snapshot: value };
  } catch (error) {
    if (typeof error === 'string' && Object.hasOwn(chatErrors, error)) return { ok: false, message: chatErrors[error as keyof typeof chatErrors] };
  }
  return { ok: false, uncertain: true, message: 'Chat information could not be confirmed. Reconnect to check again without resending.' };
}
