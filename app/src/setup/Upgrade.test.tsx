import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import Upgrade from './Upgrade';
import type { UpgradeStatus } from './upgrade-api';

const transaction = '10000000-0000-4000-8000-000000000001';
const eligible: UpgradeStatus = { state: 'eligible', transaction: null, createdAt: null, sourceReleases: ['0.1.2'], targetRelease: '0.1.3', retainedAllocatedBytes: 1200000000, additionalBytes: 2400000000, credential: 'eligible', canCancel: false, busy: false, error: null, repeated: false, canReplace: true, canRestore: false };
const fresh: UpgradeStatus = { ...eligible, state: 'consumed', transaction, createdAt: 1700000000, canReplace: false, canRestore: true };
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function mount(status = eligible) {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn<(command: string) => unknown>(command => command === 'workspace_upgrade_replace' || command === 'workspace_upgrade_retry' ? { outcome: 'completed', status: fresh } : command === 'workspace_upgrade_cancel' ? 'requested' : fresh); mockIPC(ipc);
  const onChange = vi.fn(); const onBusy = vi.fn();
  const view = render(<Upgrade status={status} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  return { ...view, ipc, onChange, onBusy };
}
test.each(['eligible', 'absent'] as const)('eligible workspace shows saved-connection copy only for an %s credential', credential => {
  const { ipc } = mount({ ...eligible, credential });
  expect(!!screen.queryByText('A saved connection can be reused without connecting automatically.')).toBe(credential === 'eligible');
  expect(screen.queryByRole('button', { name: 'Use saved connection' })).toBeNull();
  expect(ipc).not.toHaveBeenCalled();
});
test('transaction witness is one text run for the native accessibility phase acknowledgement', () => {
  const { container, ipc } = mount({ ...fresh, state: 'sourceRetained', transaction: '40000000-0000-4000-8000-000000000005', canReplace: true });
  fireEvent.click(screen.getByText('Update details'));
  const code = container.querySelector('code');
  expect(code?.textContent).toBe('4000…0005');
  // WebKit exposes adjacent DOM Text nodes as separate AXStaticText values.
  expect(Array.from(code!.childNodes).filter(node => node.nodeType === Node.TEXT_NODE).map(node => node.textContent)).toEqual(['4000…0005']);
  expect(ipc).not.toHaveBeenCalled();
});
test('new workspace is explicit, measured, and Not now cannot switch or reuse a credential', () => {
  const { ipc } = mount();
  const size = (gb: number) => `${new Intl.NumberFormat(undefined, { minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(gb)} GB`;
  expect(screen.getByText(text => text.includes(size(1.2)) && text.includes(size(2.4)))).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  const confirm = screen.getByRole('group', { name: 'Set up new workspace?' });
  expect(within(confirm).getByText(/Do not open the older version until Restore has completed/)).toBeTruthy();
  expect(within(confirm).getByText(/Old cluster objects, chats and volumes/)).toBeTruthy();
  fireEvent.click(within(confirm).getByRole('button', { name: 'Not now' }));
  expect(ipc).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Set up new workspace' }));
});
test('confirmed replacement does not connect or replay any test and drops old model queues', async () => {
  const { ipc, onBusy, onChange } = mount();
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await waitFor(() => expect(onChange).toHaveBeenCalledWith(fresh));
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_replace', {});
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
});
test.each(['eligible', 'absent', 'attention', 'transferred', 'removed'] as const)('saved connection handling is explicit for %s', async credential => {
  const { ipc } = mount({ ...fresh, credential });
  if (credential === 'eligible') {
    fireEvent.click(screen.getByRole('button', { name: 'Use saved connection' }));
    await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_transfer', { transaction }));
    expect(screen.getByText(/Connect the AI explicitly/)).toBeTruthy();
  } else {
    expect(screen.queryByRole('button', { name: 'Use saved connection' })).toBeNull();
    if (credential === 'attention') expect(screen.getByText(/needs attention/)).toBeTruthy();
    if (credential === 'absent' || credential === 'removed') expect(screen.getByText(/Enter a new sign-in or key in the AI card/)).toBeTruthy();
    if (credential === 'transferred') expect(screen.getByText(/Use Connect in the AI card/)).toBeTruthy();
  }
});
test('restore confirmation names the exact pair and warns that the newer workspace cannot reopen', async () => {
  const { ipc } = mount(fresh);
  expect(screen.queryByText(transaction)).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Restore previous workspace' }));
  const confirm = screen.getByRole('group', { name: 'Restore previous workspace?' });
  expect(screen.queryByRole('button', { name: 'Use saved connection' })).toBeNull();
  expect(within(confirm).getByText(/Restore the workspace.*Bridge 0.1.2/)).toBeTruthy();
  expect(within(confirm).getByText(/Bridge 0.1.3/)).toBeTruthy();
  expect(within(confirm).getByText(/cannot be reopened/)).toBeTruthy();
  expect(within(confirm).getByText(/Removing sign-in in the older app/)).toBeTruthy();
  fireEvent.click(within(confirm).getByRole('button', { name: 'Restore this workspace' }));
  await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_restore', { transaction }));
});
test('a fresh workspace cannot change credentials before its first boot completes', () => {
  const { rerender, ipc, onChange, onBusy } = mount({ ...fresh, state: 'fresh' });
  rerender(<Upgrade status={{ ...fresh, state: 'fresh' }} canUseCredential={false} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  const reuse = screen.getByRole('button', { name: 'Use saved connection' });
  expect(reuse.matches(':disabled')).toBe(true);
  fireEvent.click(reuse); expect(ipc).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Restore previous workspace' }).matches(':disabled')).toBe(false);
});

test('shared credential removal cannot happen before warning and confirmation', async () => {
  const { ipc } = mount({ ...fresh, credential: 'attention' });
  expect(screen.getByRole('button', { name: 'Remove saved connection' }).classList.contains('button-danger')).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved connection' }));
  const confirm = screen.getByRole('group', { name: 'Remove saved connection?' });
  expect(within(confirm).getByText(/every retained workspace/)).toBeTruthy();
  expect(ipc).not.toHaveBeenCalled();
  expect(within(confirm).getByRole('button', { name: 'Remove for all retained workspaces' }).classList.contains('button-danger')).toBe(true);
  fireEvent.click(within(confirm).getByRole('button', { name: 'Remove for all retained workspaces' }));
  await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_remove', { transaction }));
});
test('interrupted setup retries only its transaction and busy cancellation preserves recovery', async () => {
  const { ipc, rerender, onChange, onBusy } = mount({ ...fresh, state: 'incomplete', canReplace: true });
  expect(screen.getByText(/Choose Try setup again to continue/)).toBeTruthy();
  expect(screen.getByText(/Reopen this version of Bridge if recovery is interrupted again/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Try setup again' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_retry', { transaction }));
  rerender(<Upgrade status={{ ...fresh, state: 'incomplete', canReplace: false, busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  await waitFor(() => expect(ipc).toHaveBeenLastCalledWith('workspace_upgrade_cancel', {}));
});
test('Cancel remains usable while this renderer awaits replacement, without releasing its busy gate early', async () => {
  const { ipc, rerender, onChange, onBusy } = mount();
  let finish!: (value: unknown) => void;
  ipc.mockImplementation(command => command === 'workspace_upgrade_replace' ? new Promise(resolve => { finish = resolve; }) : 'requested');
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  rerender(<Upgrade status={{ ...eligible, busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  const cancel = screen.getByRole('button', { name: 'Cancel update' });
  expect(cancel.matches(':disabled')).toBe(false); fireEvent.click(cancel);
  await waitFor(() => expect(ipc).toHaveBeenLastCalledWith('workspace_upgrade_cancel', {}));
  expect(onBusy.mock.calls).toEqual([[true]]);
  finish({ outcome: 'completed', status: fresh });
  await waitFor(() => expect(onBusy.mock.calls).toEqual([[true], [false]]));
});

test('busy does not imply replacement or cancellation without the native capability', () => {
  mount({ ...fresh, busy: true, canCancel: false });
  expect(screen.queryByRole('button', { name: 'Cancel update' })).toBeNull();
  expect(screen.getByRole('status').textContent).toBe('The workspace is busy. Your agent files are kept.');
  expect(screen.queryByText(/Updating the workspace/)).toBeNull();
});
test.each(['requested', 'completed', 'notCancellable'] as const)('cancel %s is acknowledged truthfully and never releases the outstanding forward fence', async outcome => {
  const { ipc, rerender, onChange, onBusy } = mount();
  let finish!: (value: unknown) => void;
  ipc.mockImplementation(command => command === 'workspace_upgrade_replace'
    ? new Promise(resolve => { finish = resolve; }) : command === 'workspace_upgrade_cancel' ? outcome : fresh);
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  rerender(<Upgrade status={{ ...eligible, busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  await screen.findByText(outcome === 'requested' ? /Cancellation requested/ : outcome === 'completed' ? /Cancellation completed/ : /can no longer be cancelled/);
  expect(screen.queryByText(/Wait for it to finish/)).toBeNull();
  expect(onBusy.mock.calls).toEqual([[true]]);
  expect(onChange).not.toHaveBeenCalled();
  expect(screen.queryByText(/New workspace selected|Connected/)).toBeNull();
  expect(ipc.mock.calls).not.toContainEqual(['workspace_upgrade_status', expect.objectContaining({ refresh: true })]);
  expect(ipc.mock.calls.map(([c]) => c).filter(c => ['setup_connect_model', 'agent_run', 'agent_chat_start'].includes(c))).toEqual([]);
  await act(async () => { finish({ outcome: 'cancelled', status: { ...fresh, state: 'incomplete', canReplace: true } }); });
  await waitFor(() => expect(onBusy.mock.calls).toEqual([[true], [false]]));
});
test.each(['replace', 'retry'] as const)('forward %s cancellation is not reported as completion or connection', async action => {
  const { ipc, onChange, onBusy, rerender } = mount(action === 'retry' ? { ...fresh, state: 'incomplete', canReplace: true } : eligible);
  ipc.mockReturnValue({ outcome: 'cancelled', status: { ...fresh, state: 'incomplete', canReplace: true } });
  fireEvent.click(screen.getByRole('button', { name: action === 'retry' ? 'Try setup again' : 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await waitFor(() => expect(onChange).toHaveBeenCalled());
  rerender(<Upgrade status={{ ...fresh, state: 'incomplete', canReplace: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  await screen.findByText(/Update cancelled/);
  expect(onChange).toHaveBeenCalledWith({ ...fresh, state: 'incomplete', canReplace: true });
  expect(screen.queryByText(/New workspace selected|Connected/)).toBeNull();
});
test('a late cancellation acknowledgement cannot replace the final forward outcome', async () => {
  const { ipc, rerender, onChange, onBusy } = mount();
  let finish!: (value: unknown) => void; let acknowledge!: (value: unknown) => void;
  ipc.mockImplementation(command => command === 'workspace_upgrade_replace'
    ? new Promise(resolve => { finish = resolve; }) : new Promise(resolve => { acknowledge = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  rerender(<Upgrade status={{ ...eligible, busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  await act(async () => { finish({ outcome: 'cancelled', status: { ...fresh, state: 'incomplete', canReplace: true } }); });
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
  expect(onChange).toHaveBeenCalledWith({ ...fresh, state: 'incomplete', canReplace: true });
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  await act(async () => { acknowledge('requested'); });
  // The parent still has the old transaction: finalOutcome cannot mask a stale ACK.
  expect(screen.queryByText(/Cancellation requested/)).toBeNull();
  rerender(<Upgrade status={{ ...fresh, state: 'incomplete', canReplace: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  await screen.findByText(/Update cancelled/);
  expect(screen.queryByText(/Cancellation requested/)).toBeNull();
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
});
test.each(['requested', 'completed', 'notCancellable'] as const)('observed cancel %s expires at terminal status but not same-operation cached polls', async outcome => {
  const observed = { ...fresh, state: 'sourceSelected' as const, busy: true, canCancel: true };
  const { ipc, rerender, onChange, onBusy } = mount(observed); ipc.mockReturnValue(outcome);
  const show = (status: UpgradeStatus) => rerender(<Upgrade status={status} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  const message = outcome === 'requested' ? /Cancellation requested/ : outcome === 'completed' ? /Cancellation completed/ : /can no longer be cancelled/;
  await screen.findByText(message);
  show({ ...observed }); expect(screen.getByText(message)).toBeTruthy();
  show({ ...observed, canCancel: false }); expect(screen.getByText(message)).toBeTruthy();
  show({ ...observed, busy: false }); expect(screen.getByText(message)).toBeTruthy();
  show(fresh);
  expect(screen.queryByText(message)).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('New workspace selected');
  expect(onBusy).not.toHaveBeenCalled(); expect(onChange).not.toHaveBeenCalled();
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_cancel', {});
});
test.each(['terminal', 'other-transaction'] as const)('late observer cancellation cannot overwrite %s status', async change => {
  const { ipc, rerender, onChange, onBusy } = mount({ ...fresh, state: 'sourceSelected', busy: true, canCancel: true });
  let acknowledge!: (value: unknown) => void; ipc.mockImplementation(() => new Promise(resolve => { acknowledge = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  rerender(<Upgrade status={change === 'terminal' ? fresh : { ...fresh, transaction: '20000000-0000-4000-8000-000000000002', state: 'sourceSelected', busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  await act(async () => { acknowledge('requested'); });
  expect(screen.queryByText(/Cancellation requested/)).toBeNull();
  expect(screen.getByRole('status').textContent).toContain(change === 'terminal' ? 'New workspace selected' : 'Updating the workspace');
  expect(onBusy).not.toHaveBeenCalled(); expect(onChange).not.toHaveBeenCalled();
});
test('a different active transaction retires an already displayed observer acknowledgement', async () => {
  const { ipc, rerender, onChange, onBusy } = mount({ ...fresh, state: 'sourceSelected', busy: true, canCancel: true });
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  await screen.findByText(/Cancellation requested/);
  rerender(<Upgrade status={{ ...fresh, transaction: '20000000-0000-4000-8000-000000000002', state: 'sourceSelected', busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.queryByText(/Cancellation requested/)).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('Updating the workspace');
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_cancel', {});
});
test('a terminal cache observation cannot release an outstanding local forward command', async () => {
  const { ipc, rerender, onChange, onBusy } = mount();
  let finish!: (value: unknown) => void;
  ipc.mockImplementation(command => command === 'workspace_upgrade_replace' ? new Promise(resolve => { finish = resolve; }) : 'requested');
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  rerender(<Upgrade status={{ ...fresh, busy: true, canCancel: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel update' }));
  await screen.findByText(/Cancellation requested/);
  rerender(<Upgrade status={fresh} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.queryByText(/Cancellation requested/)).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('Updating the workspace');
  expect(screen.getByRole('button', { name: 'Use saved connection' }).matches(':disabled')).toBe(true);
  expect(onBusy.mock.calls).toEqual([[true]]);
  await act(async () => { finish({ outcome: 'cancelled', status: { ...fresh, state: 'incomplete', canReplace: true } }); });
  rerender(<Upgrade status={{ ...fresh, state: 'incomplete', canReplace: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  await screen.findByText(/Update cancelled/);
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
});

test('forward final cancellation survives terminal and same-transaction observations', async () => {
  const { ipc, rerender, onChange, onBusy } = mount();
  ipc.mockReturnValue({ outcome: 'cancelled', status: { ...fresh, state: 'incomplete', canReplace: true } });
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await waitFor(() => expect(onChange).toHaveBeenCalled());
  rerender(<Upgrade status={{ ...fresh, state: 'incomplete', canReplace: true }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  rerender(<Upgrade status={fresh} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
});

test('forced stop distinguishes confirmed unclean termination from unconfirmed stop', () => {
  mount({ ...eligible, error: 'forcedStop' });
  expect(screen.getByRole('alert').textContent).toMatch(/forcibly stopped without a clean shutdown/);
  expect(screen.getByRole('alert').textContent).toMatch(/may be lost/);
  expect(screen.queryByText(/could not be stopped safely/)).toBeNull();
});

test('Check again clears only the local lost-command error', async () => {
  const { ipc, rerender, onChange, onBusy } = mount(fresh);
  ipc.mockRejectedValue('lost reply');
  fireEvent.click(screen.getByRole('button', { name: 'Use saved connection' }));
  await screen.findByRole('alert');
  const onCheck = vi.fn();
  rerender(<Upgrade status={{ ...fresh, state: 'sourceSelected', error: 'network' }} onChange={onChange} onBusy={onBusy} onCheck={onCheck} />);
  fireEvent.click(screen.getByRole('button', { name: 'Check again' }));
  expect(onCheck).toHaveBeenCalledOnce();
  expect(screen.getByRole('alert').textContent).toContain('could not be downloaded');
  rerender(<Upgrade status={{ ...fresh, state: 'sourceSelected' }} onChange={onChange} onBusy={onBusy} onCheck={onCheck} />);
  expect(screen.queryByRole('alert')).toBeNull();
});

test('quarantined native stop guidance supersedes an earlier generic command failure', async () => {
  const { ipc, rerender, onChange, onBusy } = mount(fresh);
  ipc.mockRejectedValue('private stop error');
  fireEvent.click(screen.getByRole('button', { name: 'Use saved connection' }));
  await screen.findByText('Workspace update information is unavailable. Check again before changing your workspace.');
  rerender(<Upgrade status={{ ...fresh, state: 'conflict', error: 'stopUnconfirmed', canRestore: false }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.getByRole('alert').textContent).toContain('Quit and reopen');
});

test('interrupted restore offers only exact restore continuation, never another replacement', async () => {
  const { ipc } = mount({ ...fresh, state: 'restoring', credential: 'absent' });
  expect(screen.getByRole('status').textContent).toContain('Restore was interrupted');
  expect(screen.queryByRole('button', { name: /Set up.*workspace/ })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Restore previous workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Restore this workspace' }));
  await waitFor(() => expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_restore', { transaction }));
});
test('restored transaction is recovery-only and fresh C is another confirmed setup', () => {
  const { ipc } = mount({ ...fresh, state: 'restored', repeated: true, canReplace: true, canRestore: false });
  expect(screen.queryByRole('button', { name: 'Restore previous workspace' })).toBeNull();
  expect(screen.getByText(/Quit and open Bridge 0.1.2/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Set up another new workspace' }));
  expect(screen.getByText(/workspace kept from your last update will not be reopened/)).toBeTruthy();
  expect(ipc).not.toHaveBeenCalled();
});
test.each(['network', 'noSpace', 'stopUnconfirmed', 'integrity'] as const)('finite %s failure stays announced with no automatic retry', error => {
  const { ipc } = mount({ ...eligible, error });
  expect(screen.getByRole('alert').textContent).toBeTruthy();
  expect(ipc).not.toHaveBeenCalled();
});
test.each([
  ['replace', eligible, { canReplace: false }, 'Set up new workspace', 'Keep old workspace and set up new'],
  ['retry', { ...fresh, state: 'incomplete', canReplace: true }, { canReplace: false }, 'Try setup again', 'Keep old workspace and set up new'],
  ['restore', fresh, { canRestore: false }, 'Restore previous workspace', 'Restore this workspace'],
  ['remove absent', fresh, { credential: 'absent' }, 'Remove saved connection', 'Remove for all retained workspaces'],
  ['remove removed', fresh, { credential: 'removed' }, 'Remove saved connection', 'Remove for all retained workspaces'],
] as const)('%s confirmation expires when its native capability is withdrawn', (_action, status, withdrawal, label, submitLabel) => {
  const { rerender, onChange, onBusy, ipc } = mount(status);
  fireEvent.click(screen.getByRole('button', { name: label }));
  const submit = screen.getByRole('button', { name: submitLabel });
  rerender(<Upgrade status={{ ...status, ...withdrawal }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.queryByRole('group', { name: /workspace\?|connection\?/ })).toBeNull();
  fireEvent.click(submit);
  expect(ipc).not.toHaveBeenCalled(); expect(onBusy).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(screen.getByRole('status'));
});
test('removal confirmation expires when credential use is withdrawn and returns focus to status', () => {
  const { rerender, onChange, onBusy, ipc } = mount(fresh);
  fireEvent.click(screen.getByRole('button', { name: 'Remove saved connection' }));
  const submit = screen.getByRole('button', { name: 'Remove for all retained workspaces' });
  rerender(<Upgrade status={fresh} canUseCredential={false} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.queryByRole('group', { name: 'Remove saved connection?' })).toBeNull();
  fireEvent.click(submit); expect(ipc).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(screen.getByRole('status'));
});
test.each([
  [{ ...fresh, state: 'incomplete', canReplace: true }, 'Try setup again'],
  [fresh, 'Restore previous workspace'],
  [fresh, 'Remove saved connection'],
] as const)('a same-transaction observation preserves a still-valid %s confirmation', (status, label) => {
  const { rerender, onChange, onBusy, ipc } = mount(status);
  fireEvent.click(screen.getByRole('button', { name: label }));
  const confirm = screen.getByRole('group', { name: /workspace\?|connection\?/ });
  const cancel = within(confirm).getByRole('button', { name: 'Not now' });
  rerender(<Upgrade status={{ ...status, additionalBytes: 3000000000 }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.getByRole('group', { name: /workspace\?|connection\?/ })).toBe(confirm); expect(document.activeElement).toBe(cancel);
  expect(ipc).not.toHaveBeenCalled();
});
test('forward cancellation belongs to its reply transaction, survives terminal polls, and does not mask another transaction', async () => {
  const { rerender, onChange, onBusy, ipc } = mount();
  const cancelled: UpgradeStatus = { ...fresh, state: 'incomplete', canReplace: true };
  ipc.mockReturnValue({ outcome: 'cancelled', status: cancelled });
  fireEvent.click(screen.getByRole('button', { name: 'Set up new workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep old workspace and set up new' }));
  await waitFor(() => expect(onChange).toHaveBeenCalledWith(cancelled));
  // The reply may arrive before the parent adopts its transaction.
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  const show = (status: UpgradeStatus) => rerender(<Upgrade status={status} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  show(cancelled); expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  show({ ...cancelled }); expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  show({ ...fresh }); expect(screen.getByText(/Update cancelled/)).toBeTruthy();
  show({ ...cancelled, transaction: '20000000-0000-4000-8000-000000000002' });
  expect(screen.queryByText(/Update cancelled/)).toBeNull();
  expect(screen.getByRole('status').textContent).toContain('Choose Try setup again');
  expect(ipc).toHaveBeenCalledExactlyOnceWith('workspace_upgrade_replace', {});
  expect(onBusy.mock.calls).toEqual([[true], [false]]);
});
test('a changed transaction dismisses a stale confirmation rather than restoring another pair', () => {
  const { rerender, onChange, onBusy, ipc } = mount(fresh);
  fireEvent.click(screen.getByRole('button', { name: 'Restore previous workspace' }));
  rerender(<Upgrade status={{ ...fresh, transaction: '20000000-0000-4000-8000-000000000002' }} onChange={onChange} onBusy={onBusy} onCheck={() => {}} />);
  expect(screen.queryByRole('button', { name: 'Restore this workspace' })).toBeNull();
  expect(ipc).not.toHaveBeenCalled();
});
