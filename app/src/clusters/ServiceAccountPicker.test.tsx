import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import ClusterCard from './ClusterCard';
import ServiceAccountPicker from './ServiceAccountPicker';
import { chooseContext } from './test-context';

const context = { id: '00000000-0000-4000-8000-000000000001', name: 'team-dev', current: false };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: context.id, context_name: context.name, service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'image:v1' };
const checks = [
  ['agents', 'create'], ['agents', 'get'], ['agents', 'list'], ['tasks', 'create'], ['tasks', 'get'], ['tasks', 'list'],
  ['providers', 'get'], ['providers', 'list'], ['tools', 'get'], ['tools', 'list'],
].map(([resource, verb]) => ({ resource, verb, status: 'allowed' }));
function fixture(list: unknown = { names: ['default', 'team-runner', 'task-reader'], truncated: false }, preview: unknown = { status: 'allowed', checks }) {
  vi.stubGlobal('isTauri', true);
  const ipc = vi.fn((command: string) => {
    if (command === 'cluster_contexts' || command === 'cluster_reload') return [context];
    if (command === 'cluster_discover') return [target];
    if (command === 'cluster_use') return { status: 'needServiceAccount', providers: [], failure: null };
    if (command === 'cluster_service_accounts') return typeof list === 'function' ? list() : list;
    if (command === 'cluster_service_account_preview') return typeof preview === 'function' ? preview() : preview;
    throw Error('Unsupported');
  });
  mockIPC(ipc); return ipc;
}
async function open() {
  const user = userEvent.setup(); const view = render(<ClusterCard onPlace={vi.fn()} onOpenAgent={vi.fn()} />);
  await chooseContext(user, 'team-dev');
  await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  return { user, view };
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });

test('choosing a listed account previews only it, and only explicit Use sends its name', async () => {
  const ipc = fixture(); const { user } = await open();
  const use = screen.getByRole('button', { name: 'Use this cluster' });
  expect(use.matches(':disabled')).toBe(true);
  await user.click(await screen.findByRole('button', { name: 'ServiceAccount' }));
  expect(screen.getAllByText('Not checked').length).toBe(3);
  for (const name of ['default', 'team-runner', 'task-reader']) {
    const description = screen.getByRole('button', { name }).getAttribute('aria-describedby');
    expect(description && document.getElementById(description)?.textContent).toBe('Not checked');
  }
  await user.type(screen.getByRole('searchbox', { name: 'Find an account' }), 'runner');
  expect(screen.queryByRole('button', { name: 'default' })).toBeNull();
  await user.click(screen.getByRole('button', { name: 'team-runner' }));
  await screen.findByText('Required Orka permissions: Allowed');
  const description = screen.getByRole('button', { name: 'ServiceAccount' }).getAttribute('aria-describedby');
  expect(description && document.getElementById(description)?.textContent).toBe('team-runner');
  expect(screen.getByText('Name/group preview only. Actual token permissions are checked on Use. Connection and broader access not verified.')).toBeTruthy();
  expect(use.matches(':disabled')).toBe(false);
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_service_account_preview')).toEqual([['cluster_service_account_preview', { targetId: target.id, saName: 'team-runner' }]]);
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use')).toHaveLength(1);
  await user.click(use);
  await waitFor(() => expect(ipc.mock.calls.filter(([c]) => c === 'cluster_use')).toEqual([
    ['cluster_use', { targetId: target.id, saName: null }], ['cluster_use', { targetId: target.id, saName: 'team-runner' }],
  ]));
});

test('account-list authentication rejection reuses the existing sign-in recovery path', async () => {
  fixture(() => Promise.reject({ check: 'serviceAccount', result: 'authenticationRejected' }));
  await open();
  await screen.findByText(/Refresh the sign-in outside Bridge, then press Reload kubeconfig/);
});

test('a delayed list does not take focus from another control', async () => {
  let finish!: (value: unknown) => void; fixture(() => new Promise(resolve => { finish = resolve; }));
  const user = userEvent.setup();
  render(<><ServiceAccountPicker target={target} active disabled={false} value="" onChange={vi.fn()} /><input aria-label="Another control" /></>);
  const other = screen.getByRole('textbox', { name: 'Another control' }); await user.click(other);
  await act(async () => finish({ names: ['team-runner'], truncated: false }));
  await screen.findByRole('button', { name: 'ServiceAccount' });
  expect(document.activeElement).toBe(other);
});

test('disabled account controls do not start a list until enabled', async () => {
  const ipc = fixture();
  const view = render(<ServiceAccountPicker target={target} active disabled value="" onChange={vi.fn()} />);
  expect(ipc.mock.calls.some(([c]) => c === 'cluster_service_accounts')).toBe(false);
  view.rerender(<ServiceAccountPicker target={target} active disabled={false} value="" onChange={vi.fn()} />);
  await screen.findByRole('button', { name: 'ServiceAccount' });
  expect(ipc.mock.calls.filter(([c]) => c === 'cluster_service_accounts')).toHaveLength(1);
});

