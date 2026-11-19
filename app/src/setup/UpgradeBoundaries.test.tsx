import type { InvokeArgs } from '@tauri-apps/api/core';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import App from '../App';
import Setup from './Setup';
import { openLocalSetup } from './test-navigation';
import type { UpgradeStatus } from './upgrade-api';

const eligible: UpgradeStatus = { state: 'eligible', transaction: null, createdAt: null, sourceReleases: ['0.1.2'], targetRelease: '0.1.3', retainedAllocatedBytes: 1200000000, additionalBytes: 2400000000, credential: 'absent', canCancel: false, busy: false, error: null, repeated: false, canReplace: true, canRestore: false };
const current: UpgradeStatus = { ...eligible, state: 'consumed', transaction: '10000000-0000-4000-8000-000000000001', createdAt: 1700000000, canReplace: false, canRestore: true };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
function isRefresh(args?: InvokeArgs) { return !!args && 'refresh' in args && args.refresh === true; }
function fixture(update: (args: Record<string, unknown>) => unknown, phase = 'ready', returning = true) {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1');
  if (returning) localStorage.setItem('bridge.firstTest.v1', 'attempted');
  const ipc = vi.fn((command: string, args: InvokeArgs = {}): unknown => {
    if (command === 'workspace_upgrade_status') return update({ refresh: isRefresh(args) });
    if (command === 'setup_status') return { selected: 'localVm', localVm: phase, model: phase === 'ready' ? 'available' : 'locked', workspaceReplaced: true, ...(phase === 'failed' ? { workspaceFailure: { cause: 'services', operation: 'start', codes: [] }, localVmError: 'Services did not start' } : {}) };
    if (command === 'setup_select_local') return { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceReplaced: true };
    if (command === 'setup_requirements') return null;
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); return ipc;
}
test.each(['sourceSelected', 'fresh'] as const)('Places is not green during %s attention', async state => {
  fixture(() => ({ ...current, state, error: state === 'fresh' ? 'network' : null })); render(<App />);
  const row = await screen.findByRole('region', { name: 'This computer' });
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain(state === 'fresh' ? 'Workspace update needs attention' : 'Workspace recovery needs attention'));
  expect(within(row).getByText('Needs you', { selector: '.pill' })).toBeTruthy();
  expect(row.textContent).not.toContain('Workspace ready');
});
test('Overview shows recovery even with a ready VM', async () => {
  fixture(() => ({ ...current, state: 'sourceSelected' })); render(<App />);
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace recovery needs attention'));
  expect(document.querySelector('.overview')?.textContent).toContain('Workspace recovery needs attention');
});
test.each(['pending', 'failed'] as const)('independent setup observation blocks replaced admission with %s upgrade status', async observation => {
  const pending = deferred<unknown>(); const notice = vi.fn();
  fixture(() => { if (observation === 'failed') throw Error('Lost upgrade status'); return pending.promise; });
  render(<Setup onOpenExample={() => {}} onWorkspaceAction={notice} />);
  await screen.findByRole('region', { name: 'This computer' });
  await waitFor(() => expect(notice.mock.lastCall?.[3]).toBeTruthy());
  expect(document.querySelector('.setup-model')).toBeNull();
  fireEvent(window, new Event('focus'));
  await screen.findByRole('region', { name: 'This computer' });
});
test.each(['hung', 'failed'] as const)('unknown upgrade status never admits the automatic first test (%s)', async observation => {
  const ipc = fixture(() => { if (observation === 'failed') throw Error('Lost upgrade status'); return new Promise(() => {}); }, 'ready', false);
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    return original(command);
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await screen.findByText('Connected', { selector: '.pill' });
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 550)); });
  expect(ipc.mock.calls.filter(([c]) => c === 'agent_run')).toHaveLength(0);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('pending');
});

test('slow upgrade status does not block or get superseded by recurring setup polls', async () => {
  const update = deferred<unknown>();
  const onStatus = vi.fn();
  fixture(() => update.promise, 'starting');
  render(<Setup onOpenExample={() => {}} onStatus={onStatus} />);
  await waitFor(() => expect(onStatus.mock.calls.length).toBeGreaterThan(1));
  await act(async () => { update.resolve({ ...current, state: 'sourceSelected' }); });
  await screen.findByText(/Workspace setup was interrupted/);
});

