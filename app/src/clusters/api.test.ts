import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, test, vi } from 'vitest';
import { contexts, discover, connectCluster, reloadContexts, clusterPlaces, reconnectCluster, forgetCluster } from './api';

afterEach(() => { clearMocks(); vi.unstubAllGlobals(); });
const id = '00000000-0000-4000-8000-000000000001';
const place = { key: 'full-installation-key', context: 'team', service: 'control/orka', namespace: 'agents', stale: true, observedAt: null, agents: [] };
test('places reads only the passive native snapshot and preserves unobserved facts', async () => {
  vi.stubGlobal('isTauri', true);
  const value = { places: [place], selectedKey: place.key, persistenceState: 'complete' };
  const ipc = vi.fn(() => value); mockIPC(ipc);
  await expect(clusterPlaces()).resolves.toEqual(value);
  expect(ipc).toHaveBeenCalledExactlyOnceWith('cluster_places', {});
});
test.each([
  null,
  { places: [place], selectedKey: 'same-name-not-same-installation', persistenceState: 'complete' },
  { places: [{ ...place, observedAt: null, stale: false }], selectedKey: null, persistenceState: 'complete' },
  { places: [place], selectedKey: null, persistenceState: 'complete', token: 'SECRET_MARKER' },
  { places: [place], selectedKey: null },
  { places: [place], selectedKey: null, persistenceState: 'invented' },
])('invalid places snapshot is not projected: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value);
  await expect(clusterPlaces()).resolves.toBeUndefined();
});
test('unavailable persistence is preserved, not treated as an empty saved list', async () => {
  vi.stubGlobal('isTauri', true);
  const value = { places: [place], selectedKey: null, persistenceState: 'unavailable' };
  mockIPC(() => value); await expect(clusterPlaces()).resolves.toEqual(value);
});
test('remembered reconnect uses only the native key and validates fresh target handles', async () => {
  vi.stubGlobal('isTauri', true);
  const value = { id, context_id: id, context_name: 'team', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image:v1' };
  const ipc = vi.fn(() => value); mockIPC(ipc);
  await expect(reconnectCluster(place.key)).resolves.toEqual(value);
  expect(ipc).toHaveBeenCalledWith('cluster_reconnect', { key: place.key });
  mockIPC(() => ({ ...value, id: 'saved-name', token: 'SECRET_MARKER' }));
  await expect(reconnectCluster(place.key)).rejects.toMatchObject({ failure: { check: 'configuration', result: 'unknown' } });
  mockIPC(() => Promise.reject({ check: 'orkaIdentity', result: 'identityRejected' }));
  await expect(reconnectCluster(place.key)).rejects.toMatchObject({ failure: { check: 'orkaIdentity', result: 'identityRejected' } });
});
test('forget accepts only native void success and never exports rejection diagnostics', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(() => null); mockIPC(ipc);
  await expect(forgetCluster(place.key)).resolves.toBeUndefined();
  expect(ipc).toHaveBeenCalledWith('cluster_forget', { key: place.key });
  for (const value of [false, {}, { token: 'SECRET_MARKER' }]) {
    mockIPC(() => value); await expect(forgetCluster(place.key)).rejects.toThrow('Cluster information could not be read');
  }
  mockIPC(() => Promise.reject('SECRET_MARKER'));
  await expect(forgetCluster(place.key)).rejects.toThrow('Cluster information could not be read');
});
test('a passive snapshot error or browser preview does not export private diagnostics', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(() => Promise.reject({ message: 'SECRET_MARKER' })); mockIPC(ipc);
  await expect(clusterPlaces()).resolves.toBeUndefined();
  vi.stubGlobal('isTauri', false); ipc.mockClear();
  await expect(clusterPlaces()).resolves.toBeUndefined(); expect(ipc).not.toHaveBeenCalled();
});
test.each([null, {}, [{ id, name: 'team', token: 'SECRET_MARKER' }], [{ id: 'endpoint', name: 'team' }], [{ id, name: 'line\nfeed' }]])('invalid context reply is not exported: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value);
  await expect(contexts()).rejects.toThrow('Cluster information could not be read');
});
test.each([contexts, reloadContexts])('context reads require native current-context metadata and reject extra fields', async read => {
  vi.stubGlobal('isTauri', true);
  for (const current of [true, false]) {
    mockIPC(() => [{ id, name: 'team', current }]);
    await expect(read()).resolves.toEqual([{ id, name: 'team', current }]);
  }
  for (const value of [{ id, name: 'team' }, { id, name: 'team', current: 'true' }, { id, name: 'team', current: true, token: 'SECRET_MARKER' }]) {
    mockIPC(() => [value]); await expect(read()).rejects.toThrow('Cluster information could not be read');
  }
});
test('native diagnostics cannot be echoed through the adapter', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => Promise.reject({ token: 'SECRET_MARKER', message: 'SECRET_MARKER' }));
  await expect(discover(id)).rejects.toThrow('Cluster information could not be read');
});
test('a different context target cannot be selected', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => [{ id, context_id: '00000000-0000-4000-8000-000000000002', context_name: 'team', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image:v1' }]);
  await expect(discover(id)).rejects.toThrow();
});
test.each([{ status: 'connected', providers: [] }, { status: 'needServiceAccount', providers: [{ id, name: 'model', ready: true }] }, { status: 'invented', providers: [] }])('invalid access is not a pass: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value); await expect(connectCluster(id, null)).rejects.toThrow();
});
test('reload invokes the offline native command', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(() => [{ id, name: 'fixed', current: false }]); mockIPC(ipc);
  await expect(reloadContexts()).resolves.toEqual([{ id, name: 'fixed', current: false }]);
  expect(ipc).toHaveBeenCalledWith('cluster_reload', {});
});
test('finite failure retains its check but private diagnostics are not exported', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(() => Promise.reject({ check: 'providers', result: 'permissionDenied' }));
  await expect(discover(id)).rejects.toMatchObject({ failure: { check: 'providers', result: 'permissionDenied' } });
  mockIPC(() => Promise.reject({ check: 'providers', result: 'permissionDenied', diagnostic: 'SECRET_MARKER' }));
  await expect(discover(id)).rejects.toMatchObject({ failure: { check: 'clusterApi', result: 'unknown' } });
});
test.each([
  { status: 'connected', providers: [{ id, name: 'model', ready: true }], failure: { check: 'providers', result: 'unknown' } },
  { status: 'unavailable', providers: [], failure: null },
  { status: 'denied', providers: [], failure: { check: 'secret', result: 'permissionDenied' } },
])('inconsistent failure metadata is rejected: %j', async value => {
  vi.stubGlobal('isTauri', true); mockIPC(() => value); await expect(connectCluster(id, null)).rejects.toThrow();
});
test('valid failures and non-failures carry consistent metadata', async () => {
  vi.stubGlobal('isTauri', true);
  for (const value of [
    { status: 'connected', providers: [{ id, name: 'model', ready: true }], failure: null },
    { status: 'needServiceAccount', providers: [], failure: null },
    { status: 'unavailable', providers: [], failure: { check: 'providers', result: 'noProviders' } },
  ]) { mockIPC(() => value); await expect(connectCluster(id, null)).resolves.toEqual(value); }
});
test('browser preview does not invoke native commands', async () => {
  vi.stubGlobal('isTauri', false); const ipc = vi.fn(); mockIPC(ipc);
  await expect(contexts()).rejects.toThrow('desktop app'); expect(ipc).not.toHaveBeenCalled();
});
