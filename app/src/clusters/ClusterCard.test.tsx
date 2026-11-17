import { chooseContext, contextChoice, reloadConfig } from './test-context';
import { openLocalSetup } from '../setup/test-navigation';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from '../App';
import ClusterCard from './ClusterCard';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name,
  service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'ghcr.io/orka:0.9.4' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
function native(status: object = { selected: null, localVm: 'idle', model: 'locked' }, use: unknown = { status: 'connected', providers: [provider], failure: null }) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'setup_status') return status;
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return use;
    throw Error('Unsupported');
  });
  mockIPC(ipc); return ipc;
}
async function open() {
  const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'I already run Orka on a cluster' }));
  await screen.findByRole('button', { name: 'kubectl context' });
  return user;
}
async function discover(user: ReturnType<typeof userEvent.setup>) {
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
}
beforeEach(() => localStorage.clear());
afterEach(() => { cleanup(); clearMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

test('cluster actions share the footer with retry first and reload quiet', async () => {
  native(); const user = await open(); await discover(user);
  const use = screen.getByRole('button', { name: 'Use this cluster' });
  const reload = screen.getByRole('button', { name: 'Reload kubeconfig' });
  expect(use.parentElement).toBe(reload.parentElement);
  expect(reload.classList.contains('button-quiet')).toBe(true);
  await user.click(use);
  const openAgent = await screen.findByRole('button', { name: 'Open your agent' });
  const row = openAgent.parentElement!;
  expect(screen.getByRole('button', { name: 'Check access again' }).parentElement).toBe(row);
  expect(screen.getByRole('button', { name: 'Reload kubeconfig' }).parentElement).toBe(row);
  expect(screen.getByRole('button', { name: 'Change context' }).parentElement).toBe(row);
  expect(row.querySelectorAll('.button-primary')).toHaveLength(1);
  expect(row.querySelectorAll('.button-secondary')).toHaveLength(2);
  await user.click(screen.getByRole('button', { name: 'Change context' }));
  expect(document.activeElement).toBe(await screen.findByRole('button', { name: 'kubectl context' }));
});

test('single-target discovery keeps This computer selected and successful Use selects the cluster', async () => {
  const ipc = native({ selected: 'localVm', localVm: 'ready', model: 'available' });
  ipc.mockImplementation(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return { status: 'connected', providers: [provider], failure: null };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\\nname: reader\\n---\\nRead.', tests: 'Checks' };
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  const run = await screen.findByRole('button', { name: 'Run tests' });
  await waitFor(() => expect(run.matches(':disabled')).toBe(false));
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  expect(screen.getByRole('button', { name: 'Send and run tests' }).matches(':disabled')).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findAllByText('Access confirmed');
  expect(document.querySelector('.host-chip')?.textContent).toContain(context.name);
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(screen.getByRole('button', { name: 'Send and run tests' }).matches(':disabled')).toBe(true);
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_change_local');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('agent_run');
});

test('failed context discovery leaves local Run and Chat usable without reselecting This computer', async () => {
  const ipc = native();
  const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
  ipc.mockImplementation(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return Promise.reject({ check: 'clusterApi', result: 'connectionFailed' });
    if (command === 'agent_open') return { documentId, name: 'reader', agent: '---\nname: reader\n---\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status' || command === 'agent_run') return { documentId, busy: null, state: 'idle', canRun: true, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'agent_chat_status' || command === 'agent_chat_send') return { documentId, state: 'ready', busy: false, canSend: true, versions: [], error: null };
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await chooseContext(user, context.name);
  await screen.findByRole('alert');
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  const run = await screen.findByRole('button', { name: 'Run tests' });
  await waitFor(() => expect(run.matches(':disabled')).toBe(false));
  await user.click(run);
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toEqual([['agent_run', { documentId }]]));
  await user.click(screen.getByRole('button', { name: 'Chat' }));
  await user.type(await screen.findByRole('textbox', { name: 'Message' }), 'Still local');
  const send = screen.getByRole('button', { name: 'Send message' });
  await waitFor(() => expect(send.matches(':disabled')).toBe(false));
  await user.click(send);
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'agent_chat_send')).toEqual([['agent_chat_send', { documentId, message: 'Still local' }]]));
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('send_start');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_select_local');
});

