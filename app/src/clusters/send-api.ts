import { invoke, isTauri } from '@tauri-apps/api/core';
import { isDocumentRef, isDocumentId, SAMPLE_DOCUMENT_ID, isSourceId, type DocumentRef, type SourceId } from '../designer/agent-api';
import { isRunSnapshot, type RunSnapshot } from '../designer/run-api';

export type Cancellation = 'cancelling' | 'requested' | 'confirmed' | 'unconfirmed' | 'denied' | 'unsupported';
const isCancellation = (v: unknown) => v === null || ['cancelling', 'requested', 'confirmed', 'unconfirmed', 'denied', 'unsupported'].includes(v as string);
export type SendView = Pick<RunSnapshot, 'acknowledged' | 'pending' | 'report'> & {
  documentId: string; id: string; targetId: string; providerId: string; targetName: string | null;
  source: SourceId | null; model: string | null; agentName: string | null;
  state: 'running' | 'complete' | 'unknown';
  stage: 'capture' | 'access' | 'preflight' | 'submit' | 'submitRejected' | 'storage' | 'observe' | 'stopped' | 'done'
    | 'updateConflict' | 'updateUnsupported' | 'updateMissing' | 'updateDenied' | 'updateInvalid' | 'updateUnconfirmed' | 'earlierWork' | 'legacyDestination' | 'destinationMismatch';
  error: string | null; storageError: boolean; cancellation?: Cancellation | null;
  agentAction?: 'create' | 'update' | 'unchanged' | null;
};
const uuid = (v: unknown) => typeof v === 'string' && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(v);
const text = (v: unknown) => v === null || typeof v === 'string' && v.length > 0 && v.length <= 4096;
export const sendErrors = {
  capture: 'Your saved agent or check files changed, or could not be read. Check this destination again before sending.',
  access: 'Cluster access could not be confirmed. Choose your cluster again in Clusters.',
  preflight: 'The cluster, tools or AI connection could not be confirmed. Check your cluster and try again.',
  storage: 'Nothing was sent because Bridge could not save a summary. Check local storage before trying again.',
  submit: 'Sending was not confirmed. Named resources may still exist. Nothing was retried.',
  submitRejected: 'Orka did not accept the resource. Check cluster access and try again.',
  observe: 'The result could not be confirmed. The cluster may still be running checks.',
  stopped: 'Stopped watching. The cluster may still be running checks.',
  updateConflict: 'This agent changed outside Bridge. Your files are unchanged.',
  updateUnsupported: "This Orka can't safely update this agent",
  updateMissing: 'The linked agent is no longer available on this cluster. Your files are unchanged.',
  updateDenied: 'Cannot update this agent with this cluster identity.',
  updateInvalid: 'Orka did not accept the agent update. Your files are unchanged.',
  updateUnconfirmed: 'Outcome not confirmed. Check this cluster before trying again. Nothing was retried.',
  destinationMismatch: 'The Orka destination could not be verified. Bridge did not send the Orka credential.',
  legacyDestination: 'Saved ownership predates destination verification. Bridge cannot reuse it or create a duplicate agent.',
  earlierWork: 'An earlier Send is not finished or its outcome is not confirmed. Check its status or cancel its tests before updating this agent.',
} as const;
function valid(v: unknown): v is SendView {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) return false;
  const row = v as Record<string, unknown>;
  return Object.keys(row).filter(key => key !== 'cancellation' && key !== 'agentAction').sort().join(',') === 'acknowledged,agentName,documentId,error,id,model,pending,providerId,report,source,stage,state,storageError,targetId,targetName'
    && (!('cancellation' in row) || isCancellation(row.cancellation))
    && (!('agentAction' in row) || row.agentAction === null || ['create', 'update', 'unchanged'].includes(row.agentAction as string))
    && typeof row.storageError === 'boolean'
    && isDocumentId(row.documentId) && uuid(row.id) && uuid(row.targetId) && uuid(row.providerId) && text(row.targetName)
    && (row.source === null || isSourceId(row.source)) && text(row.model) && text(row.agentName)
    && ['running', 'complete', 'unknown'].includes(row.state as string)
    && (row.stage === 'done' || typeof row.stage === 'string' && Object.hasOwn(sendErrors, row.stage))
    && (row.error === null || typeof row.error === 'string' && Object.hasOwn(sendErrors, row.error))
    && isRunSnapshot({ state: row.state, canRun: false, report: row.report, acknowledged: row.acknowledged, pending: row.pending, error: null });
}
export type SendCheck = {
  documentId: string; source: SourceId | null; targetId: string; providerId: string; model: string | null;
  rows: { label: string; status: 'allowed' | 'blocked' | 'cannotCheck' | 'unverified'; message: string }[];
  canSend: boolean;
  action?: 'create' | 'update' | 'unchanged' | 'blocked'; canSeparate?: boolean;
};
function isCheck(v: unknown): v is SendCheck {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) return false;
  const row = v as Record<string, unknown>;
  return Object.keys(row).filter(key => key !== 'action' && key !== 'canSeparate').sort().join(',') === 'canSend,documentId,model,providerId,rows,source,targetId'
    && ((!('action' in row) && !('canSeparate' in row))
      || ['create', 'update', 'unchanged', 'blocked'].includes(row.action as string) && typeof row.canSeparate === 'boolean'
        && (row.action !== 'blocked' || row.canSend === false))
    && isDocumentId(row.documentId) && uuid(row.targetId) && uuid(row.providerId) && text(row.model)
    && (row.source === null || isSourceId(row.source))
    && Array.isArray(row.rows) && row.rows.length > 0 && row.rows.length <= 1024
    && row.rows.every(r => r && typeof r === 'object' && Object.keys(r).sort().join(',') === 'label,message,status'
      && typeof r.label === 'string' && text(r.label) && typeof r.message === 'string' && text(r.message)
      && ['allowed', 'blocked', 'cannotCheck', 'unverified'].includes(r.status))
    && typeof row.canSend === 'boolean'
    && (!row.canSend || row.source !== null && row.model !== null && row.rows.every(r => r.status === 'allowed' || r.status === 'unverified'));
}
export async function sendPreflight(targetId: string, providerId: string, documentId = SAMPLE_DOCUMENT_ID): Promise<SendCheck> {
  if (isTauri()) {
    try {
      const value: unknown = await invoke('send_preflight', { documentId, targetId, providerId });
      if (isCheck(value) && value.documentId === documentId && value.targetId === targetId && value.providerId === providerId) return value;
    } catch { /* Do not forward native diagnostics. */ }
  }
  throw Error('Destination checks could not be read. Check your cluster and try again.');
}
export type AttemptSummary = {
  id: string; source: SourceId; targetDisplay: string;
  targetUids: { clusterIdentity?: string; serviceNamespace: string; serviceUid: string; deploymentNamespace: string; deploymentUid: string; watchedNamespace: string };
  provider: string; plannedAgentName: string; agentUid: string | null;
  verdict: 'running' | 'passed' | 'failed' | 'unknown' | 'cancelled'; time: number;
};
export function isSummary(value: unknown): value is AttemptSummary {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const row = value as Record<string, unknown>;
  const identity = row.targetUids;
  return Object.keys(row).sort().join(',') === 'agentUid,id,plannedAgentName,provider,source,targetDisplay,targetUids,time,verdict'
    && uuid(row.id) && isSourceId(row.source)
    && [row.targetDisplay, row.provider, row.plannedAgentName].every(v => v !== null && text(v))
    && text(row.agentUid)
    && ['running', 'passed', 'failed', 'unknown', 'cancelled'].includes(row.verdict as string)
    && typeof row.time === 'number' && Number.isSafeInteger(row.time) && row.time >= 0 && row.time <= 253402300799
    && typeof identity === 'object' && identity !== null && !Array.isArray(identity)
    && Object.keys(identity).filter(key => key !== 'clusterIdentity').sort().join(',') === 'deploymentNamespace,deploymentUid,serviceNamespace,serviceUid,watchedNamespace'
    && (!('clusterIdentity' in identity) || typeof identity.clusterIdentity === 'string' && /^[a-f0-9]{64}$/.test(identity.clusterIdentity))
    && Object.values(identity).every(v => v !== null && text(v));
}
export async function sendHistory(): Promise<AttemptSummary[]> {
  if (isTauri()) {
    try {
      const value: unknown = await invoke('send_history');
      if (Array.isArray(value) && value.length <= 1024 && value.every(isSummary)) return value;
    } catch { /* Stored bytes and diagnostics are not display data. */ }
  }
  throw Error('Saved Send summaries could not be read. They are unavailable, not passing results.');
}
export type SendHistoryDetail = {
  id: string; documentId: string | null; model: string | null; capturedAt: number | null;
  report: RunSnapshot['report']; interrupted: boolean; canCheck: boolean; canRemove: boolean;
  cancellation?: Cancellation | null; canCancel?: boolean;
  detailState: 'saved' | 'legacy' | 'omitted' | 'removed';
  availability: 'available' | 'missing' | 'unavailable' | null;
};
function isHistoryDetail(value: unknown): value is SendHistoryDetail {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return false;
  const row = value as Record<string, unknown>;
  return Object.keys(row).filter(key => key !== 'cancellation' && key !== 'canCancel').sort().join(',') === 'availability,canCheck,canRemove,capturedAt,detailState,documentId,id,interrupted,model,report'
    && (!('cancellation' in row) || isCancellation(row.cancellation))
    && (!('canCancel' in row) || typeof row.canCancel === 'boolean')
    && uuid(row.id) && (row.documentId === null || isDocumentId(row.documentId)) && text(row.model)
    && (row.capturedAt === null || typeof row.capturedAt === 'number' && Number.isSafeInteger(row.capturedAt) && row.capturedAt >= 0 && row.capturedAt <= 253402300799)
    && ['interrupted', 'canCheck', 'canRemove'].every(key => typeof row[key] === 'boolean')
    && ['saved', 'legacy', 'omitted', 'removed'].includes(row.detailState as string)
    && (row.availability === null || ['available', 'missing', 'unavailable'].includes(row.availability as string))
    && isRunSnapshot({ state: 'unknown', canRun: false, report: row.report, acknowledged: [], pending: null, error: null });
}
export async function sendHistoryDetail(command: 'send_history_details' | 'send_history_check' | 'send_history_remove' | 'send_history_cancel', sendId: string): Promise<SendHistoryDetail> {
  if (isTauri() && uuid(sendId)) {
    try {
      const value: unknown = await invoke(command, { sendId });
      if (isHistoryDetail(value) && value.id === sendId) return value;
    } catch { /* Neither stored content nor native diagnostics is an error message. */ }
  }
  throw Error(command === 'send_history_cancel'
    ? 'Cancellation could not be confirmed. Work may still be running. Saved answers are unchanged.'
    : command === 'send_history_check'
    ? 'Cluster status could not be checked. Reconnect this cluster in Clusters and try again. Saved answers are unchanged.'
    : command === 'send_history_remove' ? 'Saved details could not be removed. Nothing on the cluster was changed.'
      : 'Saved details could not be read. The summary is still available.');
}
export type SendStatus = { documentId: string; view: SendView | null; busy: DocumentRef | null };
export async function sendCall(command: 'send_start' | 'send_status' | 'send_cancel', args: Record<string, unknown> = {}): Promise<SendStatus> {
  if (!isTauri()) throw Error('Send is available only in the desktop app.');
  const documentId = args.documentId ?? SAMPLE_DOCUMENT_ID;
  try {
    const value: unknown = await invoke(command, { ...args, documentId });
    if (typeof value === 'object' && value !== null && Object.keys(value).sort().join(',') === 'busy,documentId,view'
      && 'documentId' in value && value.documentId === documentId && isDocumentId(value.documentId)
      && 'view' in value && (value.view === null || valid(value.view) && value.view.documentId === documentId)
      && 'busy' in value && (value.busy === null || isDocumentRef(value.busy))) return value as SendStatus;
  } catch { /* No native diagnostics or credentials reach the screen. */ }
  throw Error('Send information could not be read. Nothing was retried. Reopen this view to check the result.');
}
/** Where Overview shows the agent a Send recorded; every field is native-owned, derived from that Send's own history. */
export type AgentFocus = { key: string; name: string; namespace: string; uid: string };
export type AgentCheck = 'present' | 'gone' | 'cannotCheck';
const bounded = (v: unknown) => typeof v === 'string' && v.length > 0 && v.length <= 4096;
export async function sendAgentFocus(sendId: string): Promise<AgentFocus | undefined> {
  if (!isTauri()) return undefined;
  try {
    const v: unknown = await invoke('send_agent_focus', { sendId });
    if (typeof v === 'object' && v !== null && Object.keys(v).sort().join(',') === 'key,name,namespace,uid') {
      const r = v as Record<string, unknown>;
      if (bounded(r.key) && bounded(r.name) && bounded(r.namespace) && bounded(r.uid)) return v as AgentFocus;
    }
  } catch { /* An unacknowledged Send has no agent to open. */ }
}
/** Read-only; anything but a well-formed answer is cannotCheck, never gone. */
export async function sendAgentCheck(sendId: string): Promise<AgentCheck> {
  if (isTauri()) { try { const v: unknown = await invoke('send_agent_check', { sendId }); if (v === 'present' || v === 'gone') return v; } catch { /* Not evidence of deletion. */ } }
  return 'cannotCheck';
}
