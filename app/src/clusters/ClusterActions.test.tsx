import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { cleanup, render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import App from '../App';
import Places from './Places';
import { setupView } from '../setup/progress';
import { chooseContext } from './test-context';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name, service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const place = { key: 'binding-primary', context: 'team-dev', namespace: 'agents', service: 'control / orka', stale: false, observedAt: Date.now() - 120_000, agents: [] };
const status = { selected: null, localVm: 'idle', model: 'locked' } as const;
function native(failure?: { check: string; result: string }, stage = 'discover', useReply?: () => unknown) {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => {
    if (command === 'setup_status') return status;
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_places') return { places: [place], selectedKey: place.key, persistenceState: 'complete' };
    if (command === 'overview_places') return { places: [place], historyState: 'complete', unobserved: [] };
    if (command === 'cluster_contexts') return failure && stage === 'contexts' ? Promise.reject(failure) : [context];
    if (command === 'cluster_discover') { if (failure && stage === 'discover') return Promise.reject(failure); return [target]; }
    if (command === 'cluster_reconnect') return failure && stage === 'reconnect' ? Promise.reject(failure) : target;
    if (command === 'cluster_use') return useReply ? useReply() : failure && stage === 'use' ? Promise.reject(failure) : { status: 'connected', providers: [provider], failure: null };
    if (command === 'cluster_forget') return Promise.reject(Error('storage unavailable'));
    throw Error('Unsupported: ' + command);
  });
}
async function added(useReply?: () => unknown) {
  native(undefined, 'discover', useReply); const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'I already run Orka on a cluster' }));
  await chooseContext(user, context.name);
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findByRole('button', { name: 'Done' });
  return user;
}
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('added cluster names the page and keeps only Done and Open outside details', async () => {
  const user = await added();
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
  expect(screen.getByRole('button', { name: 'Done' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Open your agent' })).toBeTruthy();
  for (const name of ['Check access again', 'Change context', 'Reload kubeconfig', 'Remove']) expect(screen.queryByRole('button', { name })).toBeNull();
  expect(document.querySelector('.host-chip .chip')?.textContent).toBe('In use');
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  for (const name of ['Check access again', 'Change context', 'Reload kubeconfig', 'Remove']) expect(screen.getByRole('button', { name })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Remove' }).className).toContain('button-quiet');
  expect(screen.getByRole('button', { name: 'Remove' }).className).toContain('cluster-remove');
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'kubectl context' })));
  await chooseContext(user, context.name);
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
  expect(document.querySelector('.host-chip .chip')).toBeNull();
});

test.each([false, true])('Keep cluster restores visible focus even if details closed: %s', async close => {
  const user = await added();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  if (close) await user.click(screen.getByRole('button', { name: 'Hide details' }));
  await user.click(screen.getByRole('button', { name: 'Keep cluster' }));
  await waitFor(() => expect(document.activeElement?.textContent).toBe(close ? 'Show details' : 'Remove'));
  expect(screen.queryByRole('group', { name: 'Remove cluster confirmation' })).toBeNull();
});

test('rechecking a known cluster keeps its name without retaining In use', async () => {
  let reject!: (failure: unknown) => void; let calls = 0;
  const pending = new Promise((_, no) => { reject = no; });
  const user = await added(() => ++calls === 1 ? { status: 'connected', providers: [provider], failure: null } : pending);
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Check access again' }));
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
  expect(document.querySelector('.host-chip .chip')).toBeNull();
  reject({ check: 'clusterApi', result: 'connectionFailed' });
  await screen.findByRole('alert');
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
});

test('failed removal preserves confirmation and named cluster', async () => {
  const user = await added();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining("Couldn't confirm removal"));
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
  expect(screen.getByRole('button', { name: 'Keep cluster' })).toBeTruthy();
});

