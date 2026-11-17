import { invoke, isTauri } from '@tauri-apps/api/core';
import { isPreflightFailure, type PreflightFailure } from './preflight-state';
import { isWorkspaceFailure, type WorkspaceFailure } from './recovery-info';

const localStates = ['notIntegrated', 'idle', 'preparing', 'installing', 'starting', 'ready', 'closing', 'closed', 'cancelled', 'failed'] as const;
export type LocalVmState = (typeof localStates)[number];
export type WorkspaceProgress = { phase: 'verifying' | 'materializing' | 'starting'; bytes: number | null; totalBytes: number | null; elapsedMs: number; remainingMs: number | null };
export type SetupStatus = { selected: 'localVm' | null; localVm: LocalVmState; localVmError?: string | null; model: 'locked' | 'available'; modelSupported?: boolean; preflightFailure?: PreflightFailure; workspaceFailure?: WorkspaceFailure; workspaceProgress?: WorkspaceProgress | null };
function isProgress(value: unknown): value is WorkspaceProgress | null {
  if (value === null) return true;
  if (typeof value !== 'object' || Array.isArray(value)
    || Object.keys(value).sort().join(',') !== 'bytes,elapsedMs,phase,remainingMs,totalBytes') return false;
  const p = value as Record<string, unknown>;
  const integer = (n: unknown): n is number => typeof n === 'number' && Number.isSafeInteger(n) && n >= 0;
  return (p.phase === 'verifying' || p.phase === 'materializing' || p.phase === 'starting') && integer(p.elapsedMs)
    && (p.remainingMs === null || integer(p.remainingMs))
    && ((p.bytes === null && p.totalBytes === null) || (integer(p.bytes) && integer(p.totalBytes) && p.totalBytes > 0 && p.bytes <= p.totalBytes));
}
function isLocalState(value: unknown): value is LocalVmState {
  return typeof value === 'string' && localStates.some(state => state === value);
}
type Result = { ok: true; status: SetupStatus } | { ok: false; message: string };

async function call(command: 'setup_status' | 'setup_select_local' | 'setup_cancel_local' | 'setup_change_local'): Promise<Result> {
  if (!isTauri()) return { ok: false, message: 'Workspace setup is not available in this preview.' };
  try {
    const value = await invoke<unknown>(command);
    if (typeof value !== 'object' || value === null || Array.isArray(value)
      || Object.keys(value).some(key => !['localVm', 'localVmError', 'model', 'modelSupported', 'selected', 'workspaceProgress', 'preflightFailure', 'workspaceFailure'].includes(key))
      || !('selected' in value) || (value.selected !== null && value.selected !== 'localVm')
      || !('localVm' in value) || !isLocalState(value.localVm)
      || ('localVmError' in value && value.localVmError !== null && typeof value.localVmError !== 'string')
      || ('modelSupported' in value && typeof value.modelSupported !== 'boolean')
      || ('workspaceProgress' in value && !isProgress(value.workspaceProgress))
      || ('preflightFailure' in value && (!isPreflightFailure(value.preflightFailure) || value.localVm !== 'failed'))
      || ('workspaceFailure' in value && (!isWorkspaceFailure(value.workspaceFailure) || value.localVm !== 'failed'))
      || (value.localVm === 'ready' && value.selected !== 'localVm')
      || !('model' in value) || (value.model !== 'locked' && value.model !== 'available')
      || (value.model === 'available' && value.localVm !== 'ready')
      || (command === 'setup_select_local' && value.selected !== 'localVm')) {
      return { ok: false, message: 'Setup information is not supported by this build.' };
    }
    const status: SetupStatus = { selected: value.selected, localVm: value.localVm, model: value.model };
    if ('localVmError' in value && (value.localVmError === null || typeof value.localVmError === 'string')) {
      status.localVmError = value.localVmError;
    }
    if ('modelSupported' in value && typeof value.modelSupported === 'boolean') status.modelSupported = value.modelSupported;
    if ('preflightFailure' in value && isPreflightFailure(value.preflightFailure)) status.preflightFailure = value.preflightFailure;
    if ('workspaceProgress' in value && isProgress(value.workspaceProgress)) status.workspaceProgress = value.workspaceProgress;
    if ('workspaceFailure' in value && isWorkspaceFailure(value.workspaceFailure)) status.workspaceFailure = value.workspaceFailure;
    return { ok: true, status };
  } catch {
    return { ok: false, message: 'The setup choice could not be read or saved.' };
  }
}
export const setupStatus = () => call('setup_status');
export const selectLocal = () => call('setup_select_local');
export const cancelLocal = () => call('setup_cancel_local');
export const changeLocal = () => call('setup_change_local');

export type ExamplePair = { agent: string; tests: string };
export async function loadExample(prepare = false): Promise<{ ok: true; pair: ExamplePair } | { ok: false; message: string; retryable?: boolean }> {
  if (!isTauri()) return { ok: false, message: 'The native example is not available in this preview.' };
  try {
    const value = await (prepare ? invoke<unknown>('setup_example', { prepare: true }) : invoke<unknown>('setup_example'));
    if (typeof value === 'object' && value !== null && !Array.isArray(value)
      && Object.keys(value).sort().join(',') === 'agent,tests'
      && 'agent' in value && typeof value.agent === 'string' && value.agent.length > 0 && value.agent.length <= 65536
      && 'tests' in value && typeof value.tests === 'string' && value.tests.length > 0 && value.tests.length <= 65536) {
      return { ok: true, pair: { agent: value.agent, tests: value.tests } };
    }
  } catch (error) {
    if (prepare && error === 'exampleBusy') return { ok: false, message: 'The workspace is busy.', retryable: true };
  }
  return { ok: false, message: 'The example could not be opened.' };
}
