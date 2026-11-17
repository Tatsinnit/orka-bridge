import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from './App';
import { openLocalSetup } from './setup/test-navigation';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-one', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'fixture' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const pair = { documentId, name: 'reader', source, agent: '---\nname: reader\nmodel: literal-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead a page.', tests: '---\nschemaVersion: 1\n---\n\n```yaml agent-test\nid: first\ninput: Read\nassertions:\n  - id: answer\n    type: contains\n    value: page\n```\n' };
const record = { id: documentId, source, targetDisplay: 'team-one · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'svc', deploymentNamespace: 'control', deploymentUid: 'dep', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'bridge-reader', agentUid: 'uid', verdict: 'passed', time: 123, documentId, detailState: 'saved', report: { verdict: 'passed', passed: 1, total: 1 } };
const calls: { command: string; args: unknown }[] = [];
const inventory = ['one', 'two'].map(key => ({ key, context: `team-${key}`, service: 'control/orka', namespace: 'agents', stale: false, observedAt: 123, agents: [{ name: 'bridge-reader', namespace: 'agents', uid: 'uid', agentType: null, model: null, tools: null, record }] }));
function ipc(select: (args: unknown) => unknown = () => ({ target, providers: [provider] }), extra: (command: string) => unknown = () => undefined) {
  mockIPC((command, args) => {
    calls.push({ command, args });
    const other = extra(command); if (other !== undefined) return other;
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'closed', model: 'locked' };
    if (command === 'overview_files') return { state: 'complete', runActive: false, historyState: 'complete', runAgents: [], files: [{ documentId, name: 'reader', state: 'readable', source, summary: null, reason: null, localResult: null }] };
    if (command === 'overview_places') return { historyState: 'complete', unobserved: [], places: inventory };
    if (command === 'overview_agents') return { state: 'unavailable', installation: null, agents: [], observation: null, observedAt: null, runActive: false, reason: 'stopped' };
    if (command === 'cluster_select') return select(args);
    if (command === 'agent_open') return pair;
    if (command === 'agent_selected') return documentId;
    if (command === 'agents_list') return [{ documentId, name: 'reader' }];
    if (command === 'agent_select') return null;
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_status') return { documentId, view: null, busy: null };
    if (command === 'send_history') return [];
    if (command === 'workspace_catalog' || command === 'cluster_catalog') return { defaultModel: 'literal-model', models: { state: 'ready', items: [{ id: 'literal-model' }] }, tools: { state: 'ready', items: [] } };
    if (command === 'send_preflight') return { documentId, targetId: target.id, providerId: provider.id, source, model: 'literal-model', rows: [{ label: 'Destination', status: 'allowed', message: 'Ready' }], canSend: true };
    return null;
  });
}
beforeEach(() => { vi.stubGlobal('isTauri', true); localStorage.setItem('bridge.welcomeSeen.v1', '1'); calls.length = 0; });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); localStorage.clear(); });
const actions = () => calls.map(c => c.command).filter(c => ['agent_run', 'send_start', 'cluster_use', 'cluster_reconnect'].includes(c));

test.each([true, false])('Overview follows acknowledged local AI connection, not a ready inventory (%s)', async succeeds => {
  let finish!: (value: unknown) => void;
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'setup_connect_model') return new Promise(resolve => { finish = resolve; });
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const localRun = await screen.findByRole('button', { name: 'Run tests reader' });
  expect(localRun.matches(':disabled')).toBe(true);
  expect(screen.getByRole('button', { name: 'Run tests reader on team-one' }).matches(':disabled')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true);
  await act(async () => finish(succeeds ? { ready: true } : { ready: false }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(!succeeds));
  expect(screen.getByRole('button', { name: 'Chat reader' }).matches(':disabled')).toBe(!succeeds);
  expect(actions()).toEqual([]);
});

test.each([{ connectedInSetup: false, nativeConnected: true }, { connectedInSetup: true, nativeConnected: false }])('Overview honors native connection evidence over setup state: %j', async ({ connectedInSetup, nativeConnected }) => {
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') return { ready: true };
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'idle', canRun: nativeConnected, modelConnected: nativeConnected, report: null, acknowledged: [], pending: null, error: null };
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  if (connectedInSetup) {
    await openLocalSetup(user);
    await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  }
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await waitFor(() => expect(calls.some(c => c.command === 'agent_run_status')).toBe(true));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(!nativeConnected));
  expect(screen.getByRole('button', { name: 'Chat reader' }).matches(':disabled')).toBe(!nativeConnected);
  expect(actions()).toEqual([]);
});

