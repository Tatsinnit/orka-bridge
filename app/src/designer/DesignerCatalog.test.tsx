import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Example from './Example';
import type { SelectedPlace } from '../clusters/api';

const documentId = '9b2b93c4-375c-4815-9717-39852a96e411';
const pair = { documentId, name: 'reader', agent: '---\nname: reader\nmodel: saved-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools: []\n---\nRead.', tests: 'Checks' };
const catalog = (model: string) => ({ defaultModel: model, models: { state: 'ready', items: [{ id: model }] }, tools: { state: 'ready', items: [{ kind: 'builtin', name: 'web_fetch', description: 'Fetch a page' }] } });
const place = (targetId = '19dc68be-f597-4535-8ebd-7759f544b27a', providerId = '29dc68be-f597-4535-8ebd-7759f544b27a'): SelectedPlace => ({ kind: 'cluster', target: { id: targetId, context_id: targetId, context_name: 'team-dev', service_name: 'orka', service_namespace: 'control', watched_namespace: 'agents', image: 'fixture-image' }, providerId });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });

test('without a connected destination no catalog is requested or blamed for failure', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(command => command === 'agent_open' ? pair : null); mockIPC(ipc);
  render(<Example remote place={{ kind: 'cluster' }} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getAllByText('Connect a cluster in Clusters to load tools and models').length).toBeGreaterThan(0);
  expect(screen.queryByText(/(?:Tools|Models) could not be read|Couldn't read (?:tools|models)/)).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'workspace_catalog' || c === 'cluster_catalog')).toBe(false);
  expect(screen.getByRole('option', { name: 'saved-model' })).toBeTruthy();
});
test('local catalog recovery does not claim that a running cluster is disconnected', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(command => command === 'agent_open' ? pair : null); mockIPC(ipc);
  render(<Example place={{ kind: 'thisComputer' }} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getAllByText('Open Clusters to load tools and models on This computer.').length).toBeGreaterThan(0);
  expect(screen.queryByText('Connect a cluster in Clusters to load tools and models')).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'workspace_catalog' || c === 'cluster_catalog')).toBe(false);
});

test('a connected cluster without a chosen AI connection does not request or blame a catalog', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(command => command === 'agent_open' ? pair : null); mockIPC(ipc);
  render(<Example remote place={{ ...place(), kind: 'cluster', providerId: undefined }} catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getAllByText('Choose an AI connection in Clusters to load tools and models').length).toBeGreaterThan(0);
  expect(screen.queryByText('Connect a cluster in Clusters to load tools and models')).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'workspace_catalog' || c === 'cluster_catalog')).toBe(false);
  expect(screen.getByRole('option', { name: 'saved-model' })).toBeTruthy();
});

