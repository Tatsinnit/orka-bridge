import { invoke, isTauri } from '@tauri-apps/api/core';

export type VersionReply = { kind: 'version'; version: string; hasDiagnostics: boolean };

export type AgentSummary = { name: string; namespace: string; uid: string };
export type AgentsReply = { kind: 'agents'; agents: AgentSummary[]; hasDiagnostics: boolean };
type EngineAction = { type: 'version' } | { type: 'listAgents'; context: string; namespace: string };

const errorMessages = {
  unavailable: 'The engine is not available in this app session.',
  unsupported: 'This engine response is not supported.',
  execution: 'The engine request could not be completed.',
  invalidResponse: 'The engine returned an invalid response.',
} as const;
type EngineErrorKind = keyof typeof errorMessages;
export type EngineError = { kind: EngineErrorKind; message: string };
export type EngineResult<T> = { ok: true; value: T } | { ok: false; error: EngineError };

function safeError(rejection: unknown): EngineError {
  const kind = typeof rejection === 'object' && rejection !== null && 'kind' in rejection
    ? rejection.kind : undefined;
  const safeKind = typeof kind === 'string' && Object.hasOwn(errorMessages, kind)
    ? kind as EngineErrorKind : 'invalidResponse';
  return { kind: safeKind, message: errorMessages[safeKind] };
}

async function call<T>(action: EngineAction): Promise<EngineResult<T>> {
  if (!isTauri()) {
    return { ok: false, error: { kind: 'unavailable', message: 'The native bridge is not available in this preview.' } };
  }
  try {
    // Rust validates and projects successful wire replies before they reach the renderer.
    const value = await invoke<T>('engine_call', { request: { action } });
    return { ok: true, value };
  } catch (rejection) {
    return { ok: false, error: safeError(rejection) };
  }
}

export function engineVersion() {
  return call<VersionReply>({ type: 'version' });
}

export function listAgents(context: string, namespace: string) {
  return call<AgentsReply>({ type: 'listAgents', context, namespace });
}
