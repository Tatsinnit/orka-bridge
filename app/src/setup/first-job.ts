import { invoke, isTauri } from '@tauri-apps/api/core';

export type FirstJob = { id: string; prompt: string; phase: 'setup' };
export const firstJobPromptBytes = 8 * 1024;
const control = /\p{Cc}/u;

export function prepareFirstJobPrompt(value: string): { ok: true; prompt: string } | { ok: false; message: string } {
  const prompt = value.replaceAll('\r\n', '\n').replaceAll('\r', '\n').trim();
  if (!prompt) return { ok: false, message: 'Write what you want done first.' };
  if ([...prompt].some(character => character !== '\n' && character !== '\t' && control.test(character))) {
    return { ok: false, message: 'Remove unsupported control characters from the job.' };
  }
  if (new TextEncoder().encode(prompt).length > firstJobPromptBytes) {
    return { ok: false, message: 'This job is too long. Shorten it to 8,192 bytes or less.' };
  }
  return { ok: true, prompt };
}

const uuid = (value: unknown) => typeof value === 'string'
  && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  && value !== '00000000-0000-0000-0000-000000000000';

export function isFirstJob(value: unknown): value is FirstJob {
  if (!(typeof value === 'object' && value !== null && !Array.isArray(value)
    && Object.keys(value).sort().join(',') === 'id,phase,prompt'
    && 'id' in value && uuid(value.id)
    && 'prompt' in value && typeof value.prompt === 'string'
    && 'phase' in value && value.phase === 'setup')) return false;
  const prepared = prepareFirstJobPrompt(value.prompt);
  return prepared.ok && prepared.prompt === value.prompt;
}

export async function beginFirstJob(prompt: string): Promise<{ ok: true; job: FirstJob } | { ok: false; message: string }> {
  const prepared = prepareFirstJobPrompt(prompt);
  if (!prepared.ok) return prepared;
  if (!isTauri()) return { ok: false, message: 'Jobs can be saved only in the native app.' };
  try {
    const value = await invoke<unknown>('first_job_begin', { prompt: prepared.prompt });
    if (isFirstJob(value)) return { ok: true, job: value };
  } catch { /* Native details and paths never enter the prompt UI. */ }
  return { ok: false, message: 'The job could not be saved. Check the text and try again.' };
}

export async function discardFirstJob(id: string): Promise<{ ok: true } | { ok: false; message: string }> {
  if (!isTauri() || !uuid(id)) return { ok: false, message: 'The saved job could not be discarded.' };
  try { await invoke('first_job_discard', { id }); return { ok: true }; }
  catch { return { ok: false, message: 'The saved job could not be discarded.' }; }
}