test.each([false, true])('failed catalog is attributed only to its actual destination (remote=%s)', async remote => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : null);
  render(<Example remote={remote} place={remote ? place() : { kind: 'thisComputer' }} catalogKey={1} />);
  const destination = remote ? 'team-dev · control/orka · agents' : 'This computer';
  await screen.findByText(`Couldn't read models from ${destination}. Check Clusters or edit File view.`);
  expect(screen.getByText(`Couldn't read tools from ${destination}. Check Clusters or edit File view.`, { selector: 'p' })).toBeTruthy();
});
test('catalog failure distinguishes installations sharing a context and watched namespace', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : null);
  const selected = place();
  if (selected.kind === 'cluster' && selected.target) selected.target = { ...selected.target, service_namespace: 'other-control', service_name: 'other-orka' };
  render(<Example remote place={selected} catalogKey={1} />);
  await screen.findByText("Couldn't read models from team-dev · other-control/other-orka · agents. Check Clusters or edit File view.");
  expect(screen.getByText("Couldn't read tools from team-dev · other-control/other-orka · agents. Check Clusters or edit File view.", { selector: 'p' })).toBeTruthy();
});
test('paused catalog makes no request and does not manufacture a failed-cluster message', async () => {
  vi.stubGlobal('isTauri', true); const ipc = vi.fn(command => command === 'agent_open' ? pair : null); mockIPC(ipc);
  render(<Example remote place={place()} catalogKey={1} catalogPaused />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getAllByText('Tools and models are paused while another operation finishes.').length).toBeGreaterThan(0);
  expect(screen.queryByText(/Couldn't read (?:tools|models)|(?:Tools|Models) could not be read/)).toBeNull();
  expect(ipc.mock.calls.some(([c]) => c === 'cluster_catalog')).toBe(false);
});

test.each(['target', 'provider'])('remote catalog is scoped to the selected %s and rejects its late reply', async changed => {
  vi.stubGlobal('isTauri', true); const pending: ((value: unknown) => void)[] = []; const args: unknown[] = [];
  mockIPC((command, values) => {
    if (command === 'agent_open') return pair;
    if (command === 'cluster_catalog') { args.push(values); return new Promise(resolve => pending.push(resolve)); }
    return null;
  });
  const first = place(); const view = render(<Example remote place={first} catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await waitFor(() => expect(pending).toHaveLength(1));
  view.rerender(<Example remote place={changed === 'target' ? place('39dc68be-f597-4535-8ebd-7759f544b27a') : place(undefined, '49dc68be-f597-4535-8ebd-7759f544b27a')} catalogKey={1} />);
  await waitFor(() => expect(pending).toHaveLength(2));
  await act(async () => pending[0](catalog('old-model')));
  expect(screen.queryByRole('option', { name: 'old-model' })).toBeNull();
  await act(async () => pending[1](catalog('new-model')));
  expect(screen.getByRole('option', { name: 'new-model' })).toBeTruthy();
  expect((screen.getByRole('combobox', { name: 'It thinks with' }) as HTMLSelectElement).value).toBe('saved-model');
  expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(false);
  expect(Object.keys(args[0] as object).sort()).toEqual(['providerId', 'targetId']);
});

test('a busy first catalog stays loading and becomes a tool list without Refresh', async () => {
  vi.stubGlobal('isTauri', true); let held = true;
  mockIPC(command => {
    if (command === 'agent_open') return pair;
    if (command === 'workspace_catalog') { if (held) throw 'catalogBusy'; return catalog('ready-model'); }
    return null;
  });
  const user = userEvent.setup(); render(<Example catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  expect(screen.getByText('Loading available tools and models…')).toBeTruthy();
  expect(screen.queryByText(/Tools could not be read/)).toBeNull();
  expect(screen.queryByRole('button', { name: 'Refresh tools and models' })).toBeNull();
  held = false;
  await waitFor(() => expect(screen.getByRole('button', { name: 'Add a tool' }).matches(':disabled')).toBe(false));
  await user.click(screen.getByRole('button', { name: 'Add a tool' }));
  expect(screen.getByText('Read the contents of a web page at an address.')).toBeTruthy();
  expect(screen.queryByText('Loading available tools and models…')).toBeNull();
});

test('Stop clears a busy catalog wait and an older read cannot restart discovery', async () => {
  vi.stubGlobal('isTauri', true); let calls = 0;
  mockIPC(command => {
    if (command === 'agent_open') return pair;
    if (command === 'workspace_catalog') { calls++; throw 'catalogBusy'; }
    return null;
  });
  const view = render(<Example catalogKey={1} />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await waitFor(() => expect(calls).toBeGreaterThan(1));
  expect(screen.getByText('Loading available tools and models…')).toBeTruthy();
  view.rerender(<Example setupPending />);
  expect(screen.queryByText('Loading available tools and models…')).toBeNull();
  expect(screen.getByText('Tools will be available when the workspace is ready.', { selector: 'p' })).toBeTruthy();
  const stoppedCalls = calls;
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 550)); });
  expect(calls).toBe(stoppedCalls);
});

test('failure guidance opens the editable instructions without rerunning tests', async () => {
  vi.stubGlobal('isTauri', true); const runs: string[] = [];
  mockIPC(command => {
    if (command === 'agent_open') return pair;
    if (command === 'agent_run') runs.push(command);
    if (command === 'agent_run_status') return { documentId, busy: null, state: 'complete', canRun: true, modelConnected: true, report: { verdict: 'failed', cases: [{ id: 'reads', question: 'Read a page', phase: 'Succeeded', answer: 'Other answer', completedTools: ['web_fetch'], assertions: [{ id: 'contains', kind: 'contains', expected: 'checksum', verdict: 'failed' }], verdict: 'failed' }] }, acknowledged: [], pending: null, error: null };
    return null;
  });
  const user = userEvent.setup(); render(<Example />);
  await user.click(await screen.findByRole('button', { name: 'File' }));
  await user.click(await screen.findByRole('button', { name: 'Edit instructions' }));
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Instructions' })));
  expect(runs).toEqual([]);
});

test.each(['different-model', null])('remote defaults do not claim that a saved model is unavailable (%s)', async defaultModel => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : command === 'cluster_catalog' ? { ...catalog('different-model'), defaultModel, models: { state: 'ready', items: defaultModel ? [{ id: defaultModel }] : [] } } : null);
  render(<Example remote place={place()} catalogKey={1} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'It thinks with' }).matches(':disabled')).toBe(false));
  expect(screen.getByRole('option', { name: 'saved-model' })).toBeTruthy();
  expect(screen.queryByText(/not listed by the connected AI/)).toBeNull();
  if (defaultModel === null) expect(screen.getByText(/No default model is configured for this AI connection/)).toBeTruthy();
});

