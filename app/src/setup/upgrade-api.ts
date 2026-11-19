import { invoke, isTauri } from '@tauri-apps/api/core';

const states = ['none', 'eligible', 'sourceSelected', 'sourceRetained', 'fresh', 'incomplete', 'consumed', 'restoring', 'restored', 'conflict'] as const;
const credentials = ['absent', 'eligible', 'attention', 'transferred', 'removed'] as const;
const errors = ['network', 'noSpace', 'inUse', 'stopUnconfirmed', 'forcedStop', 'integrity', 'unsupported', 'unknown'] as const;
export type UpgradeStatus = {
  state: typeof states[number]; transaction: string | null; createdAt: number | null;
  sourceReleases: string[]; targetRelease: string | null;
  retainedAllocatedBytes: number | null; additionalBytes: number | null;
  credential: typeof credentials[number]; busy: boolean; error: typeof errors[number] | null;
  repeated: boolean; canReplace: boolean; canRestore: boolean; canCancel: boolean;
};
export type UpgradeAction = 'status' | 'refresh' | 'replace' | 'retry' | 'restore' | 'transfer' | 'remove' | 'cancel';
const uuid = (value: unknown): value is string => typeof value === 'string'
  && /^[\da-f]{8}(-[\da-f]{4}){3}-[\da-f]{12}$/i.test(value) && value !== '00000000-0000-0000-0000-000000000000';
const integer = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
function decode(value: unknown): UpgradeStatus | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || Object.keys(value).sort().join(',') !== 'additionalBytes,busy,canCancel,canReplace,canRestore,createdAt,credential,error,repeated,retainedAllocatedBytes,sourceReleases,state,targetRelease,transaction') return;
  const s = value as UpgradeStatus;
  if (!states.includes(s.state) || !credentials.includes(s.credential) || (s.error !== null && !errors.includes(s.error))
    || (s.transaction !== null && !uuid(s.transaction)) || (s.createdAt !== null && !integer(s.createdAt))
    || !Array.isArray(s.sourceReleases) || s.sourceReleases.length > 32 || !s.sourceReleases.every(v => typeof v === 'string' && v.length > 0 && v.length <= 128)
    || (s.targetRelease !== null && (typeof s.targetRelease !== 'string' || !s.targetRelease.length || s.targetRelease.length > 128))
    || (s.retainedAllocatedBytes !== null && !integer(s.retainedAllocatedBytes)) || (s.additionalBytes !== null && !integer(s.additionalBytes))
    || typeof s.canCancel !== 'boolean' || typeof s.busy !== 'boolean' || typeof s.repeated !== 'boolean' || typeof s.canReplace !== 'boolean' || typeof s.canRestore !== 'boolean'
    || (!['none', 'eligible', 'conflict'].includes(s.state) && !s.transaction)
    || (s.canReplace && !['eligible', 'sourceSelected', 'sourceRetained', 'incomplete', 'restored'].includes(s.state))
    || (s.canRestore && (!s.transaction || ['none', 'eligible', 'restored', 'conflict'].includes(s.state)))
    || ((s.canReplace || s.canRestore) && (!s.sourceReleases.length || !s.targetRelease))) return;
  return s;
}
type Failure = { ok: false; message: string };
type StatusReply = { ok: true; status: UpgradeStatus };
type ForwardReply = StatusReply & { outcome: 'completed' | 'cancelled' };
type CancelReply = { ok: true; outcome: 'requested' | 'completed' | 'notCancellable' };
type ObservationAction = Exclude<UpgradeAction, 'replace' | 'retry' | 'cancel'>;
export function upgradeCall(action: 'cancel'): Promise<CancelReply | Failure>;
export function upgradeCall(action: 'replace' | 'retry', transaction?: string): Promise<ForwardReply | Failure>;
export function upgradeCall(action: ObservationAction, transaction?: string): Promise<StatusReply | Failure>;
export function upgradeCall(action: Exclude<UpgradeAction, 'cancel'>, transaction?: string): Promise<StatusReply | ForwardReply | Failure>;
export async function upgradeCall(action: UpgradeAction, transaction?: string): Promise<StatusReply | ForwardReply | CancelReply | Failure> {
  const failure = { ok: false as const, message: 'Workspace update information is unavailable. Check again before changing your workspace.' };
  if (!isTauri() || (['retry', 'restore', 'transfer', 'remove'].includes(action) && !uuid(transaction))) return failure;
  try {
    const command = `workspace_upgrade_${action === 'refresh' ? 'status' : action}`;
    const value = await (action === 'refresh' ? invoke<unknown>(command, { refresh: true })
      : transaction ? invoke<unknown>(command, { transaction }) : invoke<unknown>(command));
    if (action === 'cancel') return value === 'requested' || value === 'completed' || value === 'notCancellable' ? { ok: true, outcome: value } : failure;
    if (action === 'replace' || action === 'retry') {
      if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).sort().join(',') !== 'outcome,status') return failure;
      const reply = value as { outcome: unknown; status: unknown };
      const status = decode(reply.status);
      return status && (reply.outcome === 'completed' || reply.outcome === 'cancelled') ? { ok: true, outcome: reply.outcome, status } : failure;
    }
    const status = decode(value);
    return status ? { ok: true, status } : failure;
  } catch { return failure; }
}
