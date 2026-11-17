import { chooseContext, contextChoice, reloadConfig } from './test-context';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import ClusterCard from './ClusterCard';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team', current: false };
const fresh = { id: '00000000-0000-4000-8000-000000000004', name: 'corrected', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name,
  service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image:v1' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'model', ready: true };
const connected = { status: 'connected', providers: [provider], failure: null };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function setup(handler?: (command: string) => unknown) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => handler ? handler(command) : command === 'cluster_contexts' ? [context]
    : command === 'cluster_discover' ? [target] : command === 'cluster_use' ? connected : [fresh]);
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  render(<ClusterCard onPlace={onPlace} onOpenAgent={vi.fn()} />);
  return { ipc, onPlace, user };
}
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function select(user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(false));
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
}

test.each(['empty', 'error'])('reload recovers after %s initial config without restart', async state => {
  const { user, ipc } = setup(command => command === 'cluster_contexts'
    ? state === 'empty' ? [] : Promise.reject({ check: 'configuration', result: 'unknown' }) : [fresh]);
  await screen.findByText(state === 'empty' ? /No kubectl contexts/ : /kubeconfig could not be read/);
  await reloadConfig(user);
  expect(await contextChoice(user, 'corrected')).toBeTruthy();
  expect(ipc.mock.calls.map(([command]) => command)).toEqual(['cluster_contexts', 'cluster_reload']);
});
test('reload clears all confirmed selection and requires explicit Use again', async () => {
  const { user, onPlace, ipc } = setup(); await select(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByRole('combobox', { name: 'AI connection' });
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster', target, providers: [provider], providerId: provider.id });
  await reloadConfig(user);
  await contextChoice(user, 'corrected');
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster' });
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain('Choose a context');
  expect(screen.queryByRole('combobox', { name: 'AI connection' })).toBeNull();
  expect(screen.queryByText('Cluster found')).toBeNull();
  expect(screen.queryByText('Access confirmed')).toBeNull();
  expect(ipc.mock.calls.filter(([command]) => command === 'cluster_use')).toHaveLength(1);
});
test.each(['cluster_contexts', 'cluster_discover', 'cluster_use'])('reload ignores late %s reply and stays pending until fresh config', async command => {
  const old = deferred<unknown>(); const reload = deferred<unknown>();
  const { user, onPlace, ipc } = setup(c => c === command ? old.promise : c === 'cluster_reload' ? reload.promise
    : c === 'cluster_contexts' ? [context] : [target]);
  if (command !== 'cluster_contexts') {
    await waitFor(() => expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(false));
    await chooseContext(user, context.name);
  }
  if (command === 'cluster_use') { await screen.findByText('Cluster found'); await user.click(screen.getByRole('button', { name: 'Use this cluster' })); }
  await reloadConfig(user);
  expect(screen.getByRole('button', { name: 'Reloading kubeconfig…' }).matches(':disabled')).toBe(true);
  await act(async () => old.resolve(command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? [target] : connected));
  expect(screen.queryByText('Access confirmed')).toBeNull();
  expect(screen.queryByText('Cluster found')).toBeNull();
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster' });
  await act(async () => reload.resolve([fresh]));
  expect(await contextChoice(user, 'corrected')).toBeTruthy();
  expect(screen.queryByRole('option', { name: 'team' })).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_reload')).toHaveLength(1);
});
test('failed reload clears account and old contexts, and remains recoverable', async () => {
  const { user, onPlace, ipc } = setup(command => command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? [target]
    : command === 'cluster_use' ? { status: 'needServiceAccount', providers: [], failure: null }
    : Promise.reject({ check: 'configuration', result: 'unknown' }));
  await select(user); await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await user.type(await screen.findByRole('textbox', { name: 'ServiceAccount name' }), 'old-account');
  await reloadConfig(user);
  await screen.findByRole('alert');
  expect(screen.queryByRole('textbox', { name: 'ServiceAccount name' })).toBeNull();
  expect(screen.queryByRole('option', { name: 'team' })).toBeNull();
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster' });
  ipc.mockImplementation(() => [fresh]);
  await reloadConfig(user);
  expect(await contextChoice(user, 'corrected')).toBeTruthy();
});
test('first failed check has one cause/action, safe copied labels and visible clipboard fallback', async () => {
  const { user } = setup(c => c === 'cluster_contexts' ? [context] : c === 'cluster_discover' ? [target]
    : { status: 'denied', providers: [], failure: { check: 'providers', result: 'permissionDenied' } });
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  await select(user); await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  expect((await screen.findByRole('alert')).textContent).toContain('read AI connections');
  expect(screen.getByRole('alert').textContent).toContain('Access was denied');
  expect(screen.getByRole('alert').textContent).toContain('agent files are kept');
  expect(screen.getByRole('alert').compareDocumentPosition(screen.getByRole('button', { name: 'Check again' })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  if (!screen.queryByRole('button', { name: 'Copy details for help' })) await user.click(screen.getByRole('button', { name: 'Details' }));
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(copy).toHaveBeenCalledWith('Bridge cluster connection\nOperation: Use cluster\nCheck: providers\nResult: permissionDenied');
  copy.mockRejectedValue('SECRET /private/path');
  if (!screen.queryByRole('button', { name: 'Copy details for help' })) await user.click(screen.getByRole('button', { name: 'Details' }));
  await user.click(screen.getByRole('button', { name: 'Copy details for help' }));
  expect(await screen.findByText(/share the visible error message/i)).toBeTruthy();
  expect(document.body.textContent).not.toContain('SECRET');
});
