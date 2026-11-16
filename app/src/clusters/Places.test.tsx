import { chooseContext, contextChoice, reloadConfig } from './test-context';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Setup from '../setup/SetupHarness';
import type { SelectedPlace } from './api';
import type { Place } from '../overview/api';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name,
  service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const primary = { key: 'binding-primary', context: 'team-dev', namespace: 'agents', service: 'control / orka', stale: true, observedAt: 1700000000000, agents: [] };
const secondary = { ...primary, key: 'binding-secondary', context: 'team-other', observedAt: null };
const connected: SelectedPlace = { kind: 'cluster', target, providers: [provider], providerId: provider.id };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
function native() {
  vi.stubGlobal('isTauri', true);
  const sim = { status: { selected: null as 'localVm' | null, localVm: 'idle', model: 'locked', modelSupported: true },
    snapshot: { places: [primary, secondary] as Place[], selectedKey: primary.key as string | null, persistenceState: 'complete' }, unavailable: false };
  const ipc = vi.fn((command: string): unknown => {
    if (command === 'setup_status') return sim.status;
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_places') { if (sim.unavailable) throw Error('unreadable'); return sim.snapshot; }
    if (command === 'cluster_reconnect') return target;
    if (command === 'overview_places') return { places: sim.snapshot.places, historyState: 'complete', unobserved: [] };
    if (command === 'cluster_forget') { sim.snapshot.places = sim.snapshot.places.filter(p => p.key !== primary.key); sim.snapshot.selectedKey = null; return null; }
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return { status: 'connected', providers: [provider], failure: null };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'setup_select_local') { sim.status = { ...sim.status, selected: 'localVm', localVm: 'starting' }; return sim.status; }
    throw Error('Unexpected: ' + command);
  });
  mockIPC(ipc); return { ipc, sim };
}
function Host({ initial = { kind: 'thisComputer' }, observe = () => {} }: { initial?: SelectedPlace; observe?: (place: SelectedPlace) => void }) {
  const [place, setPlace] = useState(initial);
  return <Setup place={place} onPlace={value => { setPlace(value); observe(value); }} onOpenExample={() => {}} />;
}
const row = (name: string) => screen.getByRole('region', { name });
beforeEach(() => localStorage.setItem('bridge.welcomeSeen.v1', '1'));
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); localStorage.clear(); });

test('Places passively lists this computer and retained identities with truthful historical status', async () => {
  const { ipc } = native(); render(<Host initial={connected} />);
  await screen.findByRole('heading', { name: 'Clusters' });
  await screen.findByRole('region', { name: 'team-other' });
  expect(within(row('This computer')).getAllByRole('button')).toHaveLength(1);
  expect(within(row('This computer')).getByRole('button', { name: 'Set up' })).toBeTruthy();
  expect(within(row('team-dev')).getByText('Selected for Designer')).toBeTruthy();
  expect(row('team-dev').querySelectorAll('.place-summary > p')).toHaveLength(1);
  expect(within(row('team-dev')).getByText('Selected for Designer').closest('p')).toBe(within(row('team-dev')).getByText(/^Last checked /).closest('p'));
  expect(within(row('team-dev')).getByRole('button', { name: 'Manage' })).toBeTruthy();
  expect(within(row('team-dev')).queryByRole('button', { name: 'Connect' })).toBeNull();
  expect(within(row('team-dev')).getByText(/^Last checked /).closest('.pill')).toBeNull();
  expect(row('team-dev').querySelector('.pill.status-stale')).toBeTruthy();
  expect(within(row('team-dev')).getByText('Selected for Designer').closest('.pill')).toBeNull();
  expect(row('team-other').querySelector('.pill.status-not-started')?.textContent).toBe('Not checked');
  expect(within(row('team-other')).getByText('Not checked yet')).toBeTruthy();
  expect(within(row('team-other')).queryByText(/0 agents/)).toBeNull();
  expect(document.body.textContent).not.toMatch(/Observation stale|Not observed yet|kept while Bridge/);
  expect(ipc.mock.calls.map(([c]) => c).every(c => ['setup_status', 'setup_requirements', 'cluster_places'].includes(c))).toBe(true);
});

