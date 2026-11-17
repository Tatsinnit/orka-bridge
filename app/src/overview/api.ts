import { invoke, isTauri } from '@tauri-apps/api/core';
import { type AttemptSummary, isSummary } from '../clusters/send-api';
import { isDocumentId, isSourceId, type SourceId } from '../designer/agent-api';
export type RecordedAttempt = AttemptSummary & { documentId?: string | null; detailState?: 'saved' | 'legacy' | 'omitted' | 'removed' | 'unavailable'; report?: { verdict: 'passed' | 'failed'; passed: number; total: number } | null };
export type RemoteAgent = Agent & { agentType: string | null; model: string | null; tools: string[] | null; record: RecordedAttempt | null };
export type Place = { key: string; context: string; service: string; namespace: string; stale: boolean; observedAt: number | null; checkedAt?: number | null; agents: RemoteAgent[] };
export type Places = { places: Place[]; historyState: 'complete' | 'unavailable'; unobserved: AttemptSummary[] };
export type SavedFile = { documentId: string; name: string; draft?: boolean; state: 'readable' | 'unreadable'; source: SourceId | null; summary: { name: string | null; description: string | null; model: string; tools: string[] } | null; reason: string | null; localResult: Observation | null };
// Identity of an Agent Bridge itself created for a local test: installation + namespace + UID, never a name.
export type RunAgent = { installation: string; namespace: string; agentUid: string };
export type SavedFiles = { state: 'complete' | 'unavailable'; files: SavedFile[]; runActive: boolean; historyState: 'complete' | 'unavailable'; runAgents: RunAgent[] };
export type Agent = { name: string; namespace: string; uid: string };
export type Observation = { installation: string; documentId: string; source: SourceId; namespace: string; agentName: string; agentUid: string; observedAt: number; state: 'running' | 'complete' | 'unknown'; report: { verdict: 'passed' | 'failed' | 'unknown' | 'cancelled'; passed: number; total: number } | null };
export type LiveAgents = { state: 'complete' | 'busy' | 'unavailable'; installation: string | null; agents: Agent[]; observedAt: number | null; observation: Observation | null; runActive: boolean; reason: 'image' | 'stopped' | 'read' | null };
const object = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 1048576;
const nullableText = (v: unknown) => v === null || typeof v === 'string' && v.length <= 1048576;
const time = (v: unknown) => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
function summary(v: unknown) {
  return v === null || object(v) && nullableText(v.name) && nullableText(v.description) && text(v.model) && Array.isArray(v.tools) && v.tools.every(text);
}
function observation(v: unknown) {
  return v === null || object(v) && isDocumentId(v.installation) && isDocumentId(v.documentId) && isSourceId(v.source) && text(v.namespace) && text(v.agentName) && text(v.agentUid) && time(v.observedAt)
    && ['running', 'complete', 'unknown'].includes(String(v.state)) && (v.report === null || object(v.report) && ['passed', 'failed', 'unknown', 'cancelled'].includes(String(v.report.verdict)) && count(v.report.passed) && count(v.report.total) && v.report.passed <= v.report.total);
}
const count = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
const runAgent = (v: unknown) => object(v) && isDocumentId(v.installation) && text(v.namespace) && text(v.agentUid);
// Native sends historyState, runAgents and localResult together. Older payloads omit all three, which cannot prove
// which Agents are test runs or what the last result was, so the reader normalises them to "unavailable".
export type RawSavedFiles = Omit<SavedFiles, 'historyState' | 'runAgents' | 'files'> & Partial<Pick<SavedFiles, 'historyState' | 'runAgents'>> & { files: (Omit<SavedFile, 'localResult'> & { localResult?: Observation | null })[] };
export function isSavedFiles(v: unknown): v is RawSavedFiles {
  if (!object(v) || !['complete', 'unavailable'].includes(String(v.state)) || typeof v.runActive !== 'boolean' || !Array.isArray(v.files)) return false;
  if (v.historyState !== undefined && v.historyState !== 'complete' && v.historyState !== 'unavailable') return false;
  if (v.runAgents !== undefined && !(Array.isArray(v.runAgents) && v.runAgents.every(runAgent) && (v.historyState === 'complete' || v.runAgents.length === 0))) return false;
  return v.files.every(f => object(f) && isDocumentId(f.documentId) && text(f.name) && ['readable', 'unreadable'].includes(String(f.state)) && (f.source === null || isSourceId(f.source)) && summary(f.summary) && nullableText(f.reason)
    && (f.draft === undefined || typeof f.draft === 'boolean')
    && (f.localResult === undefined || f.localResult === null || observation(f.localResult) && (f.localResult as Observation).documentId === f.documentId));
}
export function isLiveAgents(v: unknown): v is LiveAgents {
  return object(v) && ['complete', 'busy', 'unavailable'].includes(String(v.state)) && typeof v.runActive === 'boolean' && (v.installation === null || isDocumentId(v.installation)) && (v.observedAt === null || time(v.observedAt)) && (v.reason === null || v.reason === 'image' || v.reason === 'stopped' || v.reason === 'read') && observation(v.observation)
    && Array.isArray(v.agents) && v.agents.every(a => object(a) && text(a.name) && text(a.namespace) && text(a.uid))
    && (v.state !== 'complete' || v.installation !== null && v.observedAt !== null);
}
function isRecord(v: unknown): v is RecordedAttempt {
  if (!object(v)) return false;
  const { documentId, detailState, report, ...base } = v;
  if (!isSummary(base)) return false;
  if (documentId === undefined && detailState === undefined && report === undefined) return true;
  if (!(documentId === null || isDocumentId(documentId)) || !['saved', 'legacy', 'omitted', 'removed', 'unavailable'].includes(String(detailState))) return false;
  if ((detailState === 'legacy' || detailState === 'removed' || detailState === 'unavailable') && documentId !== null) return false;
  return report === null || detailState === 'saved' && object(report) && ['passed', 'failed'].includes(String(report.verdict))
    && report.verdict === base.verdict && count(report.passed) && count(report.total) && report.total > 0 && report.total <= 32 && report.passed <= report.total;
}
export function isPlaces(v: unknown): v is Places {
  if (!object(v) || !['complete', 'unavailable'].includes(String(v.historyState)) || !Array.isArray(v.places) || !Array.isArray(v.unobserved) || !v.unobserved.every(isSummary)) return false;
  const keys = new Set<string>();
  return v.places.every(p => {
    if (!object(p) || !text(p.key) || keys.has(p.key) || !text(p.context) || !text(p.service) || !text(p.namespace) || typeof p.stale !== 'boolean'
      || !(p.observedAt === null && p.stale || time(p.observedAt)) || !(p.checkedAt === undefined || p.checkedAt === null || time(p.checkedAt)) || !Array.isArray(p.agents)) return false;
    keys.add(p.key);
    const uids = new Set<string>();
    return p.agents.every(a => {
      if (!object(a) || !text(a.name) || a.namespace !== p.namespace || !text(a.uid) || uids.has(a.uid) || !nullableText(a.agentType) || !nullableText(a.model)
        || !(a.tools === null || Array.isArray(a.tools) && a.tools.every(text)) || !(a.record === null || isRecord(a.record) && a.record.agentUid === a.uid && a.record.targetUids.watchedNamespace === p.namespace)) return false;
      uids.add(a.uid); return true;
    });
  });
}
export async function readPlaces(key?: string): Promise<Places | undefined> {
  if (isTauri()) { try { const v: unknown = await invoke('overview_places', key === undefined ? {} : { key }); if (isPlaces(v)) return v; } catch { /* No native diagnostics reach Overview. */ } }
}
export const unavailableFiles: SavedFiles = { state: 'unavailable', files: [], runActive: false, historyState: 'unavailable', runAgents: [] };
export const unavailableAgents: LiveAgents = { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'read' };
export async function readFiles(): Promise<SavedFiles> {
  if (isTauri()) { try {
    const v: unknown = await invoke('overview_files');
    if (isSavedFiles(v)) return { ...v, historyState: v.historyState ?? 'unavailable', runAgents: v.runAgents ?? [], files: v.files.map(f => ({ ...f, localResult: f.localResult ?? null })) };
  } catch { /* Do not project native diagnostics. */ } }
  return unavailableFiles;
}
export async function readAgents(): Promise<LiveAgents> {
  if (isTauri()) { try { const v: unknown = await invoke('overview_agents'); if (isLiveAgents(v)) return v; } catch { /* Do not project native diagnostics. */ } }
  return unavailableAgents;
}