test('Overview refreshes native AI readiness without a mounted Designer and after reconnecting', async () => {
  let connected = true;
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'setup_connect_model') { connected = true; return { ready: true }; }
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'idle', canRun: connected, modelConnected: connected, report: null, acknowledged: [], pending: null, error: null };
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(false));
  expect(calls.some(c => c.command === 'agent_open')).toBe(false);
  connected = false;
  await user.click(screen.getByRole('button', { name: 'Open reader' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await openLocalSetup(user);
  await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(false));
  expect(actions()).toEqual([]);
});

test('Overview recovers native readiness and observes disconnects without navigating away', async () => {
  let readable = false; let connected = true; let reads = 0;
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'agent_run_status') {
      reads++;
      if (!readable) return Promise.reject(Error('temporarily unavailable'));
      return { documentId, busy: null, state: 'idle', canRun: connected, modelConnected: connected, report: null, acknowledged: [], pending: null, error: null };
    }
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(reads).toBeGreaterThan(0));
  expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true);
  readable = true;
  await user.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(false));
  connected = false;
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true));
  expect(calls.some(c => c.command === 'agent_open')).toBe(false);
  expect(actions()).toEqual([]);
});

test('local readiness never substitutes a provider for a team destination', async () => {
  ipc(() => ({ target, providers: [{ ...provider, ready: false }] }));
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Run tests reader on team-one' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(actions()).toEqual([]);
  expect(calls.filter(c => c.command === 'send_preflight')).toEqual([]);
});

test.each(['Open', 'Run tests'])('cluster %s selects the exact destination and never executes from Overview', async action => {
  ipc(); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: `${action} reader on team-one` }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(calls).toContainEqual({ command: 'cluster_select', args: { key: 'one' } });
  expect(document.querySelector('.host-chip')?.textContent).toContain('team-one');
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  if (action === 'Run tests') await waitFor(() => expect(document.activeElement).toBe(panel));
  expect(actions()).toEqual([]);
  expect(within(panel).queryByText(/Choose a cluster in Clusters/)).toBeNull();
});

test('opening another cluster resets an old management subview before returning to Clusters', async () => {
  let selectedKey = 'one';
  const second = { ...target, id: '00000000-0000-4000-8000-000000000004', context_name: 'team-two' };
  ipc(args => {
    selectedKey = (args as { key: string }).key;
    return { target: selectedKey === 'one' ? target : second, providers: [provider] };
  }, command => command === 'cluster_places' ? { places: inventory, selectedKey, persistenceState: 'complete' }
    : command === 'setup_status' ? { selected: 'localVm', localVm: 'ready', model: 'locked' } : undefined);
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader on team-one' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await user.click(await within(await screen.findByRole('region', { name: 'team-one' })).findByRole('button', { name: 'Manage' }));
  await screen.findByRole('heading', { name: 'Manage your cluster', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Run tests reader on team-two' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByRole('heading', { name: 'Clusters', level: 1 })).toBeTruthy();
  expect(await within(screen.getByRole('region', { name: 'team-two' })).findByRole('button', { name: 'Manage' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull();
});

test('lost access opens the file with no admitted destination and reconnect guidance', async () => {
  ipc(() => Promise.reject(Error('gone'))); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Run tests reader on team-one' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(await screen.findByText(/Connect this cluster again in Clusters/)).toBeTruthy();
  expect(document.querySelector('.host-chip')?.textContent).toContain('Choose a cluster');
  expect(actions()).toEqual([]);
});

test('leaving Overview invalidates a delayed cluster selection reply', async () => {
  let finish!: (v: unknown) => void;
  ipc(() => new Promise(resolve => { finish = resolve; })); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Run tests reader on team-one' }));
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  await user.click(screen.getByRole('button', { name: 'Clusters' }));
  await act(async () => finish({ target, providers: [provider] }));
  expect(screen.queryByRole('heading', { name: 'reader', level: 1 })).toBeNull();
  expect(document.querySelector('.host-chip')?.textContent).not.toContain('team-one');
  expect(actions()).toEqual([]);
});

test('a newer cluster card wins over an older selection reply', async () => {
  let finish!: (v: unknown) => void; let selections = 0;
  const second = { ...target, id: '00000000-0000-4000-8000-000000000004', context_name: 'team-two' };
  ipc(() => ++selections === 1 ? new Promise(resolve => { finish = resolve; }) : { target: second, providers: [provider] });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(await screen.findByRole('button', { name: 'Open reader on team-one' }));
  await user.click(screen.getByRole('button', { name: 'Open reader on team-two' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 });
  await act(async () => finish({ target, providers: [provider] }));
  expect(document.querySelector('.host-chip')?.textContent).toContain('team-two');
  expect(actions()).toEqual([]);
});