test.each(['denied', 'unknown'])('advisory %s does not prevent explicit Use', async status => {
  fixture(undefined, { status, checks: checks.map((c, i) => ({ ...c, status: i === 0 ? status : 'notChecked' })) });
  const { user } = await open();
  await user.click(await screen.findByRole('button', { name: 'ServiceAccount' }));
  await user.click(screen.getByRole('button', { name: 'team-runner' }));
  await screen.findByText(`Required Orka permissions: ${status === 'denied' ? 'Denied' : "Couldn't check"}`);
  expect(screen.getByRole('button', { name: 'Use this cluster' }).matches(':disabled')).toBe(false);
});

test('forbidden listing gives a plain reason and typed fallback without preview on keystrokes', async () => {
  const ipc = fixture(() => Promise.reject({ check: 'serviceAccount', result: 'permissionDenied' }));
  const { user } = await open();
  await screen.findByText(/current cluster sign-in cannot list ServiceAccounts in agents/);
  await user.type(screen.getByRole('textbox', { name: 'ServiceAccount name' }), 'existing-runner');
  expect(ipc.mock.calls.some(([c]) => c === 'cluster_service_account_preview')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await waitFor(() => expect(ipc.mock.calls).toContainEqual(['cluster_use', { targetId: target.id, saName: 'existing-runner' }]));
});

test.each(['clusterApi', 'compatibility'])('a %s failure is not blamed on account-list rights', async check => {
  fixture(() => Promise.reject({ check, result: 'permissionDenied' }));
  await open();
  await screen.findByText("Couldn't recheck this cluster. Check Clusters and try again.");
  expect(screen.queryByText(/cannot list ServiceAccounts/)).toBeNull();
});

test('an empty capped list is incomplete and offers exactly one manual input', async () => {
  fixture({ names: [], truncated: true }); await open();
  await screen.findByText('The account list is incomplete. Enter an existing ServiceAccount name.');
  expect(screen.getAllByRole('textbox', { name: 'ServiceAccount name' })).toHaveLength(1);
  expect(screen.queryByText('No ServiceAccounts were found in agents.')).toBeNull();
});

test('empty listing is not denied and truncated results do not claim completeness', async () => {
  fixture({ names: [], truncated: false }); const { user } = await open();
  await screen.findByText('No ServiceAccounts were found in agents.');
  expect(screen.queryByText(/cannot list/)).toBeNull();
  expect(screen.getByRole('textbox', { name: 'ServiceAccount name' })).toBeTruthy();
  cleanup(); const truncatedIpc = fixture({ names: ['team-runner'], truncated: true });
  render(<ClusterCard onPlace={vi.fn()} onOpenAgent={vi.fn()} />);
  await chooseContext(user, 'team-dev'); await user.click(await screen.findByRole('button', { name: 'Use this cluster' }));
  await screen.findByText('Showing up to 1,000 accounts. More may exist.');
  expect(screen.getByRole('button', { name: 'ServiceAccount' })).toBeTruthy();
  await user.type(screen.getByRole('textbox', { name: 'ServiceAccount name' }), 'omitted-runner');
  expect(truncatedIpc.mock.calls.some(([c]) => c === 'cluster_service_account_preview')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await waitFor(() => expect(truncatedIpc.mock.calls).toContainEqual(['cluster_use', { targetId: target.id, saName: 'omitted-runner' }]));
});

test('a late permission result cannot replace the newly selected account result', async () => {
  let finish!: (value: unknown) => void; const first = new Promise(resolve => { finish = resolve; }); let calls = 0;
  const ipc = fixture(undefined, () => ++calls === 1 ? first : { status: 'allowed', checks }); const { user } = await open();
  await user.click(await screen.findByRole('button', { name: 'ServiceAccount' }));
  await user.click(screen.getByRole('button', { name: 'team-runner' }));
  await screen.findByText('Required Orka permissions: Checking');
  await user.click(screen.getByRole('button', { name: 'ServiceAccount' }));
  await user.click(screen.getByRole('button', { name: 'task-reader' }));
  await screen.findByText('Required Orka permissions: Allowed');
  await act(async () => finish({ status: 'denied', checks: checks.map((c, i) => ({ ...c, status: i ? 'notChecked' : 'denied' })) }));
  expect(screen.queryByText('Required Orka permissions: Denied')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Use this cluster' }));
  await waitFor(() => expect(ipc.mock.calls).toContainEqual(['cluster_use', { targetId: target.id, saName: 'task-reader' }]));
});

test('closing restores focus, and reloading while a preview waits discards the old result', async () => {
  let finish!: (value: unknown) => void; const pending = new Promise(resolve => { finish = resolve; });
  const ipc = fixture(undefined, pending); const { user } = await open();
  const trigger = await screen.findByRole('button', { name: 'ServiceAccount' });
  await user.click(trigger); await user.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Choose a ServiceAccount' })).toBeNull();
  expect(document.activeElement).toBe(trigger);
  expect(ipc.mock.calls.some(([c]) => c === 'cluster_service_account_preview')).toBe(false);
  await user.click(trigger); await user.click(screen.getByRole('button', { name: 'team-runner' }));
  await screen.findByText('Required Orka permissions: Checking');
  await user.click(screen.getByRole('button', { name: 'Reload kubeconfig' }));
  await act(async () => finish({ status: 'allowed', checks }));
  expect(screen.queryByText('Required Orka permissions: Allowed')).toBeNull();
  expect(screen.queryByRole('button', { name: 'ServiceAccount' })).toBeNull();
});
