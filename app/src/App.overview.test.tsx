import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import App from './App';
import type { SendView } from './clusters/send-api';
import { openLocalSetup } from './setup/test-navigation';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const target = { id: '00000000-0000-4000-8000-000000000002', context_id: '00000000-0000-4000-8000-000000000001', context_name: 'team-one', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'fixture' };
const provider = { id: '00000000-0000-4000-8000-000000000003', name: 'team-model', ready: true };
const pair = { documentId, name: 'reader', source, agent: '---\nname: reader\nmodel: literal-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead a page.', tests: '---\nschemaVersion: 1\n---\n\n```yaml agent-test\nid: first\ninput: Read\nassertions:\n  - id: answer\n    type: contains\n    value: page\n```\n' };
const record = { id: documentId, source, targetDisplay: 'team-one · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'svc', deploymentNamespace: 'control', deploymentUid: 'dep', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'bridge-reader', agentUid: 'uid', verdict: 'passed', time: 123, documentId, detailState: 'saved', report: { verdict: 'passed', passed: 1, total: 1 } };
const calls: { command: string; args: unknown }[] = [];
const inventory = ['one', 'two'].map(key => ({ key, context: `team-${key}`, service: 'control/orka', namespace: 'agents', stale: false, observedAt: 123, agents: [{ name: 'bridge-reader', namespace: 'agents', uid: 'uid', agentType: null, model: null, tools: null, record }] }));
function ipc(select: (args: unknown) => unknown = () => ({ target, providers: [provider] }), extra: (command: string, args: unknown) => unknown = () => undefined) {
  mockIPC((command, args) => {
    calls.push({ command, args });
    const other = extra(command, args); if (other !== undefined) return other;
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
async function expand(name = 'reader') {
  const agent = await screen.findByRole('article', { name });
  const toggle = within(agent).getByRole('button', { name: `Show places for ${name}` });
  if (toggle.getAttribute('aria-expanded') === 'false') await userEvent.click(toggle);
  return agent;
}

test.each([true, false])('Overview follows acknowledged local AI connection, not a ready inventory (%s)', async succeeds => {
  let finish!: (value: unknown) => void;
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'setup_connect_model') return new Promise(resolve => { finish = resolve; });
    return undefined;
  });
  const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  const localRun = screen.getByRole('button', { name: 'Run tests reader' }); expect(localRun).toHaveProperty('disabled', true);
  expect(screen.getByRole('button', { name: 'Run tests reader on team-one' })).toHaveProperty('disabled', false);
  await user.click(screen.getByRole('button', { name: 'Clusters' })); await openLocalSetup(user); await user.click(await screen.findByRole('button', { name: 'Connect the AI' }));
  await waitFor(() => expect(finish).toBeTypeOf('function')); await user.click(screen.getByRole('button', { name: 'Overview' }));
  expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', true);
  await act(async () => finish(succeeds ? { ready: true } : { ready: false }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', !succeeds)); expect(actions()).toEqual([]);
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
  if (connectedInSetup) { await openLocalSetup(user); await user.click(await screen.findByRole('button', { name: 'Connect the AI' })); }
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Open reader' })); await screen.findByRole('heading', { name: 'reader', level: 1 });
  await waitFor(() => expect(calls.some(c => c.command === 'agent_run_status')).toBe(true)); await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', !nativeConnected)); expect(actions()).toEqual([]);
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
  const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', false));
  expect(calls.some(c => c.command === 'agent_open')).toBe(false);
  connected = false; await user.click(screen.getByRole('button', { name: 'Open reader' })); await screen.findByRole('heading', { name: 'reader', level: 1 }); await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', true));
  await user.click(screen.getByRole('button', { name: 'Clusters' })); await openLocalSetup(user); await user.click(await screen.findByRole('button', { name: 'Connect the AI' })); await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', false)); expect(actions()).toEqual([]);
});

test('Overview recovers native readiness on activation and observes disconnects on the same reread path', async () => {
  let readable = false, connected = true, reads = 0;
  ipc(undefined, command => {
    if (command === 'setup_status') return { selected: 'localVm', localVm: 'ready', model: 'available' };
    if (command === 'overview_agents') return { state: 'complete', installation: documentId, agents: [], observation: null, observedAt: 123, runActive: false, reason: null };
    if (command === 'agent_run_status') { reads++; if (!readable) return Promise.reject(Error('temporarily unavailable')); return { documentId, busy: null, state: 'idle', canRun: connected, modelConnected: connected, report: null, acknowledged: [], pending: null, error: null }; }
    return undefined;
  });
  const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand(); await waitFor(() => expect(reads).toBeGreaterThan(0));
  expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', true);
  readable = true; await user.click(screen.getByRole('button', { name: 'Clusters' })); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', false));
  connected = false; await user.click(screen.getByRole('button', { name: 'Clusters' })); await user.click(screen.getByRole('button', { name: 'Overview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Run tests reader' })).toHaveProperty('disabled', true)); expect(actions()).toEqual([]);
});

test('local readiness never substitutes a provider for a team destination', async () => {
  ipc(() => ({ target, providers: [{ ...provider, ready: false }] })); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand(); await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 }); expect(actions()).toEqual([]); expect(calls.filter(c => c.command === 'send_preflight')).toEqual([]);
});

