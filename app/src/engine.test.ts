import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { engineVersion, listAgents } from './engine';

afterEach(() => { clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

test('requests the engine version through the typed native wire', async () => {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn(() => ({ kind: 'version', version: 'v0.4.2', hasDiagnostics: false }));
  mockIPC(ipc);
  expect(await engineVersion()).toEqual({
    ok: true, value: { kind: 'version', version: 'v0.4.2', hasDiagnostics: false },
  });
  expect(ipc).toHaveBeenCalledExactlyOnceWith('engine_call', { request: { action: { type: 'version' } } });
});

test('distinguishes a missing native bridge from the unavailable engine reply', async () => {
  const ipc = vi.fn();
  mockIPC(ipc);
  expect(await engineVersion()).toEqual({
    ok: false, error: { kind: 'unavailable', message: 'The native bridge is not available in this preview.' },
  });
  expect(ipc).not.toHaveBeenCalled();
});

test.each([
  ['unavailable', 'The engine is not available in this app session.'],
  ['unsupported', 'This engine response is not supported.'],
  ['execution', 'The engine request could not be completed.'],
  ['invalidResponse', 'The engine returned an invalid response.'],
  ['unknown', 'The engine returned an invalid response.'],
  ['toString', 'The engine returned an invalid response.'],
])('projects %s rejection into safe public copy', async (kind, message) => {
  vi.stubGlobal('isTauri', true);
  const logs = [vi.spyOn(console, 'log'), vi.spyOn(console, 'warn'), vi.spyOn(console, 'error')];
  mockIPC(() => Promise.reject({ kind, message: 'SECRET_REJECTED_MESSAGE', diagnostics: 'SECRET_DIAGNOSTICS' }));
  const result = await engineVersion();
  expect(result).toEqual({ ok: false, error: {
    kind: ['unavailable', 'unsupported', 'execution', 'invalidResponse'].includes(kind) ? kind : 'invalidResponse',
    message,
  } });
  expect(JSON.stringify(result)).not.toContain('SECRET_');
  logs.forEach(log => expect(log).not.toHaveBeenCalled());
});

test.each([null, 'SECRET_RAW_ERROR', { message: 'SECRET_UNKNOWN_ERROR' }])('ignores unstructured rejection %j', async rejection => {
  vi.stubGlobal('isTauri', true);
  mockIPC(() => Promise.reject(rejection));
  expect(await engineVersion()).toEqual({
    ok: false, error: { kind: 'invalidResponse', message: 'The engine returned an invalid response.' },
  });
});

test('lists agent identities with literal context and namespace fields', async () => {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn(() => ({
    kind: 'agents', agents: [{ name: 'helper', namespace: 'team space', uid: 'uid-1' }], hasDiagnostics: true,
  }));
  mockIPC(ipc);
  expect(await listAgents('context ; $(literal)', 'team space')).toEqual({
    ok: true, value: {
      kind: 'agents', agents: [{ name: 'helper', namespace: 'team space', uid: 'uid-1' }], hasDiagnostics: true,
    },
  });
  expect(ipc).toHaveBeenCalledExactlyOnceWith('engine_call', {
    request: { action: { type: 'listAgents', context: 'context ; $(literal)', namespace: 'team space' } },
  });
});