test('a restored active Send keeps its completed result and unconfirmed recovery visible', async () => {
  vi.stubGlobal('isTauri', true); let state = 'running';
  const report = { verdict: 'passed', cases: [{ id: 'reads', question: 'Read', phase: 'Succeeded', answer: 'Saved completed answer', completedTools: [], verdict: 'passed', assertions: [{ id: 'text', kind: 'contains', expected: 'answer', verdict: 'passed' }] }] };
  mockIPC(command => command === 'agent_open' ? pair : command === 'send_history' ? [] : command === 'send_status' ? { documentId, busy: state === 'running' ? { documentId, name: 'reader' } : null, view: { documentId, id: documentId, targetId: documentId, providerId: documentId, targetName: 'team-dev', source: null, model: null, agentName: null, state, stage: state === 'running' ? 'capture' : 'done', acknowledged: [], pending: null, report: state === 'complete' ? report : null, error: null, storageError: false, cancellation: state === 'unknown' ? 'unconfirmed' : null } } : null);
  render(<Example />);
  const panel = await screen.findByRole('region', { name: 'Send and cluster tests' });
  state = 'complete';
  await waitFor(() => expect(panel.textContent).toContain('Saved completed answer'));
  expect(screen.getByRole('region', { name: 'Send and cluster tests' })).toBe(panel);
  expect(screen.getByRole('region', { name: 'Past Sends' }).closest('[hidden]')).toBeNull();
  state = 'unknown';
  await waitFor(() => expect(panel.textContent).toContain('Cancellation not confirmed'));
  expect(screen.getByRole('region', { name: 'Send and cluster tests' })).toBe(panel);
});

test('Personality offers an example without inserting it in the agent file', async () => {
  vi.stubGlobal('isTauri', true); mockIPC(command => command === 'agent_open' ? pair : null);
  const user = userEvent.setup(); render(<Example />);
  await screen.findByRole('textbox', { name: 'Instructions' });
  await user.click(screen.getByRole('button', { name: 'More settings' }));
  const field = screen.getByRole('textbox', { name: 'Personality' }) as HTMLTextAreaElement;
  expect(field.placeholder).toContain('Be concise'); expect(field.value).toBe('');
});
