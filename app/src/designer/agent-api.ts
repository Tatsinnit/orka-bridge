import { invoke, isTauri } from '@tauri-apps/api/core';
import type { ExamplePair } from '../setup/state';

export const SAMPLE_DOCUMENT_ID = '9b2b93c4-375c-4815-9717-39852a96e411';
export const isDocumentId = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(value);
export type DocumentRef = { documentId: string; name: string };
export function isDocumentRef(value: unknown): value is DocumentRef {
  return typeof value === 'object' && value !== null && Object.keys(value).sort().join(',') === 'documentId,name'
    && 'documentId' in value && isDocumentId(value.documentId) && 'name' in value && typeof value.name === 'string' && /^[a-z0-9][a-z0-9-]{0,62}$/.test(value.name);
}
export type SaveState = 'saved' | 'dirty' | 'saving' | 'error';
export const REPO_SAVE_CONFLICT = 'The agent file changed on disk. Your draft is still here. Copy it from File view and reconcile both versions in your editor before restarting Bridge.';
export type SourceId = { agent: string; tests: string };
export const storageCopy = {
  bridge: 'Saved on this computer with local Git history. Nothing is uploaded.',
  repo: 'Files save automatically. You manage commits in this repo.',
};
export type AgentPair = ExamplePair & { documentId?: string; name?: string; source?: SourceId; path?: string; storage?: keyof typeof storageCopy };
export function isSourceId(value: unknown): value is SourceId {
  return typeof value === 'object' && value !== null && Object.keys(value).sort().join(',') === 'agent,tests'
    && 'agent' in value && typeof value.agent === 'string' && /^[a-f0-9]{64}$/.test(value.agent)
    && 'tests' in value && typeof value.tests === 'string' && /^[a-f0-9]{64}$/.test(value.tests);
}
export function sameSource(left?: SourceId, right?: SourceId | null): boolean | undefined {
  return left && right ? left.agent === right.agent && left.tests === right.tests : undefined;
}
export async function agentCall(command: 'agent_open' | 'agent_save', content?: string, documentId = SAMPLE_DOCUMENT_ID, expectedSource?: SourceId): Promise<AgentPair> {
  if (!isTauri()) throw Error('Open the desktop app to edit your agent.');
  try {
    const value = await invoke<unknown>(command, content === undefined ? { documentId } : { documentId, content, ...(expectedSource ? { expectedSource } : {}) });
    if (typeof value === 'object' && value !== null && !Array.isArray(value)
      && ['agent,documentId,name,tests', 'agent,documentId,name,source,tests', 'agent,documentId,name,path,tests', 'agent,documentId,name,path,source,tests'].includes(Object.keys(value).filter(key => key !== 'storage').sort().join(','))
      && (!('storage' in value) || value.storage === 'bridge' || value.storage === 'repo')
      && 'documentId' in value && value.documentId === documentId && isDocumentId(value.documentId)
      && 'name' in value && typeof value.name === 'string' && /^[a-z0-9][a-z0-9-]{0,62}$/.test(value.name)
      && (!('path' in value) || (typeof value.path === 'string' && value.path.length > 0 && value.path.length <= 32768 && !value.path.includes('\0')))
      && (!('source' in value) || isSourceId(value.source))
      && 'agent' in value && typeof value.agent === 'string'
      && 'tests' in value && typeof value.tests === 'string' && value.tests.length > 0
      && new TextEncoder().encode(value.agent).length <= 1048576
      && new TextEncoder().encode(value.tests).length <= 1048576) return { documentId: value.documentId, name: value.name, agent: value.agent, tests: value.tests, ...('source' in value ? { source: value.source as SourceId } : {}), ...('path' in value ? { path: value.path as string } : {}), ...('storage' in value ? { storage: value.storage as keyof typeof storageCopy } : {}) };
  } catch (failure) {
    // Only this known conflict is actionable; never display arbitrary native error details.
    if (command === 'agent_save' && (failure === 'The agent file changed on disk.' || failure instanceof Error && failure.message === 'The agent file changed on disk.')) throw Error(REPO_SAVE_CONFLICT, { cause: failure });
  }
  throw Error(command === 'agent_open' ? 'Your agent folder could not be opened.' : 'Your changes could not be saved. Your draft is still here.');
}

// Only display unambiguous single-line front matter. The raw editor also accepts
// unfinished YAML; unsupported metadata is omitted, never guessed from the prompt.
export function agentInfo(source?: string): { name?: string; description?: string } {
  const header = source?.match(/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/)?.[1];
  function scalar(key: string) {
    const text = header?.match(new RegExp(`^${key}: +(.+)$`, 'm'))?.[1]?.trim();
    if (!text || /^[>|[\]{&*!#]/.test(text)) return undefined;
    if (text.startsWith('"')) { try { const parsed: unknown = JSON.parse(text); return typeof parsed === 'string' ? parsed : undefined; } catch { return undefined; } }
    if (text.startsWith("'")) return text.endsWith("'") ? text.slice(1, -1).replaceAll("''", "'") : undefined;
    return text.split(/\s+#/)[0];
  }
  const name = scalar('name');
  return { name: name && /^[a-z0-9][a-z0-9-]*$/.test(name) ? name : undefined, description: scalar('description') };
}
