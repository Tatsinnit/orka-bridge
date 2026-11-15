import { invoke, isTauri } from '@tauri-apps/api/core';

type Section<T> = { state: 'ready'; items: T[] } | { state: 'unavailable' };
export type CatalogModel = { id: string; pickerEnabled?: boolean; chatSupported?: boolean };
export type CatalogTool = { kind: 'builtin' | 'tool'; name: string; description: string; available?: boolean };
export type WorkspaceCatalogReply = { defaultModel: string | null; models: Section<CatalogModel>; tools: Section<CatalogTool> };

const bytes = (value: string) => new TextEncoder().encode(value).length;
function record(v: unknown, required: string[], optional: string[] = []): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
    && required.every(key => Object.hasOwn(v, key))
    && Object.keys(v).every(key => required.includes(key) || optional.includes(key));
}
function modelId(v: unknown): v is string {
  return typeof v === 'string' && /^[!-~]{1,256}$/.test(v) && v.toLowerCase() !== 'auto';
}
function optionalBool(v: Record<string, unknown>, key: string) {
  return !Object.hasOwn(v, key) || typeof v[key] === 'boolean';
}
function model(v: unknown): v is CatalogModel {
  return record(v, ['id'], ['pickerEnabled', 'chatSupported']) && modelId(v.id)
    && optionalBool(v, 'pickerEnabled') && optionalBool(v, 'chatSupported');
}
function tool(v: unknown): v is CatalogTool {
  return record(v, ['kind', 'name', 'description'], ['available'])
    && (v.kind === 'builtin' || v.kind === 'tool')
    && typeof v.name === 'string' && v.name.length > 0 && bytes(v.name) <= 4096
    && !/\p{Cc}/u.test(v.name)
    && typeof v.description === 'string' && bytes(v.description) <= 8192
    && !v.description.includes('\0') && optionalBool(v, 'available');
}
function section<T>(v: unknown, item: (v: unknown) => v is T, identity: (v: T) => string): v is Section<T> {
  if (!record(v, ['state'], ['items'])) return false;
  if (v.state === 'unavailable') return !Object.hasOwn(v, 'items');
  if (v.state !== 'ready' || !Array.isArray(v.items) || v.items.length > 4096 || !v.items.every(item)) return false;
  return new Set(v.items.map(identity)).size === v.items.length;
}
function catalog(v: unknown, remote: boolean): v is WorkspaceCatalogReply {
  return record(v, ['defaultModel', 'models', 'tools']) && (modelId(v.defaultModel) || remote && v.defaultModel === null)
    && section(v.models, model, m => m.id) && section(v.tools, tool, t => JSON.stringify([t.kind, t.name]))
    && bytes(JSON.stringify({ models: v.models, tools: v.tools })) <= 1024 * 1024;
}
/** Callers abort on workspace changes/unmount. No catalog survives in a cache. */
export async function readWorkspaceCatalog(signal: AbortSignal, remote?: { targetId: string; providerId: string }): Promise<WorkspaceCatalogReply | undefined> {
  const id = (value: string) => /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(value);
  if (signal.aborted || !isTauri() || remote && (!id(remote.targetId) || !id(remote.providerId))) return undefined;
  try {
    const value: unknown = remote ? await invoke('cluster_catalog', remote) : await invoke('workspace_catalog');
    if (!signal.aborted && catalog(value, !!remote)) return value;
  } catch {
    // Native/guest diagnostics and arbitrary error text never reach the renderer.
  }
  return undefined;
}
