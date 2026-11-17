import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { selectLocal, setupStatus } from './state';

const snapshot = { selected: null, localVm: 'notIntegrated', model: 'locked' };
afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });

test.each([null, [], {}, 'SECRET_MARKER', { ...snapshot, selected: 'existingCluster' },
  { ...snapshot, model: 'ready' }, { ...snapshot, localVm: 'ready' },
  { ...snapshot, credential: 'SECRET_MARKER' }])('rejects unsupported status %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value);
  const result = await setupStatus();
  expect(result.ok).toBe(false);
  expect(JSON.stringify(result)).not.toContain('SECRET_MARKER');
});
test.each(['idle', 'preparing', 'installing', 'starting', 'ready', 'closing', 'closed', 'failed'])('accepts finite native runtime phase %s without unlocking models', async localVm => {
  vi.stubGlobal('isTauri', true);
  const value = { ...snapshot, selected: 'localVm', localVm };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
});
test.each([null, 'Starting the local VM failed: Runtime operation failed: Busy',
  'Stopping the local VM failed: Runtime operation failed: RetainedState'])('preserves native failure reason %j on reads and selection', async localVmError => {
  vi.stubGlobal('isTauri', true);
  const value = { ...snapshot, selected: 'localVm', localVm: 'failed', localVmError };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
  expect(await selectLocal()).toEqual({ ok: true, status: value });
});
test.each([42, false, {}, ['native failure']])('rejects malformed native failure reason %j', async localVmError => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...snapshot, selected: 'localVm', localVmError }));
  expect((await setupStatus()).ok).toBe(false);
  expect((await selectLocal()).ok).toBe(false);
});
test.each([false, true])('model availability requires the selected running session, with error field %j', async withError => {
  vi.stubGlobal('isTauri', true);
  const value = { selected: 'localVm', localVm: 'ready', model: 'available', ...(withError ? { localVmError: null } : {}) };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
  expect(await selectLocal()).toEqual({ ok: true, status: value });
  for (const localVm of ['starting', 'closed', 'notIntegrated']) {
    mockIPC(() => ({ ...value, localVm }));
    expect((await setupStatus()).ok).toBe(false);
  }
});
test.each([true, false])('preserves native model support %j on reads and selection', async modelSupported => {
  vi.stubGlobal('isTauri', true);
  const value = { selected: 'localVm', localVm: 'installing', model: 'locked', modelSupported };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
  expect(await selectLocal()).toEqual({ ok: true, status: value });
});
test.each(['true', 1, null, {}])('rejects malformed native model support %j', async modelSupported => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => ({ ...snapshot, selected: 'localVm', modelSupported }));
  expect((await setupStatus()).ok).toBe(false);
  expect((await selectLocal()).ok).toBe(false);
});
test('no-argument selection requires an acknowledgement of that selection', async () => {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn(() => snapshot); mockIPC(ipc);
  expect((await selectLocal()).ok).toBe(false);
  expect(ipc).toHaveBeenCalledExactlyOnceWith('setup_select_local', {});
});
test.each([null, { phase: 'materializing', bytes: 10, totalBytes: 20, elapsedMs: 1000, remainingMs: 1000 }, { phase: 'verifying', bytes: null, totalBytes: null, elapsedMs: 5, remainingMs: null }])('accepts optional native workspace progress %j', async workspaceProgress => {
  vi.stubGlobal('isTauri', true);
  const value = { selected: 'localVm', localVm: 'installing', model: 'locked', workspaceProgress };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
});
test.each([{ phase: 'materializing', bytes: -1, totalBytes: 20, elapsedMs: 1, remainingMs: null }, { phase: 'materializing', bytes: 21, totalBytes: 20, elapsedMs: 1, remainingMs: null }, { phase: 'download', bytes: 10, totalBytes: 20, elapsedMs: 1, remainingMs: null }, { phase: 'verifying', bytes: null, totalBytes: null, elapsedMs: 1, remainingMs: 'SECRET_MARKER' }])('rejects malformed native workspace progress %j', async workspaceProgress => {
  vi.stubGlobal('isTauri', true); mockIPC(() => ({ ...snapshot, workspaceProgress }));
  expect((await setupStatus()).ok).toBe(false);
});
test.each(['workspaceUpdateRequired', 'workspaceVersionUnsupported'])('preserves finite binding refusal %s only on a failed workspace', async cause => {
  vi.stubGlobal('isTauri', true);
  const value = { selected: 'localVm', localVm: 'failed', model: 'locked', workspaceFailure: { cause, operation: 'verify', codes: [] } };
  mockIPC(() => value);
  expect(await setupStatus()).toEqual({ ok: true, status: value });
  mockIPC(() => ({ ...value, localVm: 'ready', model: 'available' }));
  expect((await setupStatus()).ok).toBe(false);
});

test('IPC errors cannot leak rejected messages or claim availability', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => Promise.reject('SECRET_MARKER'));
  expect(await setupStatus()).toEqual({ ok: false, message: 'The setup choice could not be read or saved.' });
});
