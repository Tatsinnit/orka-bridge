import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import ts from 'typescript';
import { upgradeCall } from './upgrade-api';

const transaction = '10000000-0000-4000-8000-000000000001';
const status = { state: 'eligible', transaction: null, createdAt: null, sourceReleases: ['0.1.2'], targetRelease: '0.1.3', retainedAllocatedBytes: 1200, additionalBytes: 2400, credential: 'eligible', canCancel: false, busy: false, error: null, repeated: false, canReplace: true, canRestore: false };
afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
function native(value: unknown) { vi.stubGlobal('isTauri', true); const ipc = vi.fn(() => value); mockIPC(ipc); return ipc; }

test('case-insensitive native hosts resolve the component, not the API client', () => {
  const files = Object.keys(import.meta.glob('./*.{ts,tsx}')).map(path => `/setup/${path.slice(2)}`.toLowerCase());
  const resolved = ts.resolveModuleName('./Upgrade', '/setup/Setup.tsx', { moduleResolution: ts.ModuleResolutionKind.Bundler }, {
    fileExists: path => files.includes(path.toLowerCase()), readFile: () => undefined,
  }).resolvedModule;
  expect(resolved?.resolvedFileName).toBe('/setup/Upgrade.tsx');
});
test('finite native eligibility and measured sizes survive decoding', async () => {
  native(status);
  expect(await upgradeCall('status')).toEqual({ ok: true, status });
});
test.each([
  { ...status, state: 'newerMaybe' }, { ...status, sourcePath: '/private' }, { ...status, credential: 'connected' },
  { ...status, transaction: 'not-an-id' }, { ...status, transaction: '00000000-0000-0000-0000-000000000000' },
  { ...status, retainedAllocatedBytes: -1 }, { ...status, additionalBytes: Number.MAX_SAFE_INTEGER + 1 },
  { ...status, createdAt: 1.5 }, { ...status, sourceReleases: [42] }, { ...status, busy: 1 },
  { ...status, error: 'private raw error' }, { ...status, canRestore: true },
  { ...status, state: 'conflict' }, { ...status, state: 'incomplete', transaction: null },
])('invalid or contradictory native replies never enable replacement: %j', async value => {
  native(value);
  expect((await upgradeCall('status')).ok).toBe(false);
});
test('cached observation and explicit owner refresh use the same command with distinct arguments', async () => {
  const ipc = native(status);
  expect(await upgradeCall('status')).toEqual({ ok: true, status });
  expect(await upgradeCall('refresh')).toEqual({ ok: true, status });
  expect(ipc.mock.calls).toEqual([['workspace_upgrade_status', {}], ['workspace_upgrade_status', { refresh: true }]]);
});
test.each(['replace', 'retry'] as const)('%s requires an exact tagged forward result', async action => {
  for (const outcome of ['completed', 'cancelled'] as const) {
    native({ outcome, status });
    expect(await upgradeCall(action, action === 'retry' ? transaction : undefined)).toEqual({ ok: true, outcome, status });
  }
});
test.each([
  status, null, [], {}, { outcome: 'requested', status }, { outcome: 'completed' },
  { outcome: 'completed', status, extra: true }, { outcome: 'completed', status: { ...status, canCancel: 'yes' } },
  { outcome: 'completed', status: { ...status, error: 'raw error' } },
])('forward decoder rejects malformed tags and nested status: %j', async value => {
  native(value);
  expect((await upgradeCall('replace')).ok).toBe(false);
  expect((await upgradeCall('retry', transaction)).ok).toBe(false);
});
test.each(['requested', 'completed', 'notCancellable'] as const)('cancel accepts only bare finite %s', async outcome => {
  const ipc = native(outcome);
  expect(await upgradeCall('cancel')).toEqual({ ok: true, outcome });
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_cancel', {});
});
test.each([status, { outcome: 'requested' }, 'cancelled', 'Completed', null, false])('cancel rejects non-contract replies: %j', async value => {
  native(value);
  expect((await upgradeCall('cancel')).ok).toBe(false);
});
test.each(['status', 'refresh', 'restore', 'transfer', 'remove'] as const)('%s rejects a forward wrapper instead of plain status', async action => {
  native({ outcome: 'completed', status });
  expect((await upgradeCall(action, ['status', 'refresh'].includes(action) ? undefined : transaction)).ok).toBe(false);
});
test.each([undefined, null, 'true', 1])('canCancel is a required boolean: %j', async canCancel => {
  native({ ...status, canCancel });
  expect((await upgradeCall('status')).ok).toBe(false);
});
test('forced stop is finite and busy does not imply cancellable', async () => {
  const value = { ...status, busy: true, canCancel: false, error: 'forcedStop' };
  native(value);
  expect(await upgradeCall('status')).toEqual({ ok: true, status: value });
});
test('recovery sends only the exact transaction, never a path or slot', async () => {
  const ipc = native({ ...status, state: 'consumed', transaction, createdAt: 1700000000, canReplace: false, canRestore: true });
  expect((await upgradeCall('restore', transaction)).ok).toBe(true);
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_restore', { transaction });
});
test('missing or malformed transaction is refused before IPC', async () => {
  const ipc = native(status);
  expect((await upgradeCall('restore')).ok).toBe(false);
  expect((await upgradeCall('transfer', '../other')).ok).toBe(false);
  expect(ipc).not.toHaveBeenCalled();
});
