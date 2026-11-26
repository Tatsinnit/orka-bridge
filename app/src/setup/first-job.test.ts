import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { beginFirstJob, discardFirstJob } from './first-job';

afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });

test('begin sends only the prompt and accepts the closed native snapshot', async () => {
  vi.stubGlobal('isTauri', true);
  const job = { id: '12345678-1234-4abc-8def-123456789abc', prompt: 'Explain the API', phase: 'setup' };
  const ipc = vi.fn(() => job); mockIPC(ipc);
  expect(await beginFirstJob(job.prompt)).toEqual({ ok: true, job });
  expect(ipc).toHaveBeenCalledExactlyOnceWith('first_job_begin', { prompt: job.prompt });
});

test.each([
  null,
  { id: '12345678-1234-4abc-8def-123456789abc', prompt: 'job', phase: 'running' },
  { id: '12345678-1234-4abc-8def-123456789abc', prompt: 'job', phase: 'setup', secret: 'SECRET_MARKER' },
])('rejects malformed replies and native errors without exposing data: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value === null ? Promise.reject('SECRET_MARKER') : value);
  const result = await beginFirstJob('Keep this prompt');
  expect(result).toEqual({ ok: false, message: 'The job could not be saved. Check the text and try again.' });
  expect(JSON.stringify(result)).not.toContain('SECRET_MARKER');
});

test('browser preview never pretends that a first job was saved', async () => {
  vi.stubGlobal('isTauri', false);
  expect(await beginFirstJob('Explain the API')).toEqual({ ok: false, message: 'Jobs can be saved only in the native app.' });
});

test('discard sends only the native job identity and sanitizes failures', async () => {
  vi.stubGlobal('isTauri', true);
  const id = '12345678-1234-4abc-8def-123456789abc';
  const ipc = vi.fn(() => undefined); mockIPC(ipc);
  expect(await discardFirstJob(id)).toEqual({ ok: true });
  expect(ipc).toHaveBeenCalledExactlyOnceWith('first_job_discard', { id });
  mockIPC(() => Promise.reject('SECRET_MARKER'));
  expect(await discardFirstJob(id)).toEqual({ ok: false, message: 'The saved job could not be discarded.' });
});