test('a remembered access check stays historical rather than becoming never checked without inventory', async () => {
  const { sim } = native();
  sim.snapshot.places = [{ ...primary, observedAt: null, checkedAt: 1700000000000 }];
  render(<Host />);
  const remote = await screen.findByRole('region', { name: 'team-dev' });
  expect(within(remote).getByText(/^Last checked /).closest('.pill')).toBeNull();
  expect(remote.querySelector('.pill.status-stale')?.textContent).toBe('Stale');
  expect(within(remote).queryByText('Not checked')).toBeNull();
});

test('a build without local VM integration explains unavailability rather than advertising unused setup', async () => {
  const { sim } = native(); sim.status = { ...sim.status, selected: null, localVm: 'notIntegrated' };
  render(<Host />);
  const local = await screen.findByRole('region', { name: 'This computer' });
  await within(local).findByText('Local cluster unavailable');
  expect(within(local).queryByText('The local cluster runs here')).toBeNull();
  expect(local.querySelector('.pill.status-needs-you')).toBeTruthy();
});

test('an unreadable workspace status stays unknown while retained places remain readable', async () => {
  const { ipc } = native();
  mockIPC(command => command === 'setup_status' ? Promise.reject(Error('lost status reply')) : ipc(command));
  render(<Host />);
  await screen.findByRole('alert');
  const local = within(row('This computer'));
  expect(local.queryByText('Not set up')).toBeNull();
  expect(local.queryByText('AI not connected')).toBeNull();
  expect(local.getByText('Local cluster unavailable')).toBeTruthy();
  expect(local.getByRole('button', { name: 'View details' })).toBeTruthy();
  expect(await screen.findByRole('region', { name: 'team-other' })).toBeTruthy();
  expect(screen.queryByText('Reading your clusters…')).toBeNull();
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
});

test.each<SelectedPlace>([{ kind: 'thisComputer' }, { kind: 'cluster' }])('native access alone is not a Designer selection: %j', async initial => {
  native(); render(<Host initial={initial} />);
  await screen.findByRole('region', { name: 'team-dev' });
  expect(screen.queryByText('Selected for Designer')).toBeNull();
  expect(within(row('team-dev')).getByRole('button', { name: 'Connect' })).toBeTruthy();
});

test('equal display names do not substitute for the native selected key', async () => {
  const { sim } = native(); sim.snapshot.places = [primary, { ...secondary, context: primary.context }];
  sim.snapshot.selectedKey = secondary.key;
  render(<Host initial={connected} />);
  const rows = await screen.findAllByRole('region', { name: 'team-dev' });
  expect(within(rows[0]).queryByText('Selected for Designer')).toBeNull();
  expect(within(rows[1]).getByText('Selected for Designer')).toBeTruthy();
});

test('installations sharing a context retain distinguishing details before choosing one', async () => {
  const { sim } = native();
  sim.snapshot.places = [primary, { ...secondary, context: primary.context, namespace: 'review', service: 'other / orka' }];
  render(<Host />);
  const rows = await screen.findAllByRole('region', { name: 'team-dev' });
  expect(within(rows[0]).getByText('agents · control / orka')).toBeTruthy();
  expect(within(rows[1]).getByText('review · other / orka')).toBeTruthy();
});