test.each(['fresh', 'consumed'] as const)('Manage retains ordinary %s failure restart and details', async state => {
  const ipc = fixture(() => ({ ...current, state }), 'failed'); render(<App />); await openLocalSetup();
  const restart = await screen.findByRole('button', { name: 'Restart workspace' });
  fireEvent.click(screen.getByRole('button', { name: 'Show details' }));
  expect(screen.getByText('Services did not start', { selector: 'pre' })).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  fireEvent.click(restart); await waitFor(() => expect(ipc.mock.calls.map(([c]) => c)).toContain('setup_select_local'));
});
test('Manage resumes a cancelled fresh workspace without losing Restore', async () => {
  const ipc = fixture(() => ({ ...current, state: 'fresh' }), 'cancelled'); render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Restart workspace' }));
  await waitFor(() => expect(ipc.mock.calls.map(([c]) => c)).toContain('setup_select_local'));
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
});
test.each(['starting', 'ready'] as const)('accepted ordinary restart retires the same-transaction final cancellation (%s reply)', async replyPhase => {
  const forward = deferred<unknown>();
  let cached = eligible;
  let phase = 'failed';
  let rejectStart = true;
  const observed = vi.fn();
  const fresh: UpgradeStatus = { ...current, state: 'fresh' };
  const ipc = fixture(() => cached, 'failed', false);
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  const original = ipc.getMockImplementation()!;
  const setup = () => ({ selected: 'localVm', localVm: phase, model: phase === 'ready' ? 'available' : 'locked', workspaceReplaced: true });
  ipc.mockImplementation((command, args) => {
    if (command === 'setup_status') return setup();
    if (command === 'workspace_upgrade_replace') {
      cached = { ...fresh, state: 'sourceSelected', busy: true, canCancel: true };
      phase = 'starting';
      return forward.promise;
    }
    if (command === 'workspace_upgrade_cancel') return 'requested';
    if (command === 'setup_select_local') {
      if (rejectStart) throw Error('Start was not accepted');
      phase = replyPhase;
      cached = replyPhase === 'ready' ? current : { ...fresh, busy: true, canCancel: false };
      return setup();
    }
    return original(command, args);
  });
  render(<Setup onOpenExample={() => {}} onStatus={observed} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  fireEvent(window, new Event('focus'));
  fireEvent.click(await screen.findByRole('button', { name: 'Cancel update' }));
  await screen.findByText(/Cancellation requested/);
  phase = 'cancelled'; cached = fresh;
  await act(async () => { forward.resolve({ outcome: 'cancelled', status: fresh }); });
  await screen.findByText(/Update cancelled/);
  const poll = async () => {
    const reads = observed.mock.calls.length;
    fireEvent(window, new Event('focus'));
    await waitFor(() => expect(observed.mock.calls.length).toBeGreaterThan(reads));
  };
  await poll();
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Start again' }));
  await screen.findByText('The setup choice could not be read or saved.');
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  await poll();
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  rejectStart = false;
  fireEvent.click(await screen.findByRole('button', { name: 'Start again' }));
  await waitFor(() => expect(observed.mock.lastCall?.[0]?.localVm).toBe(replyPhase));
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  await screen.findByText(replyPhase === 'starting' ? 'The workspace is busy. Your agent files are kept.' : 'New workspace selected. Connect the AI explicitly before running your agents.');
  phase = 'ready'; cached = current;
  await poll();
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  expect(screen.getByText('New workspace selected. Connect the AI explicitly before running your agents.')).toBeTruthy();
  await poll();
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'workspace_upgrade_replace')).toEqual([['workspace_upgrade_replace', {}]]);
  expect(ipc.mock.calls.filter(([c]) => c === 'workspace_upgrade_cancel')).toEqual([['workspace_upgrade_cancel', {}]]);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_select_local')).toEqual([['setup_select_local', {}], ['setup_select_local', {}]]);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_cancel_local', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
});
test('first setup retains fresh start-failure recovery', async () => {
  fixture(() => ({ ...current, state: 'fresh' }), 'failed', false); render(<App />); await openLocalSetup();
  expect(await screen.findByRole('button', { name: 'Try again' })).toBeTruthy();
  expect(screen.getByText(/Press Try again to restart the workspace/)).toBeTruthy();
});
test.each(['failed', 'cancelled'])('unknown replacement does not expose ordinary first-setup recovery (%s)', async phase => {
  const update = deferred<unknown>();
  fixture(() => update.promise, phase, false);
  render(<App />);
  await screen.findByRole('list', { name: 'Setup steps' });
  expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Start again' })).toBeNull();
  await act(async () => { update.resolve({ ...current, state: 'fresh' }); });
  await screen.findByRole('button', { name: phase === 'failed' ? 'Try again' : 'Start again' });
});

