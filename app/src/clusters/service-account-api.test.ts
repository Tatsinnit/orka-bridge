import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { serviceAccounts, previewServiceAccount } from './api';
const targetId = '00000000-0000-4000-8000-000000000001';
const checks = [['agents', 'create'], ['agents', 'get'], ['agents', 'list'], ['tasks', 'create'], ['tasks', 'get'], ['tasks', 'list'], ['providers', 'get'], ['providers', 'list'], ['tools', 'get'], ['tools', 'list']].map(([resource, verb]) => ({ resource, verb, status: 'allowed' }));
afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
test('account IPC sends only target/name and validates finite full replies', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(command => command === 'cluster_service_accounts' ? { names: ['team-runner'], truncated: false } : { status: 'allowed', checks }); mockIPC(ipc);
  await expect(serviceAccounts(targetId)).resolves.toEqual({ names: ['team-runner'], truncated: false });
  await expect(previewServiceAccount(targetId, 'team-runner')).resolves.toEqual({ status: 'allowed', checks });
  expect(ipc.mock.calls).toEqual([['cluster_service_accounts', { targetId }], ['cluster_service_account_preview', { targetId, saName: 'team-runner' }]]);
});
test('Kubernetes subdomain names allow long parts but retain the total bound', async () => {
  vi.stubGlobal('isTauri', true);
  for (const name of ['a'.repeat(64), `${'a'.repeat(64)}.runner`, 'a'.repeat(253)]) {
    mockIPC(() => ({ names: [name], truncated: false }));
    await expect(serviceAccounts(targetId)).resolves.toEqual({ names: [name], truncated: false });
  }
  mockIPC(() => ({ names: ['a'.repeat(254)], truncated: false }));
  await expect(serviceAccounts(targetId)).rejects.toThrow();
});

test.each([
  { names: ['team-runner', 'team-runner'], truncated: false }, { names: ['bad/name'], truncated: false },
  { names: ['team'], truncated: false, token: 'PRIVATE' }, { names: Array(1001).fill('team'), truncated: true },
])('invalid account list fails closed: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value); await expect(serviceAccounts(targetId)).rejects.toThrow();
});
test.each([
  { status: 'allowed', checks: [] }, { status: 'allowed', checks: checks.map(c => ({ ...c, status: 'notChecked' })) },
  { status: 'denied', checks }, { status: 'unknown', checks },
  { status: 'allowed', checks: [...checks.slice(1), checks[1]] },
  { status: 'allowed', checks: checks.map(c => ({ ...c, secret: 'PRIVATE' })) },
])('invalid/inconsistent permission evidence is not Allowed: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value); await expect(previewServiceAccount(targetId, 'team')).rejects.toThrow();
});