test('Add stays blank while remembered Connect resolves a fresh target without choosing a context', async () => {
  const { ipc } = native(); const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(await screen.findByRole('button', { name: 'Add a cluster' }));
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(screen.queryByRole('radio')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Set up' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Existing cluster' })).toBeNull();
  expect(observe).not.toHaveBeenCalled();
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('cluster_discover');
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
  expect(observe).toHaveBeenLastCalledWith({ kind: 'cluster' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  await screen.findByRole('button', { name: 'Use this cluster' });
  expect(screen.queryByRole('button', { name: 'kubectl context' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Change context' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_reconnect')).toEqual([['cluster_reconnect', { key: primary.key }]]);
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_discover')).toHaveLength(1);
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_select_local');
});

test('Manage Change context focuses its selector after a delayed kubeconfig read', async () => {
  const { ipc } = native(); const contexts = deferred<unknown>(); const user = userEvent.setup();
  mockIPC(command => command === 'cluster_contexts' ? contexts.promise : ipc(command));
  render(<Host initial={connected} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  const change = await screen.findByRole('button', { name: 'Change context' });
  change.focus(); await user.keyboard('{Enter}');
  const select = await screen.findByRole('button', { name: 'kubectl context' });
  expect(select.matches(':disabled')).toBe(true);
  const remove = screen.getByRole('button', { name: 'Remove' });
  remove.focus(); await user.keyboard('{Enter}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Keep cluster' }));
  await user.keyboard('{Enter}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove' }));
  await act(async () => contexts.resolve([context]));
  await waitFor(() => expect(select.matches(':disabled')).toBe(false));
  expect(document.activeElement).toBe(select);
});

test('Manage rechecks the same installation with its already-entered ServiceAccount', async () => {
  const { ipc } = native(); const user = userEvent.setup(); const accounts: unknown[] = [];
  mockIPC((command, args) => {
    if (command === 'cluster_use') {
      const name = args && 'saName' in args ? args.saName : undefined;
      accounts.push(name);
      if (name !== 'team-runner') return { status: 'needServiceAccount', providers: [], failure: null };
    }
    return ipc(command);
  });
  render(<Host />);
  await user.click(await screen.findByRole('button', { name: 'Add a cluster' }));
  await chooseContext(user, context.name);
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await user.type(await screen.findByRole('textbox', { name: 'ServiceAccount name' }), 'team-runner');
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Access confirmed');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  await user.click(await screen.findByRole('button', { name: 'Check access again' }));
  expect(await screen.findByText('Access confirmed')).toBeTruthy();
  expect(screen.queryByRole('textbox', { name: 'ServiceAccount name' })).toBeNull();
  expect(accounts).toEqual([null, 'team-runner', 'team-runner']);
});

