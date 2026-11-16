import { invoke, isTauri } from '@tauri-apps/api/core';
import { unsupportedToolMessage } from './built-in-tools';
import { type DocumentRef, isDocumentId, isDocumentRef, isSourceId, SAMPLE_DOCUMENT_ID, type SourceId } from './agent-api';
export type Verdict = 'passed' | 'failed' | 'unknown' | 'cancelled';
type Assertion = { id: string; kind: 'contains' | 'toolCalled'; expected: string; verdict: Exclude<Verdict, 'cancelled'> };
type Case = { id: string; title?: string; question: string; phase: string; answer: string | null; completedTools: string[]; assertions: Assertion[]; verdict: Verdict };
type Receipt = { kind: 'Agent' | 'Task'; name: string; uid: string };
export const errors = {
  session: 'The running local VM is not available for Run.',
  unsupportedTool: unsupportedToolMessage,
  source: 'Check the format of your saved agent and check files.',
  model: 'Connect the local VM model in Clusters before running the example.',
  submit: 'Submission was not confirmed. A named resource may still exist; it was not retried.',
  observe: 'Observation did not finish. The acknowledged Task may still be running.',
} as const;
export type RunSnapshot = { attemptId?: string | null; documentId?: string; busy?: DocumentRef | null; state: 'idle' | 'running' | 'complete' | 'unknown'; canRun: boolean; modelConnected?: boolean; stage?: 'capture' | 'model' | 'submit' | 'observe' | 'cancel' | 'done' | null; canCancel?: boolean; canRetryCancellation?: boolean; canCheckCancellation?: boolean; cancellation?: 'none' | 'cancelling' | 'cancelled' | 'unconfirmed' | 'beforeStart'; model?: string | null; vendor?: 'openai' | 'anthropic' | null; source?: SourceId | null; sourceCurrent?: boolean; report: { cases: Case[]; verdict: Verdict } | null; acknowledged: Receipt[]; pending: Omit<Receipt, 'uid'> | null; error: keyof typeof errors | null };
function record(value: unknown, keys: string[]): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value) && Object.keys(value).sort().join(',') === keys.sort().join(',');
}
const text = (value: unknown, max = 4096): value is string => typeof value === 'string' && value.length > 0 && value.length <= max;
const verdict = (value: unknown): value is Verdict => value === 'passed' || value === 'failed' || value === 'unknown' || value === 'cancelled';
export function isRunSnapshot(value: unknown): value is RunSnapshot {
  const keys = ['state', 'canRun', 'report', 'acknowledged', 'pending', 'error'];
  if (typeof value === 'object' && value !== null) {
    if ('documentId' in value) keys.push('documentId');
    if ('attemptId' in value) keys.push('attemptId');
    if ('busy' in value) keys.push('busy');
    if ('source' in value) keys.push('source');
    if ('modelConnected' in value) keys.push('modelConnected');
    if ('stage' in value) keys.push('stage');
    if ('model' in value) keys.push('model');
    if ('vendor' in value) keys.push('vendor');
    for (const key of ['canCancel', 'canRetryCancellation', 'canCheckCancellation', 'cancellation']) if (key in value) keys.push(key);
  }
  if (!record(value, keys) || ('documentId' in value && !isDocumentId(value.documentId)) || ('busy' in value && value.busy !== null && !isDocumentRef(value.busy)) || ('modelConnected' in value && typeof value.modelConnected !== 'boolean') || ('source' in value && value.source !== null && !isSourceId(value.source)) || typeof value.state !== 'string' || !['idle', 'running', 'complete', 'unknown'].includes(value.state) || typeof value.canRun !== 'boolean' || value.state === 'running' && value.canRun || value.error !== null && !(typeof value.error === 'string' && Object.hasOwn(errors, value.error))) return false;
  if ('attemptId' in value && value.attemptId !== null && !isDocumentId(value.attemptId)) return false;
  for (const key of ['canCancel', 'canRetryCancellation', 'canCheckCancellation']) if (key in value && typeof value[key] !== 'boolean') return false;
  if ('cancellation' in value && !['none', 'cancelling', 'cancelled', 'unconfirmed', 'beforeStart'].includes(value.cancellation as string)) return false;
  if ('stage' in value && !(value.state === 'idle' ? value.stage === null : value.state === 'running' ? ['capture', 'model', 'submit', 'observe', 'cancel'].includes(value.stage as string) : value.stage === 'done')) return false;
  if ('model' in value && value.model !== null && !text(value.model, 1048576)) return false;
  if ('vendor' in value && value.vendor !== null && value.vendor !== 'openai' && value.vendor !== 'anthropic') return false;
  if (!Array.isArray(value.acknowledged) || value.acknowledged.length > 33 || !value.acknowledged.every(r => record(r, ['kind', 'name', 'uid']) && (r.kind === 'Agent' || r.kind === 'Task') && text(r.name) && text(r.uid))) return false;
  if (value.pending !== null && !(record(value.pending, ['kind', 'name']) && (value.pending.kind === 'Agent' || value.pending.kind === 'Task') && text(value.pending.name))) return false;
  if (value.report === null) return value.state !== 'complete';
  const report = value.report;
  return record(report, ['cases', 'verdict']) && verdict(report.verdict) && Array.isArray(report.cases) && (report.cases.length > 0 || report.verdict === 'cancelled' && (value.cancellation === 'beforeStart' || value.state === 'running' && value.cancellation === 'cancelling') || report.verdict === 'unknown' && (value.cancellation === 'unconfirmed' || value.state === 'running' && value.cancellation === 'cancelling')) && report.cases.length <= 32 && report.cases.every(c =>
    record(c, ['id', 'question', 'phase', 'answer', 'completedTools', 'assertions', 'verdict', ...(typeof c === 'object' && c !== null && 'title' in c ? ['title'] : [])]) && (!('title' in c) || typeof c.title === 'string' && c.title.length <= 1048576) && text(c.id) && text(c.question, 1048576) && text(c.phase) && verdict(c.verdict) && (c.answer === null || typeof c.answer === 'string' && c.answer.length <= 1048576)
    && Array.isArray(c.completedTools) && c.completedTools.length <= 6000 && c.completedTools.every(t => text(t))
    && Array.isArray(c.assertions) && (c.verdict === 'cancelled' ? c.phase === 'Cancelled' && c.assertions.length === 0 : c.assertions.length > 0) && c.assertions.length <= 64 && c.assertions.every(a => record(a, ['id', 'kind', 'expected', 'verdict']) && text(a.id) && text(a.expected, 1048576) && (a.kind === 'contains' || a.kind === 'toolCalled') && verdict(a.verdict) && a.verdict !== 'cancelled'));
}
export function sampleRun(snapshot?: RunSnapshot): RunSnapshot | undefined {
  return snapshot?.documentId === SAMPLE_DOCUMENT_ID ? snapshot : undefined;
}
export async function runCall(command: 'agent_run_status' | 'agent_run' | 'agent_run_cancel' | 'agent_run_check_cancellation', documentId = SAMPLE_DOCUMENT_ID, bundledExample = false, attemptId?: string): Promise<{ ok: true; snapshot: RunSnapshot } | { ok: false; message: string }> {
  if (!isTauri()) return { ok: false, message: 'Run is available only in the native app.' };
  try { const value = await invoke<unknown>(command, { documentId, ...(bundledExample ? { bundledExample: true, ...(attemptId ? { attemptId } : {}) } : {}) }); if (isRunSnapshot(value) && value.documentId === documentId && 'busy' in value) return { ok: true, snapshot: value }; } catch { /* Never expose native errors or diagnostics. */ }
  return { ok: false, message: 'Run information is not supported by this build.' };
}
