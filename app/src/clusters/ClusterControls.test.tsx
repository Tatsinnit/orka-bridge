import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import ClusterCard from './ClusterCard';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: true };
const other = { ...context, id: '00000000-0000-4000-8000-000000000004', name: 'other', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name,
  service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image:v1' };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function setup(handler?: (command: string) => unknown) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => handler ? handler(command) : command === 'cluster_contexts' || command === 'cluster_reload' ? [context, other] : [target]);
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  render(<ClusterCard onPlace={onPlace} onOpenAgent={vi.fn()} />);
  return { user, ipc, onPlace };
}
async function picker(user: ReturnType<typeof userEvent.setup>) {
  const trigger = screen.getByRole('button', { name: 'kubectl context' });
  await waitFor(() => expect(trigger.matches(':disabled')).toBe(false));
  await user.click(trigger);
  return screen.getByRole('dialog', { name: 'Choose a context' });
}
beforeEach(() => {
  // jsdom has no top layer. Native light dismissal is observed through toggle.
  Object.defineProperty(HTMLDivElement.prototype, 'showPopover', { configurable: true, value() { this.style.display = 'grid'; } });
});
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); Reflect.deleteProperty(HTMLDivElement.prototype, 'showPopover'); });

test('keyboard context picker searches native metadata without selecting or discovering in the background', async () => {
  const { user, ipc, onPlace } = setup();
  const dialog = await picker(user);
  expect(dialog.getAttribute('popover')).toBe('auto');
  expect(within(dialog).getByText('Current context')).toBeTruthy();
  expect(within(dialog).queryByText('Orka found')).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole('searchbox', { name: 'Find a context' }));
  await user.type(screen.getByRole('searchbox', { name: 'Find a context' }), 'team-dev');
  expect(within(dialog).queryByRole('button', { name: 'other' })).toBeNull();
  await user.tab(); await user.keyboard('{Enter}');
  await screen.findByText('Cluster found');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'kubectl context' }));
  expect(ipc.mock.calls).toEqual([['cluster_contexts', {}], ['cluster_discover', { contextId: context.id }]]);
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster' });
  expect(screen.getByRole('button', { name: 'kubectl context', description: context.name })).toBeTruthy();
  expect(screen.queryByText('Access confirmed')).toBeNull();
});

test('context picker Close, Escape and native outside dismissal preserve focus without starting discovery', async () => {
  const { user, ipc } = setup();
  await picker(user); await user.click(screen.getByRole('button', { name: 'Close picker' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'kubectl context' }));
  await picker(user); await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'kubectl context' }));
  const dialog = await picker(user);
  const outside = screen.getByRole('button', { name: 'Reload kubeconfig' }); outside.focus();
  fireEvent(dialog, Object.assign(new Event('toggle'), { oldState: 'open', newState: 'closed' }));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(document.activeElement).toBe(outside);
  expect(ipc.mock.calls).toEqual([['cluster_contexts', {}]]);
});

test('Orka found is session discovery evidence only, cleared when reload returns fresh opaque IDs', async () => {
  const { user, ipc } = setup();
  await picker(user); await user.click(screen.getByRole('button', { name: context.name }));
  await screen.findByText('Cluster found');
  let dialog = await picker(user);
  expect(within(dialog).getByRole('button', { name: context.name }).textContent).toContain('Orka found');
  expect(within(dialog).getByRole('button', { name: other.name }).textContent).not.toContain('Orka found');
  await user.keyboard('{Escape}');
  ipc.mockImplementation(command => command === 'cluster_reload' ? [{ ...context, id: other.id }] : []);
  await user.click(screen.getByRole('button', { name: 'Reload kubeconfig' }));
  dialog = await picker(user);
  expect(within(dialog).queryByText('Orka found')).toBeNull();
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain('Choose a context');
  await user.click(screen.getByRole('button', { name: context.name }));
  await screen.findByText('No cluster with Orka was found using this context.');
  dialog = await picker(user);
  expect(within(dialog).queryByText('Orka found')).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_discover')).toEqual([
    ['cluster_discover', { contextId: context.id }], ['cluster_discover', { contextId: other.id }],
  ]);
});

test('retrying a context-list read preserves a remembered working connection', async () => {
  vi.stubGlobal('isTauri', true);
  const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-ai', ready: true };
  const remembered = { key: 'team', context: context.name, service: 'control / orka', namespace: 'agents', stale: false, observedAt: 123, agents: [] };
  let reads = 0;
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_reconnect') return target;
    if (command === 'cluster_use') return { status: 'connected', providers: [provider], failure: null };
    if (command === 'cluster_places') return { places: [remembered], selectedKey: remembered.key, persistenceState: 'complete' };
    if (command === 'overview_places') return { places: [remembered], unobserved: [], historyState: 'complete' };
    if (command === 'cluster_contexts') { if (++reads === 1) throw { check: 'configuration', result: 'unknown' }; return [context]; }
    throw Error('Unexpected command ' + command);
  });
  mockIPC(ipc); const onPlace = vi.fn(); const user = userEvent.setup();
  render(<ClusterCard entry={{ revision: 1, remembered }} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Access confirmed');
  const confirmedPlace = onPlace.mock.lastCall;
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(reads).toBe(2));
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('cluster_reload');
  expect(onPlace.mock.lastCall).toEqual(confirmedPlace);
  expect(screen.getByRole('button', { name: 'Open your agent' }).matches(':disabled')).toBe(false);
});

test('pending discovery uses Running and a failed check leaves one primary retry with secondary reload in Details', async () => {
  const pending = deferred<unknown>();
  const { user, ipc } = setup(command => command === 'cluster_contexts' ? [context] : pending.promise);
  await picker(user); await user.click(screen.getByRole('button', { name: context.name }));
  expect(screen.getByRole('status').querySelector('.status-running')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(true);
  await act(async () => pending.resolve(Promise.reject({ check: 'credentials', result: 'unknown' })));
  const alert = await screen.findByRole('alert');
  expect(alert.textContent).toContain("Couldn't sign in to this cluster");
  expect(alert.textContent).not.toMatch(/Credential resolution|cause could not be established|run checks/);
  expect(alert.classList.contains('cluster-failure')).toBe(true);
  expect(screen.queryByRole('button', { name: 'Reload kubeconfig' })).toBeNull();
  expect(document.querySelectorAll('.cluster-card .button-primary')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Details' }));
  expect(screen.getByText('Check: credentials')).toBeTruthy();
  expect(screen.getByText('Result: unknown')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Reload kubeconfig' }).classList.contains('button-secondary')).toBe(true);
  ipc.mockImplementation(() => []);
  await user.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByText('No cluster with Orka was found using this context.');
});