test('transferred connection uses only shared removal and reconnect remains explicit', async () => {
  let removed = false;
  const ipc = fixture(() => ({ ...current, credential: removed ? 'removed' : 'transferred' }));
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(command => {
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'workspace_upgrade_remove') { removed = true; return { ...current, credential: 'removed' }; }
    return original(command);
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Change' }));
  expect(screen.queryByRole('button', { name: 'Remove saved sign-in' })).toBeNull();
  expect(screen.getByText(/Remove saved connection in the Workspace card/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved connection' }));
  expect(screen.getByText(/every retained workspace that references it/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Remove for all retained workspaces' }));
  await screen.findByRole('button', { name: 'Connect the AI' });
  expect(screen.queryByText('Connected', { selector: '.pill' })).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_connect_model')).toHaveLength(1);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_remove_model' || c === 'agent_run')).toHaveLength(0);
});

test('screen reader ready announcement does not contradict recovery', async () => {
  fixture(() => ({ ...current, state: 'sourceSelected' }), 'ready', false); render(<App />); await openLocalSetup();
  await screen.findByText(/Workspace setup was interrupted/);
  expect(screen.queryByText('Your workspace is ready on this computer.')).toBeNull();
});
test('mutation invalidates prior status reads and refreshes after a lost reply', async () => {
  const stale = deferred<unknown>(); const reply = deferred<unknown>(); let polls = 0;
  const ipc = fixture(() => ++polls === 1 ? eligible : polls === 2 ? stale.promise : current, 'failed');
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation(command => command === 'workspace_upgrade_replace' ? reply.promise : original(command));
  render(<App />); await openLocalSetup();
  fireEvent(window, new Event('focus')); await waitFor(() => expect(polls).toBe(2));
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await act(async () => { reply.resolve(null); });
  expect(polls).toBeGreaterThan(2);
  await act(async () => { stale.resolve(eligible); });
  await screen.findByText(/New workspace selected/);
  expect(screen.queryByRole('button', { name: 'Set up new workspace' })).toBeNull();
});


test.each([false, true])('manual Check again queries the owner, retains Restore on Busy/IO and keeps failure sticky through cache polls (returning=%s)', async returning => {
  let fixed = false; let unavailable = true;
  const ipc = fixture(args => {
    if (args.refresh) { if (unavailable) throw Error('Busy or IO'); fixed = true; }
    return { ...current, retainedAllocatedBytes: fixed ? 1300000000 : 1200000000, error: fixed ? null : 'inUse' };
  }, 'ready', returning);
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  expect(screen.getByText('Showing last observed workspace information. Workspace files have not been checked again.')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  expect(screen.getByText(/Transaction:/).textContent).toContain('1000');
  // Cache can now report repaired state; it is not proof that the owner's read succeeded.
  fixed = true;
  fireEvent(window, new Event('focus'));
  await screen.findByText(/Retained workspace: 1.3 GB allocated/);
  expect(screen.getByText(/Could not check workspace files/)).toBeTruthy();
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
  unavailable = false;
  fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByText('Workspace files checked on request.');
  expect(screen.queryByText(/Could not check workspace files/)).toBeNull();
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toHaveLength(2);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_start', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('fallback Check again uses owner refresh without serialising independent setup observations', async () => {
  const owner = deferred<UpgradeStatus>(); let cachedUnavailable = true; let cached = current;
  const ipc = fixture(args => {
    if (args.refresh) return owner.promise.then(status => { cached = status; return status; });
    if (cachedUnavailable) throw Error('Lost initial observation');
    return cached;
  }, 'ready', false);
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText('Checking workspace files. Showing last observed workspace information.');
  const setupReads = ipc.mock.calls.filter(([c]) => c === 'setup_status').length;
  cachedUnavailable = false; fireEvent(window, new Event('focus'));
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'setup_status').length).toBeGreaterThan(setupReads));
  await screen.findByRole('button', { name: 'Restore previous workspace' });
  expect(screen.getByText('Checking workspace files. Showing last observed workspace information.')).toBeTruthy();
  await act(async () => { owner.resolve({ ...current, state: 'sourceSelected', canReplace: true }); });
  await screen.findByText('Workspace files checked on request.');
  expect(screen.getByRole('button', { name: 'Try setup again' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toHaveLength(1);
});

test.each(['failed', 'succeeded'] as const)('identity-less cache before a %s owner reply preserves the pending check and recovery history', async outcome => {
  let complete!: (value: UpgradeStatus) => void; let fail!: (error: Error) => void;
  const owner = new Promise<UpgradeStatus>((resolve, reject) => { complete = resolve; fail = reject; });
  let cached: UpgradeStatus = { ...current, error: 'inUse' };
  const ipc = fixture(args => args.refresh ? owner : cached);
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText('Checking workspace files. Showing last observed workspace information.');
  // Native publishes its cache before the independent refresh IPC settles.
  cached = { ...current, state: 'conflict', transaction: null, createdAt: null, sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, canRestore: false, error: 'unknown' };
  fireEvent(window, new Event('focus'));
  await screen.findByText('The update could not be completed. Your workspaces are kept. Check again before continuing.');
  expect(screen.getByText('Checking workspace files. Showing last observed workspace information.')).toBeTruthy();
  expect(screen.getByText(/Retained workspace: 1.2 GB allocated/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  expect(screen.getByText(/Transaction:/).textContent).toContain('1000');
  if (outcome === 'failed') {
    await act(async () => { fail(Error('Native owner query failed')); });
    await screen.findByText(/Could not check workspace files/);
    expect(screen.queryByRole('button', { name: 'Restore previous workspace' })).toBeNull();
    expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
    expect(screen.getByText(/Transaction:/).textContent).toContain('1000');
  } else {
    cached = { ...current, retainedAllocatedBytes: 1400000000 };
    await act(async () => { complete(cached); });
    await screen.findByText('Workspace files checked on request.');
    await screen.findByText(/Retained workspace: 1.4 GB allocated/);
    expect(screen.getByRole('button', { name: 'Restore previous workspace' }).matches(':disabled')).toBe(false);
  }
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_restore', 'workspace_upgrade_replace', 'setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('identity loss retires an earlier successful check without carrying its freshness forward', async () => {
  let cached: UpgradeStatus = { ...current, error: 'inUse' };
  fixture(args => args.refresh ? { ...current, error: null } : cached);
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText('Workspace files checked on request.');
  cached = { ...current, state: 'conflict', transaction: null, createdAt: null, canReplace: false, canRestore: false, error: 'unknown' };
  fireEvent(window, new Event('focus'));
  await screen.findByText('Workspace recovery needs attention. Your workspaces are kept. Open the matching Bridge version or ask for help.');
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
});

test('an old manual response cannot overwrite a newer transaction or claim freshness for it', async () => {
  const owner = deferred<unknown>(); let changed = false;
  fixture(args => args.refresh ? owner.promise : { ...current, transaction: changed ? '20000000-0000-4000-8000-000000000002' : current.transaction, error: changed ? null : 'inUse' });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText('Checking workspace files. Showing last observed workspace information.');
  changed = true; fireEvent(window, new Event('focus'));
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  await waitFor(() => expect(screen.getByText(/Transaction:/).textContent).toContain('2000'));
  await act(async () => { owner.resolve({ ...current, state: 'sourceSelected', canReplace: true }); });
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Try setup again' })).toBeNull();
  expect(screen.getByText(/Transaction:/).textContent).toContain('2000');
});

test('an operation invalidates a prior manual owner error without admitting first-test replay', async () => {
  const ipc = fixture(args => { if (args.refresh) throw Error('Busy'); return { ...current, error: 'inUse' }; });
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => command === 'workspace_upgrade_restore'
    ? { ...current, state: 'restored', canRestore: false, canReplace: true } : original(command, args));
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  fireEvent.click(screen.getByRole('button', { name: 'Restore previous workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Restore this workspace' }));
  await waitFor(() => expect(screen.queryByText(/Could not check workspace files/)).toBeNull());
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_connect_model', 'agent_run', 'agent_chat_start', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('same known transaction retains explicit Retry after Busy without laundering the failed owner check', async () => {
  let allocated = 1200000000;
  const ipc = fixture(args => {
    if (args.refresh) throw Error('Busy');
    return { ...current, state: 'sourceSelected', canReplace: true, error: 'inUse', retainedAllocatedBytes: allocated };
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  allocated = 1300000000; fireEvent(window, new Event('focus'));
  await screen.findByText(/Retained workspace: 1.3 GB allocated/);
  expect(screen.getByRole('button', { name: 'Try setup again' }).matches(':disabled')).toBe(false);
  expect(screen.getByText(/Could not check workspace files/)).toBeTruthy();
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toHaveLength(1);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_retry', 'setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('manual owner refresh busy observations say Checking, not Updating, and never acquire Cancel capability', async () => {
  const owner = deferred<unknown>(); let refreshing = false;
  const ipc = fixture(args => {
    if (args.refresh) { refreshing = true; return owner.promise; }
    return { ...current, retainedAllocatedBytes: refreshing ? 1300000000 : 1200000000, error: 'inUse', busy: refreshing, canCancel: false };
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  fireEvent(window, new Event('focus'));
  await screen.findByText(/Retained workspace: 1.3 GB allocated/);
  expect(screen.getByText('Checking workspace files. Showing last observed workspace information.')).toBeTruthy();
  expect(screen.queryByText(/Updating the workspace/)).toBeNull();
  expect(screen.getByRole('contentinfo').textContent).not.toContain('Updating workspace');
  expect(screen.queryByRole('button', { name: 'Cancel update' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  await act(async () => { refreshing = false; owner.resolve(null); });
  await screen.findByText(/Could not check workspace files/);
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toHaveLength(1);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('a pending cache response from before a successful manual check cannot overwrite its repaired status', async () => {
  const stale = deferred<unknown>(); let cached: UpgradeStatus = { ...current, error: 'inUse' }; let hold = false; let held = false;
  fixture(args => {
    if (args.refresh) { cached = { ...current, state: 'sourceSelected', canReplace: true }; return cached; }
    if (hold) { hold = false; held = true; return stale.promise; }
    return cached;
  });
  render(<App />); await openLocalSetup();
  hold = true; fireEvent(window, new Event('focus'));
  await waitFor(() => expect(held).toBe(true));
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText('Workspace files checked on request.');
  await act(async () => { stale.resolve(current); });
  expect(screen.getByRole('button', { name: 'Try setup again' })).toBeTruthy();
  expect(screen.queryByText(/files are still in use/)).toBeNull();
});

test('manual failure retains the known transaction even if an independent cached poll is unreadable', async () => {
  let unavailable = false;
  fixture(args => {
    if (args.refresh) { unavailable = true; throw Error('IO'); }
    if (unavailable) throw Error('Lost cache observation');
    return { ...current, error: 'inUse' };
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  fireEvent(window, new Event('focus'));
  await screen.findByText('Workspace update information is unavailable. Check again before changing your workspace.');
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  expect(screen.getByText(/Transaction:/).textContent).toContain('1000');
  expect(screen.getByText(/Could not check workspace files/)).toBeTruthy();
});

test.each(['conflict', 'none'] as const)('identityless %s after manual failure retains history and fences mutations until explicit refresh', async state => {
  let cached: UpgradeStatus = { ...current, error: 'inUse' }; let repaired = false;
  const ipc = fixture(args => {
    if (args.refresh) { if (!repaired) throw Error('IO'); cached = current; }
    return cached;
  });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  const poll = async (value: UpgradeStatus) => {
    cached = value;
    await act(async () => { fireEvent(window, new Event('focus')); });
  };
  const assertFenced = () => {
    expect(screen.getByText(/Could not check workspace files/)).toBeTruthy();
    expect(screen.getByText(/Showing last observed workspace information/)).toBeTruthy();
    expect(screen.getByText(/Transaction:/).textContent).toContain('1000');
    expect(screen.getByText(/Previous: Bridge 0.1.2. New: Bridge 0.1.3./)).toBeTruthy();
    expect(screen.getByText(/Retained workspace: 1.2 GB allocated. Up to 2.4 GB additional/)).toBeTruthy();
    expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
    for (const name of ['Restore previous workspace', 'Set up new workspace', 'Try setup again', 'Use saved connection', 'Remove saved connection']) {
      expect(screen.queryByRole('button', { name })?.matches(':disabled') ?? true).toBe(true);
    }
    expect(screen.getByRole('button', { name: 'Check again' }).matches(':disabled')).toBe(false);
    expect(document.querySelector('.setup-model')).toBeNull();
  };
  await poll({ ...eligible, state, transaction: null, sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, canReplace: false, error: state === 'conflict' ? 'unknown' : null });
  assertFenced();
  // Neither old metadata nor a metadata change within the old transaction proves repair.
  await poll(current); assertFenced();
  await poll({ ...current, sourceReleases: ['0.1.3'], targetRelease: '0.1.4' }); assertFenced();
  repaired = true;
  fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await screen.findByText('Workspace files checked on request.');
  expect(screen.queryByText(/Could not check workspace files/)).toBeNull();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' }).matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_restore', 'workspace_upgrade_replace', 'setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_start', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
});

test('a positively different known transaction retires an identityless manual failure fence', async () => {
  let cached: UpgradeStatus = { ...current, error: 'inUse' };
  fixture(args => { if (args.refresh) throw Error('IO'); return cached; });
  render(<App />); await openLocalSetup();
  fireEvent.click(await screen.findByRole('button', { name: 'Check again' }));
  await screen.findByText(/Could not check workspace files/);
  cached = { ...current, state: 'conflict', transaction: null, canRestore: false, sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, error: 'unknown' };
  await act(async () => { fireEvent(window, new Event('focus')); });
  expect(screen.getByText(/Could not check workspace files/)).toBeTruthy();
  cached = { ...current, transaction: '20000000-0000-4000-8000-000000000002' };
  await act(async () => { fireEvent(window, new Event('focus')); });
  await waitFor(() => expect(screen.queryByText(/Could not check workspace files/)).toBeNull());
  expect(screen.getByRole('button', { name: 'Restore previous workspace' }).matches(':disabled')).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
  expect(screen.getByText(/Transaction:/).textContent).toContain('2000');
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
});

test('ordinary setup observation retry does not retire pending first-test intent or query its owned journal', async () => {
  vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); localStorage.setItem('bridge.firstTest.v1', 'pending');
  let unavailable = false;
  const ipc = vi.fn((command: string, args: InvokeArgs = {}): unknown => {
    if (command === 'workspace_upgrade_status') {
      if (isRefresh(args)) throw Error('Busy: own ready journal');
      return { ...eligible, state: 'none', sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, canReplace: false };
    }
    if (command === 'setup_status') {
      if (unavailable) throw Error('Transient setup observation');
      return { selected: 'localVm', localVm: 'ready', model: 'available' };
    }
    if (command === 'setup_requirements') return null;
    throw Error('Unexpected ' + command);
  });
  mockIPC(ipc); vi.useFakeTimers();
  try {
    await act(async () => { render(<Setup onOpenExample={() => {}} />); });
    expect(screen.getByRole('region', { name: 'This computer' })).toBeTruthy();
    unavailable = true;
    await act(async () => { fireEvent(window, new Event('focus')); });
    const retry = screen.getByRole('button', { name: 'Check again' });
    expect(screen.getByText('The setup choice could not be read or saved.')).toBeTruthy();
    const reads = ipc.mock.calls.filter(([c]) => c === 'setup_status').length;
    unavailable = false;
    // Do not advance the polling clock: only the explicit click can read Setup again.
    await act(async () => { fireEvent.click(retry); });
    expect(ipc.mock.calls.filter(([c]) => c === 'setup_status')).toHaveLength(reads + 1);
    expect(screen.queryByText('The setup choice could not be read or saved.')).toBeNull();
    expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toEqual([]);
    expect(localStorage.getItem('bridge.firstTest.v1')).toBe('pending');
    expect(screen.queryByText(/Could not check workspace files/)).toBeNull();
    expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
  } finally { cleanup(); vi.useRealTimers(); }
});

test.each(['none', 'eligible'] as const)('unselected upgrade observation failure offers cached retry and admits only known none (%s)', async state => {
  const pending = deferred<unknown>(); let reads = 0;
  const ipc = fixture(() => ++reads === 1 ? pending.promise : { ...eligible, state, canReplace: state === 'eligible' }, 'idle', false);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => command === 'setup_status'
    ? { selected: null, localVm: 'idle', model: 'locked' } : original(command, args));
  render(<Setup onOpenExample={() => {}} />); await openLocalSetup();
  expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(true);
  await act(async () => { pending.resolve(null); });
  await screen.findByText('Workspace update information is unavailable. Check again before changing your workspace.');
  fireEvent.click(screen.getByRole('button', { name: 'Retry workspace status' }));
  await waitFor(() => expect(reads).toBeGreaterThan(1));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' }).matches(':disabled')).toBe(state !== 'none'));
  if (state === 'eligible') expect(screen.getByRole('button', { name: 'Set up new workspace' })).toBeTruthy();
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toEqual([]);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run'].includes(c))).toEqual([]);
});

test.each([['eligible', 'ready'], ['eligible', 'starting'], ['none', 'failed']] as const)('selected unreplaced %s/%s retains an enabled cached retry after a later read failure', async (state, phase) => {
  let unavailable = false; let observedPhase: string = phase;
  const known: UpgradeStatus = state === 'eligible' ? eligible : { ...eligible, state: 'none', sourceReleases: [], targetRelease: null, retainedAllocatedBytes: null, additionalBytes: null, canReplace: false };
  const ipc = fixture(() => { if (unavailable) throw Error('Lost cached observation'); return known; }, phase, false);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => command === 'setup_status'
    ? { selected: 'localVm', localVm: observedPhase, model: observedPhase === 'ready' ? 'available' : 'locked' } : original(command, args));
  render(<Setup onOpenExample={() => {}} />); await openLocalSetup();
  await screen.findByRole('button', { name: state === 'eligible' ? 'Set up new workspace' : 'Try again' });
  unavailable = true; fireEvent(window, new Event('focus'));
  const retry = await screen.findByRole('button', { name: 'Retry workspace status' });
  expect(retry.matches(':disabled')).toBe(false);
  expect(screen.getByText('Workspace update information is unavailable. Check again before changing your workspace.')).toBeTruthy();
  if (state === 'eligible') {
    expect(screen.getByText(/Retained workspace: 1.2 GB allocated/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Update details' }));
    expect(screen.getByText('Previous: Bridge 0.1.2. New: Bridge 0.1.3.')).toBeTruthy();
  }
  unavailable = false; fireEvent.click(retry);
  let action = await screen.findByRole('button', { name: state === 'eligible' ? 'Set up new workspace' : 'Try again' });
  expect(action.matches(':disabled')).toBe(phase === 'starting');
  expect(screen.queryByRole('button', { name: 'Retry workspace status' })).toBeNull();
  if (phase === 'starting') {
    // A cached read can recover during startup, but it does not finish that lifecycle.
    observedPhase = 'failed'; fireEvent(window, new Event('focus'));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Set up new workspace' }).matches(':disabled')).toBe(false));
    action = screen.getByRole('button', { name: 'Set up new workspace' });
  }
  expect(screen.queryByText('Workspace files checked on request.')).toBeNull();
  if (state === 'eligible') {
    fireEvent.click(action);
    expect(screen.getByRole('group', { name: 'Set up new workspace?' })).toBeTruthy();
  }
  expect(ipc.mock.calls.filter(([c, a]) => c === 'workspace_upgrade_status' && isRefresh(a))).toEqual([]);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_select_local', 'setup_connect_model', 'agent_run'].includes(c))).toEqual([]);
});

test.each(['failed', 'cancelled'])('explicit ordinary retry of %s fresh startup keeps ordinary Cancel across an unreadable observation without upgrade authority', async phase => {
  let starting = false; let cancelled = false; let unreadable = false;
  const observed = vi.fn();
  const ipc = fixture(() => ({ ...current, state: 'fresh', busy: starting && !cancelled, canCancel: false }), phase, false);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => {
    if (command === 'setup_select_local') { starting = true; return { selected: 'localVm', localVm: 'starting', model: 'locked', workspaceReplaced: true }; }
    if (command === 'setup_cancel_local') { cancelled = true; return { selected: 'localVm', localVm: 'cancelled', model: 'locked', workspaceReplaced: true }; }
    if (command === 'setup_status' && starting) {
      if (unreadable) throw Error('Transient startup observation');
      return { selected: 'localVm', localVm: cancelled ? 'cancelled' : 'starting', model: 'locked', workspaceReplaced: true };
    }
    return original(command, args);
  });
  localStorage.setItem('bridge.firstTest.v1', 'pending');
  render(<Setup onOpenExample={() => {}} onStatus={observed} />);
  fireEvent.click(await screen.findByRole('button', { name: phase === 'failed' ? 'Try again' : 'Start again' }));
  await screen.findByText('The workspace is busy. Your agent files are kept.');
  unreadable = true; fireEvent(window, new Event('focus'));
  await screen.findByText('The setup choice could not be read or saved.');
  await waitFor(() => expect(observed.mock.lastCall?.[0]).toBeUndefined());
  unreadable = false; fireEvent(window, new Event('focus'));
  await waitFor(() => expect(observed.mock.lastCall?.[0]?.localVm).toBe('starting'));
  await waitFor(() => expect(screen.queryByText('The setup choice could not be read or saved.')).toBeNull());
  const cancel = screen.getByRole('button', { name: /^Cancel$/ });
  expect(cancel.matches(':disabled')).toBe(false);
  expect(screen.queryByRole('button', { name: 'Cancel update' })).toBeNull();
  fireEvent.click(cancel);
  await screen.findByRole('button', { name: 'Start again' });
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_select_local')).toEqual([['setup_select_local', {}]]);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_cancel_local')).toEqual([['setup_cancel_local', {}]]);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['workspace_upgrade_cancel', 'setup_connect_model', 'agent_run', 'agent_chat_send', 'agent_chat_reconnect'].includes(c))).toEqual([]);
  expect(localStorage.getItem('bridge.firstTest.v1')).toBe('attempted');
});

test.each([false, true])('observed fresh version startup cannot ordinary-Cancel without a local start (canCancel=%s)', async canCancel => {
  const ipc = fixture(() => ({ ...current, state: 'fresh', busy: true, canCancel }), 'starting', false);
  render(<Setup onOpenExample={() => {}} />);
  const cancel = await screen.findByRole('button', { name: /^Cancel$/ });
  expect(cancel.matches(':disabled')).toBe(true);
  fireEvent.click(cancel);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_cancel_local' || c === 'workspace_upgrade_cancel')).toEqual([]);
});

test('UI-owned version mutation keeps ordinary Cancel fenced even with a Fresh noncancellable observation', async () => {
  const forward = deferred<unknown>(); let mutating = false;
  const ipc = fixture(() => mutating ? { ...current, state: 'fresh', busy: true, canCancel: false } : eligible, 'failed', false);
  const original = ipc.getMockImplementation()!;
  ipc.mockImplementation((command, args) => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: mutating ? 'starting' : 'failed', model: 'locked', workspaceReplaced: true };
    if (command === 'workspace_upgrade_replace') { mutating = true; return forward.promise; }
    return original(command, args);
  });
  render(<Setup onOpenExample={() => {}} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await screen.findByRole('button', { name: 'Restore previous workspace' });
  expect(screen.getByText('Updating the workspace. Your agent files are kept.')).toBeTruthy();
  const cancel = screen.getByRole('button', { name: /^Cancel$/ });
  expect(cancel.matches(':disabled')).toBe(true);
  fireEvent.click(cancel);
  expect(ipc.mock.calls.filter(([c]) => c === 'setup_cancel_local' || c === 'workspace_upgrade_cancel')).toEqual([]);
  await act(async () => { forward.resolve(null); });
});

test.each([['fresh', 'preparing'], ['fresh', 'installing'], ['fresh', 'starting'], ['consumed', 'preparing'], ['consumed', 'installing'], ['consumed', 'starting']] as const)('independent %s/%s startup observation fences Restore and Remove despite an idle cached projection', async (state, phase) => {
  const ipc = fixture(() => ({ ...current, state, credential: 'transferred', busy: false }), phase);
  render(<Setup onOpenExample={() => {}} />);
  const restore = await screen.findByRole('button', { name: 'Restore previous workspace' });
  const remove = screen.getByRole('button', { name: 'Remove saved connection' });
  expect(restore.matches(':disabled')).toBe(true);
  expect(remove.matches(':disabled')).toBe(true);
  fireEvent.click(restore); fireEvent.click(remove);
  expect(screen.queryByRole('group', { name: 'Restore previous workspace?' })).toBeNull();
  expect(screen.queryByRole('group', { name: 'Remove saved connection?' })).toBeNull();
  expect(ipc.mock.calls.filter(([c]) => c === 'workspace_upgrade_restore' || c === 'workspace_upgrade_remove')).toEqual([]);
});

test('native upgrade Cancel remains available during an independent startup observation', async () => {
  fixture(() => ({ ...current, state: 'fresh', busy: true, canCancel: true }), 'starting', false);
  render(<Setup onOpenExample={() => {}} />);
  const cancel = await screen.findByRole('button', { name: 'Cancel update' });
  expect(cancel.matches(':disabled')).toBe(false);
});

test('a busy noncancellable cached observation after reload is not a replacement or a manual check', async () => {
  fixture(() => ({ ...current, busy: true, canCancel: false }));
  render(<App />); await openLocalSetup();
  await waitFor(() => expect(screen.getByRole('contentinfo').textContent).toContain('Workspace is busy'));
  expect(screen.queryByText(/Updating the workspace/)).toBeNull();
  expect(screen.queryByText('Checking workspace files. Showing last observed workspace information.')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Cancel update' })).toBeNull();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' })).toBeTruthy();
});
