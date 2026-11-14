import { invoke, isTauri } from '@tauri-apps/api/core';
import { isDocumentId } from './agent-api';

export type AgentEntry = { id: string; name: string; path: string | null; available: boolean };
const displayPath = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && v.length <= 32768 && !v.includes('\0');
export async function listDocuments(): Promise<AgentEntry[] | undefined> {
  if (!isTauri()) return;
  try {
    const value: unknown = await invoke('agents_list');
    if (Array.isArray(value) && value.length <= 4096 && value.every(v => typeof v === 'object' && v !== null
      && isDocumentId(v.id) && typeof v.name === 'string' && /^[a-z0-9][a-z0-9-]{0,62}$/.test(v.name)
      && (v.path === null || displayPath(v.path)) && typeof v.available === 'boolean')
      && new Set(value.map(v => v.id)).size === value.length) return value;
  } catch { /* Native diagnostics are not UI text. */ }
}
export async function selectedDocument(): Promise<string | undefined> {
  if (!isTauri()) return;
  try { const id: unknown = await invoke('agent_selected'); if (isDocumentId(id)) return id; } catch { /* Retain the current editor. */ }
}
export async function selectDocument(documentId: string): Promise<void> {
  if (!isTauri()) return;
  try { await invoke('agent_select', { documentId }); return; } catch { /* Keep the current draft. */ }
  throw Error('Your agent selection could not be saved. Try choosing it again.');
}
export async function creationRoot(): Promise<string | undefined> {
  if (!isTauri()) return;
  try { const path: unknown = await invoke('agent_creation_root'); if (displayPath(path)) return path; } catch { /* Display only. */ }
}
export async function createDocument(name: string, model?: string): Promise<string> {
  if (isTauri()) {
    try {
      const id: unknown = await invoke('agent_create', { name, ...(model ? { model } : {}) });
      if (isDocumentId(id)) return id;
    } catch { /* Do not echo native paths or file contents. */ }
  }
  throw Error('Your agent could not be created. Check the name and folder, or choose another name if it already exists, then try again.');
}