test('selected Manage uses current App capabilities without discovery and changes context only explicitly', async () => {
  const { ipc } = native(); const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  expect(await screen.findByRole('combobox', { name: 'AI connection' })).toBeTruthy();
  expect(screen.getByText('team-dev', { selector: 'h2 code' })).toBeTruthy();
  expect(screen.getByText('agents', { selector: '.cluster-result code' })).toBeTruthy();
  expect((screen.getByRole('combobox', { name: 'AI connection' }) as HTMLSelectElement).value).toBe(provider.id);
  expect(screen.getByRole('button', { name: 'Change context' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
  expect(observe).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(within(await screen.findByRole('region', { name: 'team-dev' })).getByText('Selected for Designer')).toBeTruthy();
  expect(observe).not.toHaveBeenCalled();
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('cluster_use');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('cluster_discover');
});

test('local Manage is navigation; explicit Use this computer never starts a stopped VM or trusts its native cluster key', async () => {
  const { ipc } = native(); const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Set up' }));
  expect(observe).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Use this computer' }));
  expect(observe).toHaveBeenLastCalledWith({ kind: 'thisComputer' });
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_select_local');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.queryByText('Selected for Designer')).toBeNull();
});

test.each([['closed', 'Local cluster stopped'], ['cancelled', 'Setup paused'], ['idle', 'Not set up']] as const)('a selected %s local workspace never claims setup is still running', async (localVm, label) => {
  const { sim } = native(); sim.status = { ...sim.status, selected: 'localVm', localVm };
  const user = userEvent.setup(); render(<Host />);
  await user.click(await screen.findByRole('button', { name: 'Clusters' }));
  const local = row('This computer');
  expect(within(local).getByText(label).closest('.pill')).toBeNull();
  expect(local.querySelector('.pill')?.className).toContain('status-not-started');
  expect(within(local).queryByText('Setup in progress')).toBeNull();
  expect(within(local).getByRole('button', { name: 'Continue setup' }).classList.contains('button-primary')).toBe(true);
});

test('snapshot failure retains known rows but does not invent a selected row or empty inventory', async () => {
  const { sim } = native(); const user = userEvent.setup(); render(<Host initial={connected} />);
  await user.click(await screen.findByRole('button', { name: 'Add a cluster' }));
  sim.unavailable = true;
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await screen.findByText("Couldn't read your clusters. Try opening Clusters again.");
  expect(row('team-dev')).toBeTruthy();
  expect(screen.queryByText('Selected for Designer')).toBeNull();
  expect(screen.queryByText(/No other clusters added yet/)).toBeNull();
});

test('a Use recheck clears the frontend target before native replies and Back cannot restore it', async () => {
  const { ipc } = native(); const pending = deferred<unknown>(); const old = ipc.getMockImplementation()!;
  ipc.mockImplementation(c => c === 'cluster_use' ? pending.promise : old(c));
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  await user.click(await screen.findByRole('button', { name: 'Check access again' }));
  expect(observe).toHaveBeenLastCalledWith({ kind: 'cluster' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(screen.queryByText('Selected for Designer')).toBeNull();
  const calls = observe.mock.calls.length;
  await act(async () => pending.resolve({ status: 'connected', providers: [provider], failure: null }));
  expect(observe).toHaveBeenCalledTimes(calls);
  expect(screen.queryByText('Selected for Designer')).toBeNull();
});

test('Back preserves a queued local Model and its one eventual connection while Places is visible', async () => {
  const { ipc, sim } = native(); sim.status = { ...sim.status, selected: 'localVm', localVm: 'installing' };
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(await screen.findByRole('button', { name: 'Clusters' }));
  await user.click(within(row('This computer')).getByRole('button', { name: 'Continue setup' }));
  expect(observe).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Connect the AI' }));
  expect(observe).toHaveBeenLastCalledWith({ kind: 'thisComputer' });
  expect(screen.getByText('Queued', { selector: '.pill' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(within(row('This computer')).getByText('Getting the local cluster ready', { selector: '.place-meta span' })).toBeTruthy();
  expect(observe).toHaveBeenCalledTimes(1);
  sim.status = { ...sim.status, localVm: 'ready', model: 'available' };
  await act(async () => window.dispatchEvent(new Event('focus')));
  await within(row('This computer')).findByText('Local cluster ready');
  await user.click(within(row('This computer')).getByRole('button', { name: 'Manage' }));
  expect(within(screen.getByRole('region', { name: 'Connect the AI' })).getByText('Connected')).toBeTruthy();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_connect_model')).toHaveLength(1);
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_select_local');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_change_local');
});

test('hidden snapshot replies are fenced and returning to Clusters reads a new passive snapshot', async () => {
  const { ipc } = native(); const pending = deferred<unknown>(); const old = ipc.getMockImplementation()!;
  let reads = 0;
  ipc.mockImplementation(c => c === 'cluster_places' && ++reads === 1 ? pending.promise : old(c));
  const view = render(<Setup onOpenExample={() => {}} />);
  await waitFor(() => expect(reads).toBe(1));
  view.rerender(<Setup active={false} onOpenExample={() => {}} />);
  await act(async () => pending.resolve({ places: [primary], selectedKey: primary.key, persistenceState: 'complete' }));
  expect(screen.queryByRole('region', { name: 'team-dev' })).toBeNull();
  expect(reads).toBe(1);
  view.rerender(<Setup active onOpenExample={() => {}} />);
  expect(await screen.findByRole('region', { name: 'team-other' })).toBeTruthy();
  expect(reads).toBe(2);
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('overview_places');
});

test('a known empty list promotes Set up, but a metadata failure does not imply emptiness', async () => {
  const { sim } = native(); sim.snapshot.places = []; sim.snapshot.selectedKey = null;
  const user = userEvent.setup(); render(<Host />);
  const setup = await screen.findByRole('button', { name: 'Set up' });
  await screen.findByText(/No other clusters added/);
  expect(setup.className).toContain('button-primary');
  expect(screen.getByRole('button', { name: 'Add a cluster' }).className).toContain('button-secondary');
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  sim.snapshot.persistenceState = 'unavailable';
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await screen.findByText(/remembered cluster information is unavailable/i);
  expect(screen.queryByText(/No other clusters added/)).toBeNull();
  expect(screen.getByRole('button', { name: 'Set up' }).className).toContain('button-secondary');
});

test('successful Connect has Done and truthful persistence warning without disabling live access', async () => {
  const { sim } = native(); sim.snapshot.persistenceState = 'unavailable';
  const user = userEvent.setup(); render(<Host />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  expect(await screen.findByText(/not be remembered/i)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Open your agent' }).className).toContain('button-secondary');
  expect(screen.getByRole('button', { name: 'Open your agent' }).matches(':disabled')).toBe(false);
  expect(screen.getByRole('button', { name: 'Done' }).className).toContain('button-primary');
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(await screen.findByRole('heading', { name: 'Clusters' })).toBeTruthy();
});

test('Manage refreshes confirmed agents and offers cancellable removal, finite failure, then selection revocation', async () => {
  const { ipc, sim } = native(); const old = ipc.getMockImplementation()!;
  const fresh = { ...primary, stale: false, observedAt: 1700100000000, agents: [{ name: 'reader', namespace: 'agents', uid: 'uid-reader', agentType: 'Declarative', model: 'gpt-test', tools: [], record: null }] };
  let fail = true;
  mockIPC((command, args) => {
    if (command === 'overview_places') { expect(args).toEqual({ key: primary.key }); return { places: [fresh], historyState: 'complete', unobserved: [] }; }
    if (command === 'cluster_forget' && fail) return Promise.reject('SECRET_MARKER');
    return old(command);
  });
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  await screen.findByRole('heading', { name: 'Agents on this cluster' });
  expect(await screen.findByText('reader')).toBeTruthy();
  expect(screen.getByText('Last checked ' + new Date(fresh.observedAt).toLocaleString())).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('button', { name: 'kubectl context' })));
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  const confirmation = screen.getByRole('group', { name: 'Remove cluster confirmation' });
  expect(confirmation.textContent).toMatch(/only forgets.*Bridge/);
  expect(confirmation.textContent).toMatch(/agents.*kubeconfig.*kept/i);
  await user.click(screen.getByRole('button', { name: 'Keep cluster' }));
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Remove' }));
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('cluster_forget');
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  expect((await screen.findByRole('alert')).textContent).toContain("Couldn't confirm removal");
  expect(confirmation.contains(document.activeElement)).toBe(true);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Keep cluster' }));
  expect(document.body.textContent).not.toContain('SECRET_MARKER');
  expect(observe).not.toHaveBeenCalled();
  fail = false;
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  await screen.findByRole('heading', { name: 'Clusters' });
  expect(screen.queryByRole('region', { name: 'team-dev' })).toBeNull();
  expect(sim.snapshot.places).toHaveLength(1);
  expect(observe).toHaveBeenLastCalledWith({ kind: 'cluster' });
});

test.each([{ check: 'configuration', result: 'unknown' }, { check: 'orkaIdentity', result: 'identityRejected' }])('remembered reconnect failure stays explicit with manual recovery and Remove: %j', async failure => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!;
  mockIPC(command => command === 'cluster_reconnect' ? Promise.reject(failure) : old(command));
  const user = userEvent.setup(); render(<Host />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  await screen.findByRole('alert');
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Remove' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  expect(await contextChoice(user, context.name)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain('Choose a context');
});

test('Manage after an unused Add keeps its already confirmed target', async () => {
  const { ipc } = native(); const user = userEvent.setup(); render(<Host initial={connected} />);
  await user.click(await screen.findByRole('button', { name: 'Add a cluster' }));
  await screen.findByRole('button', { name: 'kubectl context' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(within(row('team-dev')).getByRole('button', { name: 'Manage' }));
  expect(ipc.mock.calls.filter(([command]) => command === 'cluster_reconnect')).toHaveLength(0);
  expect(screen.getByText('Access confirmed')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
});

test('showing context choices does not strand a pending Manage inventory', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  const fresh = { ...primary, stale: false, agents: [{ name: 'reader', namespace: 'agents', uid: 'uid-reader', agentType: null, model: null, tools: [], record: null }] };
  let reads = 0;
  mockIPC(command => command === 'overview_places' ? ++reads === 1 ? pending.promise : { places: [fresh], historyState: 'complete', unobserved: [] } : old(command));
  const user = userEvent.setup(); render(<Host initial={connected} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  await screen.findByText('Reading agents…');
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  expect(await screen.findByText('reader')).toBeTruthy();
  await act(async () => pending.resolve({ places: [{ ...primary, agents: [] }], historyState: 'complete', unobserved: [] }));
  expect(screen.getByText('reader')).toBeTruthy();
  expect(reads).toBe(2);
});

test('returning to Add after reconnect replaces obsolete cached context handles', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!;
  const freshContext = { ...context, id: '00000000-0000-4000-8000-000000000099' };
  let refreshed = false;
  mockIPC(command => {
    if (command === 'cluster_contexts') return [refreshed ? freshContext : context];
    if (command === 'cluster_reconnect') { refreshed = true; return { ...target, context_id: freshContext.id }; }
    return old(command);
  });
  const user = userEvent.setup(); render(<Host />);
  await user.click(await screen.findByRole('button', { name: 'Add a cluster' }));
  expect((await contextChoice(user, context.name) as HTMLButtonElement).value).toBe(context.id);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(within(row('team-dev')).getByRole('button', { name: 'Connect' }));
  await screen.findByRole('button', { name: 'Use this cluster' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  expect((await contextChoice(user, context.name) as HTMLButtonElement).value).toBe(freshContext.id);
});

test('Remove on an unselected remembered entry fences its late reconnect reply', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  mockIPC(command => command === 'cluster_reconnect' ? pending.promise : old(command));
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  observe.mockClear();
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  await screen.findByRole('heading', { name: 'Clusters' });
  await act(async () => pending.resolve(target));
  expect(screen.queryByRole('region', { name: 'team-dev' })).toBeNull();
  expect(observe).not.toHaveBeenCalled();
});

test('a remembered cluster can reload kubeconfig and choose a fresh context manually', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!;
  mockIPC(command => command === 'cluster_reload' ? [context] : old(command));
  const user = userEvent.setup(); render(<Host />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  await screen.findByRole('button', { name: 'Use this cluster' });
  await reloadConfig(user);
  expect(await contextChoice(user, context.name)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain('Choose a context');
});

test('removing a remembered cluster survives a failed passive snapshot and fences late agent inventory', async () => {
  const { ipc, sim } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  mockIPC(command => command === 'overview_places' ? pending.promise : old(command));
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  sim.unavailable = true;
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  await screen.findByRole('heading', { name: 'Clusters' });
  await act(async () => pending.resolve({ places: [{ ...primary, stale: false }], historyState: 'complete', unobserved: [] }));
  expect(screen.queryByRole('region', { name: 'team-dev' })).toBeNull();
  expect(observe).toHaveBeenLastCalledWith({ kind: 'cluster' });
});

test('a remembered Manage entry remains removable when its passive metadata read fails', async () => {
  const { sim } = native(); const user = userEvent.setup(); render(<Host initial={connected} />);
  await screen.findByRole('region', { name: 'team-dev' }); sim.unavailable = true;
  await user.click(within(row('team-dev')).getByRole('button', { name: 'Manage' }));
  await screen.findByText(/not be remembered/i);
  expect(screen.getByRole('button', { name: 'Remove' })).toBeTruthy();
});

test('remembered reconnect revokes the old frontend target before native replies and Back fences its late result', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  mockIPC(command => command === 'cluster_reconnect' ? pending.promise : old(command));
  const observe = vi.fn(); const user = userEvent.setup(); render(<Host initial={connected} observe={observe} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-other' })).getByRole('button', { name: 'Connect' }));
  expect(observe).toHaveBeenLastCalledWith({ kind: 'cluster' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await act(async () => pending.resolve(target));
  expect(screen.getByRole('heading', { name: 'Clusters' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
});

test('returning to Clusters resumes a remembered Connect interrupted by tab navigation', async () => {
  const { ipc } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  let reconnects = 0;
  mockIPC(command => command === 'cluster_reconnect' && ++reconnects === 1 ? pending.promise : old(command));
  const props = { onOpenExample: () => {} };
  const user = userEvent.setup(); const view = render(<Setup {...props} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Connect' }));
  view.rerender(<Setup {...props} active={false} />);
  await act(async () => pending.resolve(target));
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
  view.rerender(<Setup {...props} active />);
  expect(await screen.findByRole('button', { name: 'Use this cluster' })).toBeTruthy();
  expect(reconnects).toBe(2);
});

test.each(['local', 'another cluster'])('late removal completion does not replace %s opened after Back', async destination => {
  const { ipc, sim } = native(); const old = ipc.getMockImplementation()!; const pending = deferred<unknown>();
  const other = { ...target, id: '00000000-0000-4000-8000-000000000004', context_name: secondary.context };
  mockIPC(command => command === 'cluster_forget' ? pending.promise : command === 'cluster_reconnect' ? other : old(command));
  const user = userEvent.setup(); render(<Host initial={connected} />);
  await user.click(within(await screen.findByRole('region', { name: 'team-dev' })).getByRole('button', { name: 'Manage' }));
  await user.click(screen.getByRole('button', { name: 'Remove' }));
  await user.click(screen.getByRole('button', { name: 'Remove cluster' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  if (destination === 'local') await user.click(within(row('This computer')).getByRole('button', { name: 'Set up' }));
  else {
    await user.click(within(row('team-other')).getByRole('button', { name: 'Connect' }));
    sim.snapshot.selectedKey = secondary.key;
    await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
    await screen.findByText('Access confirmed');
  }
  sim.snapshot.places = [secondary];
  await act(async () => pending.resolve(null));
  if (destination === 'local') expect(screen.getByRole('heading', { name: 'Where should your agents run?' })).toBeTruthy();
  else {
    expect(screen.getByText('team-other', { selector: '.cluster-result h2 code' })).toBeTruthy();
    expect(screen.getByText('Access confirmed')).toBeTruthy();
  }
});

test('loading local requirements stays explicit and keeps Continue gated', async () => {
  const { ipc } = native(); const pending = deferred<unknown>(); const old = ipc.getMockImplementation()!;
  ipc.mockImplementation(c => c === 'setup_requirements' ? pending.promise : old(c));
  const user = userEvent.setup(); render(<Host />);
  await user.click(within(await screen.findByRole('region', { name: 'This computer' })).getByRole('button', { name: 'Set up' }));
  expect(screen.getByText(/Reading this build's requirements/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await act(async () => pending.resolve(null));
  expect(screen.getByText(/Requirements are not available yet for this build/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.map(([command]) => command)).not.toContain('setup_select_local');
  await user.click(screen.getByRole('button', { name: 'Continue' }));
  expect(await screen.findByRole('region', { name: 'Local cluster setup' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([command]) => command === 'setup_select_local')).toHaveLength(1);
});


test('Clusters puts its subtitle before Add a cluster', async () => {
  native(); render(<Host />);
  const add = await screen.findByRole('button', { name: 'Add a cluster' });
  const subtitle = within(add.parentElement!).getByText('Choose where agents run');
  expect(subtitle.compareDocumentPosition(add) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});
test('observed row metadata states the agent count once without repeating the check label', async () => {
  const { sim } = native(); sim.snapshot.places = [{ ...primary, stale: false }];
  render(<Host />);
  const remote = await screen.findByRole('region', { name: 'team-dev' });
  expect(within(remote).getByText('0 agents')).toBeTruthy();
  expect(within(remote).getByText(/^Last checked /)).toBeTruthy();
});