test('changing a selected remote context revokes its capability before failed discovery returns', async () => {
  const ipc = native(); const pending = deferred<unknown>();
  const selection = { kind: 'cluster' as const, target, providers: [provider], providerId: provider.id };
  ipc.mockImplementation(command => command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? pending.promise : Promise.reject('Unsupported'));
  const onPlace = vi.fn(); const user = userEvent.setup();
  render(<ClusterCard selection={selection} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await chooseContext(user, context.name);
  expect(onPlace).toHaveBeenCalledExactlyOnceWith({ kind: 'cluster' });
  await act(async () => pending.resolve(Promise.reject({ check: 'clusterApi', result: 'connectionFailed' })));
  await screen.findByRole('alert');
  expect(onPlace).toHaveBeenCalledExactlyOnceWith({ kind: 'cluster' });
  expect(screen.queryByRole('button', { name: 'Open your agent' })).toBeNull();
});

test('a selected unavailable workspace can choose a cluster without resetting it', async () => {
  const ipc = native({ selected: 'localVm', localVm: 'notIntegrated', model: 'locked' });
  const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use a cluster you already have instead' }));
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_change_local');
});

test('a failed account attempt keeps the native-requested name field for correction', async () => {
  const ipc = native(undefined, { status: 'needServiceAccount', providers: [], failure: null });
  const user = await open(); await discover(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  const account = await screen.findByRole('textbox', { name: 'ServiceAccount name' });
  await user.type(account, 'misspelled');
  ipc.mockImplementation(command => command === 'cluster_use' ? { status: 'unavailable', providers: [], failure: { check: 'serviceAccount', result: 'unknown' } } : Promise.reject('Unsupported'));
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByRole('alert');
  expect(screen.getByRole('textbox', { name: 'ServiceAccount name' })).toBe(account);
});

test('local results remain explicitly local after confirming cluster access', async () => {
  native();
  mockIPC(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\\nname: reader\\n---\\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: 'complete', canRun: true, report: { verdict: 'passed', cases: [{ id: 'check', question: 'What did the page say?', phase: 'Succeeded', verdict: 'passed', answer: 'yes', completedTools: [], assertions: [{ id: 'yes', kind: 'contains', expected: 'checksum', verdict: 'passed' }] }] }, acknowledged: [], pending: null, error: null };
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return { status: 'connected', providers: [provider], failure: null };
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Designer' }));
  await screen.findByText('1 of 1 passed', { selector: '.checks-status' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await screen.findByRole('button', { name: 'kubectl context' }); await discover(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findAllByText('Access confirmed');
  expect(screen.getByText('This computer: 1 of 1 passed', { selector: '.checks-status' })).toBeTruthy();
});

test('local model acknowledgement survives a cluster round trip', async () => {
  native();
  mockIPC(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'cluster_contexts') return [context];
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await within(screen.getByRole('region', { name: 'Connect the AI' })).findByText('Connected');
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await screen.findByRole('button', { name: 'kubectl context' });
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await openLocalSetup(user);
  expect(await within(screen.getByRole('region', { name: 'Connect the AI' })).findByText('Connected')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Connect the AI' })).toBeNull();
});

test('local-result observation continues while remote access is unconfirmed', async () => {
  native(); let complete = false;
  mockIPC(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_requirements') return null;
    if (command === 'agent_open') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', name: 'web-reader', agent: '---\\nname: reader\\n---\\nRead.', tests: 'Checks' };
    if (command === 'agent_run_status') return { documentId: '9b2b93c4-375c-4815-9717-39852a96e411', busy: null, state: complete ? 'complete' : 'running', canRun: false, report: complete ? { verdict: 'passed', cases: [{ id: 'check', question: 'What did the page say?', phase: 'Succeeded', verdict: 'passed', answer: 'yes', completedTools: [], assertions: [{ id: 'yes', kind: 'contains', expected: 'checksum', verdict: 'passed' }] }] } : null, acknowledged: [], pending: null, error: null };
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    throw Error('Unsupported');
  });
  const user = userEvent.setup(); render(<App />); await openLocalSetup();
  await user.click(await screen.findByRole('button', { name: 'Designer' }));
  await screen.findByText('Running tests…', { selector: '.checks-status' });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(screen.getByRole('button', { name: 'Add a cluster' }));
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
  complete = true;
  expect(await screen.findByText('1 of 1 passed', { selector: '.checks-status' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Designer' }));
  expect(document.querySelector('.host-chip')?.textContent).toContain('This computer');
  expect(screen.getByRole('button', { name: 'Run tests' }).matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Send and run tests' }).matches(':disabled')).toBe(true);
});

test('expert_skip_selects_cluster_without_vm', async () => {
  const ipc = native(); await open();
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(localStorage.getItem('bridge.welcomeSeen.v1')).toBe('1');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_select_local');
});
test('v6_cluster_card_keyboard and Use confirms access only after discovery', async () => {
  const ipc = native(); const user = await open();
  const select = screen.getByRole('button', { name: 'kubectl context' }); select.focus();
  await chooseContext(user, context.name); await screen.findByText('Cluster found');
  expect(screen.queryByText('ghcr.io/orka:0.9.4')).toBeNull();
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('cluster_use');
  const use = screen.getByRole('button', { name: 'Use this cluster' }); use.focus(); await user.keyboard('{Enter}');
  expect((await screen.findAllByText('Access confirmed')).length).toBeGreaterThan(0);
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use')).toEqual([['cluster_use', { targetId: target.id, saName: null }]]);
  await user.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('ghcr.io/orka:0.9.4')).toBeTruthy();
  expect(screen.getByText('control / orka', { selector: 'dd' })).toBeTruthy(); expect(screen.getByText('agents', { selector: 'dd' })).toBeTruthy();
  expect(screen.getByText('team-model')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Open your agent' }).matches(':disabled')).toBe(false);
});
test('native_need_account_only_then_name_field', async () => {
  const ipc = native(undefined, { status: 'needServiceAccount', providers: [], failure: null }); const user = await open(); await discover(user);
  expect(screen.queryByRole('textbox', { name: 'ServiceAccount name' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  const account = await screen.findByRole('textbox', { name: 'ServiceAccount name' });
  expect(document.activeElement).toBe(account); await user.type(account, 'bridge-user');
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use').at(-1)).toEqual(['cluster_use', { targetId: target.id, saName: 'bridge-user' }]);
});
test.each(['unsupportedCredential', 'denied', 'unavailable'])('unsupported_access_stays_visible: %s', async status => {
  native(undefined, { status, providers: [], failure: { check: status === 'unsupportedCredential' ? 'credentials' : 'providers', result: status === 'denied' ? 'permissionDenied' : status === 'unsupportedCredential' ? 'unsupportedCredential' : 'unknown' } }); const user = await open(); await discover(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  const alert = await screen.findByRole('alert');
  expect(alert).toBeTruthy(); expect(screen.getByText('Cluster found')).toBeTruthy();
  if (status === 'unsupportedCredential') expect(alert.textContent).toMatch(/credential plugin outside Bridge/);
  if (status === 'unavailable') {
    expect(alert.textContent).toMatch(/read AI connections/);
    expect(alert.textContent).toMatch(/reason is unknown/);
  }
  expect(screen.queryByRole('textbox', { name: 'ServiceAccount name' })).toBeNull();
});
test('failure alternative does not stop or reset a local VM', async () => {
  const ipc = native({ selected: 'localVm', localVm: 'failed', localVmError: 'vmStart: unavailable', model: 'locked' });
  const user = userEvent.setup(); render(<App />);
  await user.click(await screen.findByRole('button', { name: 'Use a cluster you already have instead' }));
  await screen.findByRole('button', { name: 'kubectl context' });
  await waitFor(() => expect(ipc.mock.calls.some(([c]) => c === 'setup_status')).toBe(true));
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_change_local');
  expect(ipc.mock.calls.map(([c]) => c)).not.toContain('setup_cancel_local');
  await user.click(screen.getByRole('button', { name: 'Back to clusters' }));
  await openLocalSetup(user);
  expect(await screen.findByRole('button', { name: 'Try again' })).toBeTruthy();
});
test('unsupported discovery names the access limitation and keeps the context', async () => {
  native(); mockIPC(command => command === 'setup_status' ? { selected: null, localVm: 'idle', model: 'locked' }
    : command === 'cluster_contexts' ? [context] : Promise.reject({ check: 'credentials', result: 'unsupportedCredential' }));
  const user = await open(); await chooseContext(user, context.name);
  expect(await screen.findByText(/credential plugin outside Bridge/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain(context.name);
});
test('empty discovery is not an access error or a made-up version', async () => {
  native(); mockIPC(command => command === 'setup_status' ? { selected: null, localVm: 'idle', model: 'locked' }
    : command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? [] : Promise.reject('Unsupported'));
  const user = await open(); await chooseContext(user, context.name);
  expect(await screen.findByText('No cluster with Orka was found using this context.')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Use this cluster' })).toBeNull();
});

const connected = { status: 'connected', providers: [provider], failure: null };
const fresh = { id: '00000000-0000-4000-8000-000000000004', name: 'corrected', current: false };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
async function selectDirect(user: ReturnType<typeof userEvent.setup>) {
  await waitFor(() => expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(false));
  await chooseContext(user, context.name);
  await screen.findByText('Cluster found');
}

test('deactivating hides the card, clears confirmed access and admitted locks, but keeps remembered context, target and provider; reactivating needs a fresh explicit Use', async () => {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? [target]
    : command === 'cluster_use' ? connected : [fresh]);
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  const view = render(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await selectDirect(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Access confirmed');
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster', target, providers: [provider], providerId: provider.id });

  view.rerender(<ClusterCard active={false} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(document.querySelector('.cluster-card')?.hasAttribute('hidden')).toBe(true);

  view.rerender(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(document.querySelector('.cluster-card')?.hasAttribute('hidden')).toBe(false);
  expect(screen.getByRole('button', { name: 'kubectl context' }).textContent).toContain(context.name);
  expect(screen.getByText('Cluster found')).toBeTruthy();
  expect(screen.queryByText('Access confirmed')).toBeNull();
  const use = screen.getByRole('button', { name: 'Use this cluster' });
  expect(use.matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use')).toHaveLength(1);

  await user.click(use);
  await screen.findByText('Access confirmed');
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use')).toHaveLength(2);
  expect((screen.getByRole('combobox', { name: 'AI connection' }) as HTMLSelectElement).value).toBe(provider.id);
});

test('a late Use reply after deactivation cannot publish the connected place or resurrect confirmed status', async () => {
  vi.stubGlobal('isTauri', true);
  const pending = deferred<unknown>();
  const ipc = vi.fn((command: string) => command === 'cluster_contexts' ? [context] : command === 'cluster_discover' ? [target]
    : command === 'cluster_use' ? pending.promise : [fresh]);
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  const view = render(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await selectDirect(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  expect(screen.getByRole('button', { name: 'Use this cluster' }).matches(':disabled')).toBe(true);
  const callsBeforeDeactivate = onPlace.mock.calls.length;

  view.rerender(<ClusterCard active={false} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await act(async () => pending.resolve(connected));
  expect(onPlace.mock.calls.length).toBe(callsBeforeDeactivate);

  view.rerender(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(screen.queryByText('Access confirmed')).toBeNull();
  expect(screen.getByRole('button', { name: 'Use this cluster' }).matches(':disabled')).toBe(false);
});

test('an initial kubeconfig read interrupted by deactivation resumes on reactivation, discarding the stale reply', async () => {
  vi.stubGlobal('isTauri', true);
  const first = deferred<unknown>(); const second = deferred<unknown>(); let calls = 0;
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_contexts') { calls++; return calls === 1 ? first.promise : second.promise; }
    throw Error('Unsupported');
  });
  mockIPC(ipc);
  const onPlace = vi.fn(); const view = render(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(true);

  view.rerender(<ClusterCard active={false} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  view.rerender(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(calls).toBe(2);

  await act(async () => first.resolve([{ id: 'stale', name: 'stale-context', current: false }]));
  expect(screen.queryByText('stale-context')).toBeNull();
  expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(true);

  await act(async () => second.resolve([context]));
  await waitFor(() => expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(false));
  expect(await contextChoice(userEvent.setup(), context.name)).toBeTruthy();
});

test('a Reload kubeconfig interrupted by deactivation resumes the reload on reactivation, discarding the stale reply', async () => {
  vi.stubGlobal('isTauri', true);
  const reloadFirst = deferred<unknown>(); const reloadSecond = deferred<unknown>(); let reloadCalls = 0;
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_reload') { reloadCalls++; return reloadCalls === 1 ? reloadFirst.promise : reloadSecond.promise; }
    throw Error('Unsupported');
  });
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  const view = render(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await selectDirect(user);
  await reloadConfig(user);
  expect(screen.getByRole('button', { name: 'Reloading kubeconfig…' })).toBeTruthy();

  view.rerender(<ClusterCard active={false} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  view.rerender(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(reloadCalls).toBe(2);

  await act(async () => reloadFirst.resolve([{ id: 'stale', name: 'stale-context', current: false }]));
  expect(screen.queryByText('stale-context')).toBeNull();
  expect(screen.getByRole('button', { name: 'Reloading kubeconfig…' })).toBeTruthy();

  await act(async () => reloadSecond.resolve([context]));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Reload kubeconfig' })).toBeTruthy());
  expect(await contextChoice(userEvent.setup(), context.name)).toBeTruthy();
});

test.each(['interrupted reload', 'failed initial read', 'failed reload'])('returning after %s reads contexts again and discards stale replies and errors', async operation => {
  vi.stubGlobal('isTauri', true);
  const oldReload = deferred<unknown>(); const freshRead = deferred<unknown>(); let reads = 0;
  const failure = { check: 'configuration', result: 'unknown' };
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_contexts') {
      if (++reads > 1) return freshRead.promise;
      return operation === 'failed initial read' ? Promise.reject(failure) : [context];
    }
    if (command === 'cluster_reload') return operation === 'failed reload' ? Promise.reject(failure) : oldReload.promise;
    throw Error('Unsupported');
  });
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup(); const entry = { revision: 1 };
  const view = render(<ClusterCard active entry={entry} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  if (operation === 'failed initial read') await screen.findByRole('alert');
  else {
    await contextChoice(user, context.name);
    await reloadConfig(user);
    if (operation === 'failed reload') await screen.findByRole('alert');
  }
  expect(reads).toBe(1);
  expect(ipc.mock.calls.filter(([command]) => command === 'cluster_reload')).toHaveLength(operation === 'failed initial read' ? 0 : 1);

  view.rerender(<ClusterCard active={false} entry={entry} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  view.rerender(<ClusterCard active entry={operation === 'failed initial read' ? entry : { revision: 2 }} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(true);

  await act(async () => oldReload.resolve([{ id: 'stale', name: 'stale-context', current: false }]));
  expect(screen.queryByText('stale-context')).toBeNull();
  expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(true);
  await act(async () => freshRead.resolve([context]));
  await waitFor(() => expect(screen.getByRole('button', { name: 'kubectl context' }).matches(':disabled')).toBe(false));
  expect(await contextChoice(userEvent.setup(), context.name)).toBeTruthy();
  expect(screen.queryByText(/No kubectl contexts were found/)).toBeNull();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(onPlace).toHaveBeenCalledTimes(operation === 'failed initial read' ? 0 : 1);
  const completedReads = reads;
  view.rerender(<ClusterCard active entry={{ revision: 3 }} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(reads).toBe(completedReads);
});

test('reactivating after a provider reconnect remaps the remembered provider name to its freshly minted ID', async () => {
  vi.stubGlobal('isTauri', true);
  const providerA = { id: '00000000-0000-4000-8000-000000000010', name: 'team-model', ready: true };
  const providerB = { id: '00000000-0000-4000-8000-000000000011', name: 'other-model', ready: true };
  const providerA2 = { id: '00000000-0000-4000-8000-000000000020', name: 'team-model', ready: true };
  const providerB2 = { id: '00000000-0000-4000-8000-000000000021', name: 'other-model', ready: true };
  let useCalls = 0;
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_contexts') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') {
      useCalls++;
      return useCalls === 1 ? { status: 'connected', providers: [providerA, providerB], failure: null }
        : { status: 'connected', providers: [providerA2, providerB2], failure: null };
    }
    throw Error('Unsupported');
  });
  mockIPC(ipc);
  const onPlace = vi.fn(); const user = userEvent.setup();
  const view = render(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  await selectDirect(user);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Access confirmed');
  await user.selectOptions(screen.getByRole('combobox', { name: 'AI connection' }), providerB.id);
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster', target, providers: [providerA, providerB], providerId: providerB.id });

  view.rerender(<ClusterCard active={false} onPlace={onPlace} onOpenAgent={vi.fn()} />);
  view.rerender(<ClusterCard active onPlace={onPlace} onOpenAgent={vi.fn()} />);
  expect(screen.queryByText('Access confirmed')).toBeNull();

  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Access confirmed');
  expect((screen.getByRole('combobox', { name: 'AI connection' }) as HTMLSelectElement).value).toBe(providerB2.id);
  expect(onPlace).toHaveBeenLastCalledWith({ kind: 'cluster', target, providers: [providerA2, providerB2], providerId: providerB2.id });
});

test('ClusterCard stays active by default for callers that do not pass the prop', async () => {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'cluster_contexts' ? [context] : Promise.reject('Unsupported'));
  render(<ClusterCard onPlace={vi.fn()} onOpenAgent={vi.fn()} />);
  expect(await screen.findByRole('button', { name: 'kubectl context' })).toBeTruthy();
  expect(document.querySelector('.cluster-card')?.hasAttribute('hidden')).toBe(false);
});
