import { invoke, isTauri } from '@tauri-apps/api/core';
import { isPlaces, type Place } from '../overview/api';

export type ContextView = { id: string; name: string };
export type TargetView = { id: string; context_id: string; context_name: string; service_name: string; service_namespace: string; watched_namespace: string; image: string };
export type ProviderView = { id: string; name: string; ready: boolean };
export type AccessStatus = 'connected' | 'needServiceAccount' | 'denied' | 'unavailable' | 'unsupportedCredential';
export const clusterChecks = ['configuration', 'credentials', 'clusterApi', 'compatibility', 'serviceAccount', 'permissions', 'orkaIdentity', 'orkaConnection', 'agents', 'providers'] as const;
export const clusterResults = ['unknown', 'connectionFailed', 'authenticationRejected', 'permissionDenied', 'unsupportedCredential', 'incompatible', 'identityRejected', 'noProviders'] as const;
export type ClusterFailure = { check: typeof clusterChecks[number]; result: typeof clusterResults[number] };
export type AccessView = { status: AccessStatus; providers: ProviderView[]; failure: ClusterFailure | null };
export class ClusterRequestError extends Error {
  constructor(readonly failure: ClusterFailure) { super('Cluster information could not be read.'); }
}
export type SelectedPlace = { kind: 'thisComputer' } | { kind: 'cluster'; target?: TargetView; providers?: ProviderView[]; providerId?: string };
const statuses: AccessStatus[] = ['connected', 'needServiceAccount', 'denied', 'unavailable', 'unsupportedCredential'];
const object = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const keys = (v: Record<string, unknown>, names: string[]) => Object.keys(v).sort().join(',') === names.sort().join(',');
const text = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 2048 && Array.from(v).every(c => c.charCodeAt(0) >= 32 && c.charCodeAt(0) !== 127);
const id = (v: unknown): v is string => typeof v === 'string' && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(v);
const context = (v: unknown): v is ContextView => object(v) && keys(v, ['id', 'name']) && id(v.id) && text(v.name);
const target = (v: unknown): v is TargetView => object(v) && keys(v, ['id', 'context_id', 'context_name', 'service_name', 'service_namespace', 'watched_namespace', 'image'])
  && id(v.id) && id(v.context_id) && ['context_name', 'service_name', 'service_namespace', 'watched_namespace', 'image'].every(k => text(v[k]));
const provider = (v: unknown): v is ProviderView => object(v) && keys(v, ['id', 'name', 'ready']) && id(v.id) && text(v.name) && typeof v.ready === 'boolean';
const list = <T>(v: unknown, valid: (item: unknown) => item is T): v is T[] => Array.isArray(v) && v.length <= 1024 && v.every(valid);
const failure = (v: unknown): v is ClusterFailure => object(v) && keys(v, ['check', 'result'])
  && clusterChecks.includes(v.check as ClusterFailure['check']) && clusterResults.includes(v.result as ClusterFailure['result']);
export const failedCheck = (error: unknown, check: ClusterFailure['check']): ClusterFailure => error instanceof ClusterRequestError ? error.failure : { check, result: 'unknown' };
async function call<T>(command: string, args: Record<string, unknown>, valid: (v: unknown) => v is T, check: ClusterFailure['check']): Promise<T> {
  if (!isTauri()) throw Error('Cluster access is available in the desktop app.');
  try { const v: unknown = await invoke(command, args); if (valid(v)) return v; } catch (error) {
    if (failure(error)) throw new ClusterRequestError(error);
  }
  throw new ClusterRequestError({ check, result: 'unknown' });
}
export type ClusterPlaceSnapshot = { places: Place[]; selectedKey: string | null; persistenceState: 'complete' | 'unavailable' };
// Unlike Overview's refresh, this reads only the native session's retained views.
export async function clusterPlaces(): Promise<ClusterPlaceSnapshot | undefined> {
  if (!isTauri()) return;
  try {
    const value: unknown = await invoke('cluster_places', {});
    if (!object(value) || !keys(value, ['places', 'selectedKey', 'persistenceState']) || (value.persistenceState !== 'complete' && value.persistenceState !== 'unavailable')) return;
    const retained = { places: value.places, historyState: 'complete', unobserved: [] };
    if (isPlaces(retained) && (value.selectedKey === null || typeof value.selectedKey === 'string' && retained.places.some(p => p.key === value.selectedKey))) {
      return { places: retained.places, selectedKey: value.selectedKey, persistenceState: value.persistenceState };
    }
  } catch { /* Passive observation never exports native diagnostics or renews access. */ }
}
export const reconnectCluster = (key: string): Promise<TargetView> => call('cluster_reconnect', { key }, target, 'configuration');
export async function forgetCluster(key: string): Promise<void> {
  await call('cluster_forget', { key }, (v): v is null => v === null, 'configuration');
}
export const contexts = () => call('cluster_contexts', {}, (v): v is ContextView[] => list(v, context), 'configuration');
export const reloadContexts = () => call('cluster_reload', {}, (v): v is ContextView[] => list(v, context), 'configuration');
export const discover = (contextId: string) => call('cluster_discover', { contextId }, (v): v is TargetView[] => list(v, target) && v.every(t => t.context_id === contextId), 'clusterApi');
export const connectCluster = (targetId: string, saName: string | null) => call('cluster_use', { targetId, saName }, (v): v is AccessView => object(v) && keys(v, ['status', 'providers', 'failure'])
  && statuses.includes(v.status as AccessStatus) && list(v.providers, provider) && (v.status === 'connected' ? v.providers.length > 0 : v.providers.length === 0)
  && (v.status === 'connected' || v.status === 'needServiceAccount' ? v.failure === null : failure(v.failure)), 'orkaConnection');
