import { invoke, isTauri } from '@tauri-apps/api/core';

export type FirstJob = { id: string; prompt: string; phase: 'setup' };

const uuid = (value: unknown) => typeof value === 'string'
  && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  && value !== '00000000-0000-0000-0000-000000000000';

export function isFirstJob(value: unknown): value is FirstJob {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    && Object.keys(value).sort().join(',') === 'id,phase,prompt'
    && 'id' in value && uuid(value.id)
    && 'prompt' in value && typeof value.prompt === 'string' && value.prompt.length > 0 && value.prompt.length <= 8192
    && 'phase' in value && value.phase === 'setup';
}

export async function beginFirstJob(prompt: string): Promise<{ ok: true; job: FirstJob } | { ok: false; message: string }> {
  if (!isTauri()) return { ok: false, message: 'Jobs can be saved only in the native app.' };
  try {
    const value = await invoke<unknown>('first_job_begin', { prompt });
    if (isFirstJob(value)) return { ok: true, job: value };
  } catch { /* Native details and paths never enter the prompt UI. */ }
  return { ok: false, message: 'The job could not be saved. Check the text and try again.' };
}

export async function discardFirstJob(id: string): Promise<{ ok: true } | { ok: false; message: string }> {
  if (!isTauri() || !uuid(id)) return { ok: false, message: 'The saved job could not be discarded.' };
  try { await invoke('first_job_discard', { id }); return { ok: true }; }
  catch { return { ok: false, message: 'The saved job could not be discarded.' }; }
}