test.each([false, true])('In use is independent of historical status, stale=%s', stale => {
  render(<Places snapshot={{ places: [{ ...place, stale }, { ...place, key: 'other', context: 'unchecked', observedAt: null }], selectedKey: place.key, persistenceState: 'complete' }} unavailable={false} status={status} view={setupView(status)} place={{ kind: 'cluster', target }} onLocal={() => {}} onCluster={() => {}} />);
  const selected = within(screen.getByRole('region', { name: 'team-dev' }));
  expect(selected.getByText('In use').className).toContain('chip');
  expect(selected.getByText(stale ? 'Stale' : 'Ready')).toBeTruthy();
  expect(selected.getByText('Orka in control/orka')).toBeTruthy();
  expect(selected.getByText(/^Checked/).textContent).toContain('2 minutes ago');
  expect(selected.getByRole('button', { name: 'Manage' })).toBeTruthy();
  const unchecked = within(screen.getByRole('region', { name: 'unchecked' }));
  expect(unchecked.getByRole('button', { name: 'Check' })).toBeTruthy();
  expect(unchecked.queryByText('In use')).toBeNull();
  expect(screen.getByText('Your workspace')).toBeTruthy();
});

test('an unconnected remembered entry restores focus to its visible Remove action', async () => {
  native({ check: 'configuration', result: 'unknown' }, 'reconnect');
  localStorage.setItem('bridge.welcomeSeen.v1', '1'); const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use' }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Keep cluster' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove' }));
});

test('removal confirmation hides maintenance and cancels when a new context is chosen', async () => {
  const user = await added();
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  for (const name of ['Change context', 'Check access again', 'Reload kubeconfig']) expect(screen.queryByRole('button', { name })).toBeNull();
  await chooseContext(user, context.name);
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  expect(await screen.findByRole('button', { name: 'Done' })).toBeTruthy();
  expect(screen.queryByRole('group', { name: 'Remove cluster confirmation' })).toBeNull();
});

test('connected kubeconfig errors retain visible removal and its focus return', async () => {
  native({ check: 'configuration', result: 'unknown' }, 'contexts');
  localStorage.setItem('bridge.welcomeSeen.v1', '1'); const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use' }));
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await user.click(await screen.findByRole('button', { name: 'Show details' }));
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Keep cluster' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove' }));
});

test('returning to the same remembered entry republishes its known name', async () => {
  native(); localStorage.setItem('bridge.welcomeSeen.v1', '1'); const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use' }));
  await screen.findByRole('button', { name: 'Use this cluster' });
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await user.click(await screen.findByRole('button', { name: 'Use' }));
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findByRole('button', { name: 'Done' });
  expect(screen.getByRole('heading', { level: 1 }).textContent).toBe('team-dev');
});

test('checked clusters say Use and populated Add stays secondary', async () => {
  native(); localStorage.setItem('bridge.welcomeSeen.v1', '1'); render(<App />);
  expect(await screen.findByRole('button', { name: 'Use' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Add a cluster' }).className).toContain('button-secondary');
  expect(document.querySelector('.host-chip .chip')).toBeNull();
});

test.each([
  [{ check: 'clusterApi', result: 'connectionFailed' }, "Can't reach team-dev. Check your network or VPN."],
  [{ check: 'clusterApi', result: 'permissionDenied' }, "Couldn't reach this cluster. Access was denied."],
])('discovery names network failures without replacing other recovery: %j', async (failure, copy) => {
  native(failure); const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'I already run Orka on a cluster' }));
  expect(screen.queryByText('Choose the context for the cluster you want to use.')).toBeNull();
  await chooseContext(user, context.name);
  expect(await within(await screen.findByRole('alert')).findByText(copy)).toBeTruthy();
});

test.each(['use', 'reconnect'])('network failure resolves the display context for %s', async stage => {
  native({ check: 'clusterApi', result: 'connectionFailed' }, stage);
  if (stage === 'reconnect') localStorage.setItem('bridge.welcomeSeen.v1', '1');
  const user = userEvent.setup(); render(<App />);
  if (stage === 'use') {
    await user.click(await screen.findByRole('button', { name: 'I already run Orka on a cluster' }));
    await chooseContext(user, context.name);
    await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  } else {
    await user.click(await screen.findByRole('button', { name: 'Use' }));
  }
  expect(await within(await screen.findByRole('alert')).findByText("Can't reach team-dev. Check your network or VPN.")).toBeTruthy();
  expect(document.querySelector('.host-chip .chip')).toBeNull();
});