test('cluster Run selects the exact destination and never executes from Overview', async () => {
  ipc(); const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' })); await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(calls).toContainEqual({ command: 'cluster_select', args: { key: 'one' } }); expect(document.querySelector('.host-chip')?.textContent).toContain('team-one');
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' }); await waitFor(() => expect(document.activeElement).toBe(panel)); expect(actions()).toEqual([]);
});

test('a hidden Send completion revalidates Overview after its first read missed the Running journal checkpoint', async () => {
  const sendId = '00000000-0000-4000-8000-000000000004';
  const running = { documentId, id: sendId, targetId: target.id, providerId: provider.id, targetName: target.context_name, source, model: 'literal-model', agentName: 'bridge-reader', state: 'running', stage: 'observe', acknowledged: [{ kind: 'Agent', name: 'bridge-reader', uid: 'uid' }], pending: null, report: null, error: null, storageError: false };
  let view: unknown = null;
  let finished = false;
  ipc(undefined, command => {
    if (command === 'send_start') { view = running; return { documentId, view, busy: null }; }
    if (command === 'send_status') return { documentId, view, busy: null };
    if (command === 'overview_places') return { historyState: 'complete', unobserved: [], places: inventory.map(place => place.key !== 'one' || !finished ? place : { ...place, agents: [
      { ...place.agents[0], record: { ...record, id: sendId, verdict: 'failed', report: { verdict: 'failed', passed: 0, total: 1 } } },
      { ...place.agents[0], uid: 'other-uid', record: { ...record, agentUid: 'other-uid', source: { ...source, tests: 'c'.repeat(64) } } },
    ] }) };
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' }));
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  const start = within(panel).getByRole('button', { name: 'Send and run tests' });
  await waitFor(() => expect(start).toHaveProperty('disabled', false)); await user.click(start);
  await screen.findByRole('button', { name: 'Cancel tests' });
  // Admission has returned Running, but persisted history still contains the previous Passed result.
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const agent = await expand();
  expect(within(agent).getByRole('article', { name: 'team-one, Passed' })).toBeTruthy();
  expect(screen.queryByRole('region', { name: 'Send and cluster tests' })).toBeNull();
  const reads = calls.filter(c => c.command === 'overview_places').length;
  finished = true;
  view = { ...running, state: 'complete', stage: 'done', report: { verdict: 'failed', cases: [{ id: 'first', question: 'Read', phase: 'Succeeded', answer: 'wrong', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'page', verdict: 'failed' }], verdict: 'failed' }] } };
  await waitFor(() => expect(within(agent).getByRole('article', { name: 'team-one, Failed' })).toBeTruthy(), { timeout: 1500 });
  expect(within(agent).getByRole('article', { name: 'team-two, Passed' })).toBeTruthy();
  const failed = within(agent).getByRole('article', { name: 'team-one, Failed' });
  expect(failed.getAttribute('data-card-key')).toBe('one/uid');
  expect(within(failed).getByText('0 of 1 passed')).toBeTruthy();
  await user.click(within(failed).getByRole('button', { name: 'Details' }));
  expect(within(failed).getByText(source.agent)).toBeTruthy();
  expect(within(failed).getByText(source.tests)).toBeTruthy();
  const other = within(agent).getByRole('article', { name: 'team-one, Not run since your edit' });
  expect(other.getAttribute('data-card-key')).toBe('one/other-uid');
  expect(other.textContent).not.toContain('Failed');
  // Repeated terminal status polls must not invalidate inventory again or restart live/file reads.
  const statusReads = calls.filter(c => c.command === 'send_status').length;
  const fileReads = calls.filter(c => c.command === 'overview_files').length;
  const agentReads = calls.filter(c => c.command === 'overview_agents').length;
  await waitFor(() => expect(calls.filter(c => c.command === 'send_status').length).toBeGreaterThan(statusReads + 1), { timeout: 1500 });
  expect(calls.filter(c => c.command === 'overview_places')).toHaveLength(reads + 1);
  expect(calls.filter(c => c.command === 'overview_files')).toHaveLength(fileReads);
  expect(calls.filter(c => c.command === 'overview_agents')).toHaveLength(agentReads);
  expect(calls.filter(c => c.command === 'send_start')).toHaveLength(1);
  expect(calls.some(c => c.command === 'send_agent_focus')).toBe(false);
});

