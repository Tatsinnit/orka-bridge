import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { readWorkspaceCatalog } from './catalog';
const ipc = vi.hoisted(() => ({ invoke: vi.fn(), native: true }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: ipc.invoke, isTauri: () => ipc.native }));
const fixture = { defaultModel: 'm:exact', models: { state: 'ready', items: [{ id: 'm:exact' }, { id: 'other', pickerEnabled: false, chatSupported: true }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Fetch' }, { kind: 'tool', name: 'custom', description: '<script>plain text</script>', available: false }] } };
beforeEach(() => { ipc.invoke.mockReset(); ipc.native = true; });
afterEach(() => { vi.useRealTimers(); });
const read = () => readWorkspaceCatalog(new AbortController().signal);
const remote = { targetId: '39dc68be-f597-4535-8ebd-7759f544b27a', providerId: '39dc68be-f597-4535-8ebd-7759f544b27b' };
test('remote catalog uses only selected native ids and permits an absent configured default', async () => {
  ipc.invoke.mockResolvedValue({ ...fixture, defaultModel: null, models: { state: 'ready', items: [] } });
  expect(await readWorkspaceCatalog(new AbortController().signal, remote)).toEqual({ ...fixture, defaultModel: null, models: { state: 'ready', items: [] } });
  expect(ipc.invoke).toHaveBeenCalledExactlyOnceWith('cluster_catalog', remote);
});
test('local replies still require a default model and malformed remote ids never invoke', async () => {
  ipc.invoke.mockResolvedValue({ ...fixture, defaultModel: null });
  expect(await read()).toBeUndefined();
  ipc.invoke.mockClear();
  expect(await readWorkspaceCatalog(new AbortController().signal, { ...remote, targetId: 'not-an-id' })).toBeUndefined();
  expect(ipc.invoke).not.toHaveBeenCalled();
});
test('reads exact metadata without supplying a target or losing unknown capabilities', async () => {
  ipc.invoke.mockResolvedValue(fixture);
  expect(await read()).toEqual(fixture);
  expect(ipc.invoke).toHaveBeenCalledExactlyOnceWith('workspace_catalog');
});
test('keeps independently unavailable and successful empty catalogs distinct', async () => {
  const value = { defaultModel: 'm', models: { state: 'ready', items: [] }, tools: { state: 'unavailable' } };
  ipc.invoke.mockResolvedValue(value); expect(await read()).toEqual(value);
});
test.each([
  { ...fixture, token: 'synthetic-private' },
  { ...fixture, defaultModel: 'auto' },
  { ...fixture, models: { state: 'invented', items: [] } },
  { ...fixture, models: { state: 'unavailable', items: [] } },
  { ...fixture, models: { state: 'ready', items: [{ id: 'm', chatSupported: null }] } },
  { ...fixture, models: { state: 'ready', items: [{ id: 'm' }, { id: 'm' }] } },
  { ...fixture, models: { state: 'ready', items: [{ id: 'm'.repeat(257) }] } },
  { ...fixture, tools: { state: 'ready', items: [{ kind: 'tool', name: 'custom', description: 'desc', url: 'http://private.invalid' }] } },
  { ...fixture, tools: { state: 'ready', items: [{ kind: 'tool', name: 'custom', description: 'é'.repeat(4097) }] } },
  { ...fixture, tools: { state: 'ready', items: [{ kind: 'mcp', name: 'x', description: 'x' }] } },
])('rejects malformed or over-budget reply without exposing supplied values', async value => {
  ipc.invoke.mockResolvedValue(value); expect(await read()).toBeUndefined();
});
test('a workspace change aborts an outstanding read and a pre-aborted read never invokes', async () => {
  const controller = new AbortController();
  let finish!: (value: unknown) => void;
  ipc.invoke.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const pending = readWorkspaceCatalog(controller.signal);
  controller.abort(); finish(fixture); expect(await pending).toBeUndefined();
  ipc.invoke.mockClear(); expect(await readWorkspaceCatalog(controller.signal)).toBeUndefined();
  expect(ipc.invoke).not.toHaveBeenCalled();
});
test('a busy native connection keeps the first catalog read pending until released', async () => {
  vi.useFakeTimers();
  ipc.invoke.mockRejectedValueOnce('catalogBusy').mockRejectedValueOnce('catalogBusy').mockResolvedValue(fixture);
  let settled = false;
  const pending = read().then(value => { settled = true; return value; });
  await vi.advanceTimersByTimeAsync(499);
  expect(settled).toBe(false); expect(ipc.invoke).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(settled).toBe(false); expect(ipc.invoke).toHaveBeenCalledTimes(2);
  await vi.advanceTimersByTimeAsync(500);
  expect(await pending).toEqual(fixture);
  expect(ipc.invoke).toHaveBeenCalledTimes(3);
});
test.each(['waiting', 'in-flight'])('aborting a busy catalog while %s prevents further native calls', async phase => {
  vi.useFakeTimers(); const controller = new AbortController();
  let fail!: (reason: unknown) => void;
  ipc.invoke.mockImplementation(() => new Promise((_resolve, reject) => { fail = reject; }));
  const pending = readWorkspaceCatalog(controller.signal);
  if (phase === 'waiting') { fail('catalogBusy'); await vi.advanceTimersByTimeAsync(0); }
  controller.abort();
  if (phase === 'in-flight') fail('catalogBusy');
  expect(await pending).toBeUndefined();
  await vi.advanceTimersByTimeAsync(5000);
  expect(ipc.invoke).toHaveBeenCalledTimes(1); expect(vi.getTimerCount()).toBe(0);
});
test.each(['catalogUnavailable', 'unknown', { message: 'catalogBusy' }])('actual failures never retry (%j)', async failure => {
  vi.useFakeTimers(); ipc.invoke.mockRejectedValue(failure);
  expect(await read()).toBeUndefined();
  await vi.advanceTimersByTimeAsync(5000);
  expect(ipc.invoke).toHaveBeenCalledTimes(1);
});
test('a remote failure cannot opt into the local busy observation loop', async () => {
  vi.useFakeTimers(); ipc.invoke.mockRejectedValue('catalogBusy');
  expect(await readWorkspaceCatalog(new AbortController().signal, remote)).toBeUndefined();
  await vi.advanceTimersByTimeAsync(5000); expect(ipc.invoke).toHaveBeenCalledTimes(1);
});
test('preview and native failure return no untrusted error text', async () => {
  ipc.native = false; expect(await read()).toBeUndefined(); expect(ipc.invoke).not.toHaveBeenCalled();
  ipc.native = true; ipc.invoke.mockRejectedValue({ message: 'synthetic-private', token: 'secret' });
  expect(await read()).toBeUndefined();
});