test.each(['loading', 'error'] as const)('a Send completion survives a real document switch while the replacement stays %s', async replacementState => {
  const replacementId = '00000000-0000-4000-8000-000000000005';
  const sendId = '00000000-0000-4000-8000-000000000006';
  const running: SendView = { documentId, id: sendId, targetId: target.id, providerId: provider.id, targetName: null, source: null, model: null, agentName: null, state: 'running', stage: 'capture', acknowledged: [], pending: null, report: null, error: null, storageError: false };
  let rejectOpen!: (reason: Error) => void;
  const replacementOpen = new Promise<never>((_, reject) => { rejectOpen = reject; });
  let selected = documentId;
  let view: SendView | null = null;
  let finished = false;
  ipc(undefined, (command, args) => {
    const request = args as { documentId: string };
    if (command === 'agents_list') return [{ id: documentId, name: 'reader', path: null, available: true }, { id: replacementId, name: 'helper', path: null, available: true }];
    if (command === 'agent_selected') return selected;
    if (command === 'agent_select') { selected = request.documentId; return null; }
    if (command === 'agent_open' && request.documentId === replacementId) return replacementOpen;
    if (command === 'agent_run_status') return { documentId: request.documentId, busy: null, state: 'idle', canRun: false, report: null, acknowledged: [], pending: null, error: null };
    if (command === 'send_start') { view = running; return { documentId, view, busy: { documentId, name: 'reader' } }; }
    if (command === 'send_status') return { documentId: request.documentId, view: request.documentId === documentId ? view : null, busy: view?.state === 'running' ? { documentId, name: 'reader' } : null };
    if (command === 'overview_places') return { historyState: 'complete', unobserved: [], places: inventory.map(place => place.key !== 'one' || !finished ? place : { ...place, agents: [
      { ...place.agents[0], record: { ...record, id: sendId, verdict: 'failed', report: { verdict: 'failed', passed: 0, total: 1 } } },
      { ...place.agents[0], uid: 'other-uid', record: { ...record, agentUid: 'other-uid', source: { ...source, tests: 'c'.repeat(64) } } },
    ] }) };
    return undefined;
  });
  const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' }));
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  const start = within(panel).getByRole('button', { name: 'Send and run tests' });
  await waitFor(() => expect(start).toHaveProperty('disabled', false)); await user.click(start);
  await screen.findByRole('button', { name: 'Cancel tests' });
  // Native has admitted A, but has not written a Running history checkpoint yet.
  const statusReads = calls.filter(c => c.command === 'send_status').length;
  await waitFor(() => expect(calls.filter(c => c.command === 'send_status').length).toBeGreaterThan(statusReads));
  const selector = screen.getByRole('combobox', { name: 'Agent' });
  await user.selectOptions(selector, replacementId);
  let replacement = await screen.findByRole('status', { name: 'Opening your agent' });
  expect(selector).toHaveProperty('value', replacementId);
  expect(selected).toBe(replacementId);
  expect(calls).toContainEqual({ command: 'agent_open', args: { documentId: replacementId } });
  expect(document.querySelector('.send-panel')).toBeNull();
  expect(screen.queryByRole('textbox', { name: 'Name' })).toBeNull();
  if (replacementState === 'error') {
    await act(async () => rejectOpen(Error('replacement folder unavailable')));
    replacement = await screen.findByRole('alert');
    expect(replacement.textContent).toBe('Your agent folder could not be opened. Open it again from Designer.');
  }
  await user.click(screen.getByRole('button', { name: 'Overview' }));
  const agent = await expand();
  expect(within(agent).getByRole('article', { name: 'team-one, Passed' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Refresh' })).toBeNull();
  const reads = calls.filter(c => c.command === 'overview_places').length;
  const fileReads = calls.filter(c => c.command === 'overview_files').length;
  const agentReads = calls.filter(c => c.command === 'overview_agents').length;
  const focused = document.activeElement;
  await act(async () => {
    finished = true;
    view = { ...running, targetName: target.context_name, source, model: 'literal-model', agentName: 'bridge-reader', state: 'complete', stage: 'done', acknowledged: [{ kind: 'Agent', name: 'bridge-reader', uid: 'uid' }], report: { verdict: 'failed', cases: [{ id: 'first', question: 'Read', phase: 'Succeeded', answer: 'wrong', completedTools: [], assertions: [{ id: 'answer', kind: 'contains', expected: 'page', verdict: 'failed' }], verdict: 'failed' }] } };
  });
  expect(calls.filter(c => c.command === 'send_start')).toHaveLength(1);
  expect(selected).toBe(replacementId);
  expect(replacement.isConnected).toBe(true);
  expect(document.querySelector('.send-panel')).toBeNull();
  const failed = await within(agent).findByRole('article', { name: 'team-one, Failed' });
  expect(document.activeElement).toBe(focused);
  expect(calls.filter(c => c.command === 'overview_files')).toHaveLength(fileReads);
  expect(calls.filter(c => c.command === 'overview_agents')).toHaveLength(agentReads);
  expect(calls).toContainEqual({ command: 'send_start', args: { documentId, targetId: target.id, providerId: provider.id, expectedSource: source } });
  expect(failed.getAttribute('data-card-key')).toBe('one/uid');
  expect(within(failed).getByText('0 of 1 passed')).toBeTruthy();
  await user.click(within(failed).getByRole('button', { name: 'Details' }));
  expect(within(failed).getByText(source.agent)).toBeTruthy();
  expect(within(failed).getByText(source.tests)).toBeTruthy();
  expect(within(agent).getByRole('article', { name: 'team-two, Passed' })).toBeTruthy();
  const other = within(agent).getByRole('article', { name: 'team-one, Not run since your edit' });
  expect(other.getAttribute('data-card-key')).toBe('one/other-uid');
  expect(other.textContent).not.toContain('Failed');
  expect(calls.filter(c => c.command === 'overview_places').length).toBeGreaterThan(reads);
  expect(calls.filter(c => c.command === 'send_start')).toHaveLength(1);
  expect(selector).toHaveProperty('value', replacementId);
  expect(replacement.isConnected).toBe(true);
  expect(document.querySelector('.send-panel')).toBeNull();
});

test('opening another cluster resets an old management subview before returning to Clusters', async () => {
  let selectedKey = 'one'; const second = { ...target, id: '00000000-0000-4000-8000-000000000004', context_name: 'team-two' };
  ipc(args => { selectedKey = (args as { key: string }).key; return { target: selectedKey === 'one' ? target : second, providers: [provider] }; }, command => command === 'cluster_places' ? { places: inventory, selectedKey, persistenceState: 'complete' } : command === 'setup_status' ? { selected: 'localVm', localVm: 'ready', model: 'locked' } : undefined);
  const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' })); await screen.findByRole('heading', { name: 'reader', level: 1 });
  await user.click(screen.getByRole('button', { name: 'Clusters' })); await user.click(await within(await screen.findByRole('region', { name: 'team-one' })).findByRole('button', { name: 'Manage' }));
  await screen.findByRole('heading', { name: 'team-one', level: 1 }); await user.click(screen.getByRole('button', { name: 'Overview' }));
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-two' })); await screen.findByRole('heading', { name: 'reader', level: 1 }); await user.click(screen.getByRole('button', { name: 'Clusters' }));
  expect(await screen.findByRole('heading', { name: 'Clusters', level: 1 })).toBeTruthy(); expect(await within(screen.getByRole('region', { name: 'team-two' })).findByRole('button', { name: 'Manage' })).toBeTruthy(); expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull();
});

test('lost access opens the file with no admitted destination and reconnect guidance', async () => {
  ipc(() => Promise.reject(Error('gone'))); const user = userEvent.setup(); render(<App />); await user.click(screen.getByRole('button', { name: 'Overview' })); await expand();
  await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' })); await screen.findByRole('heading', { name: 'reader', level: 1 });
  expect(await screen.findByText(/Connect this cluster again in Clusters/)).toBeTruthy(); expect(document.querySelector('.host-chip')?.textContent).toContain('Choose a cluster'); expect(actions()).toEqual([]);
});

test('leaving Overview invalidates a delayed cluster selection reply', async () => {
  let finish!: (v: unknown) => void; ipc(() => new Promise(resolve => { finish = resolve; })); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand(); await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' })); await waitFor(() => expect(finish).toBeTypeOf('function'));
  await user.click(screen.getByRole('button', { name: 'Clusters' })); await act(async () => finish({ target, providers: [provider] }));
  expect(screen.queryByRole('heading', { name: 'reader', level: 1 })).toBeNull(); expect(document.querySelector('.host-chip')?.textContent).not.toContain('team-one'); expect(actions()).toEqual([]);
});

test('a newer cluster card wins over an older selection reply', async () => {
  let finish!: (v: unknown) => void; let selections = 0; const second = { ...target, id: '00000000-0000-4000-8000-000000000004', context_name: 'team-two' };
  ipc(() => ++selections === 1 ? new Promise(resolve => { finish = resolve; }) : { target: second, providers: [provider] }); const user = userEvent.setup(); render(<App />);
  await user.click(screen.getByRole('button', { name: 'Overview' })); await expand(); await user.click(screen.getByRole('button', { name: 'Run tests reader on team-one' })); await user.click(screen.getByRole('button', { name: 'Run tests reader on team-two' }));
  await screen.findByRole('heading', { name: 'reader', level: 1 }); await act(async () => finish({ target, providers: [provider] })); expect(document.querySelector('.host-chip')?.textContent).toContain('team-two'); expect(actions()).toEqual([]);
});
