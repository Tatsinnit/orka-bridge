import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const id = '12345678-1234-1234-1234-123456789abc';
const id2 = '22345678-1234-1234-1234-123456789abc';
const file = { documentId: id, name: 'reader', state: 'readable', source, summary: { name: 'reader', description: 'Reads a page', model: 'declared-model', tools: ['web_fetch'] }, reason: null, localResult: null };
const record = { id, source, targetDisplay: 'team-one · agents', targetUids: { serviceNamespace: 'control', serviceUid: 'svc-one', deploymentNamespace: 'control', deploymentUid: 'dep-one', watchedNamespace: 'agents' }, provider: 'team-model', plannedAgentName: 'same-name', agentUid: 'uid-one', verdict: 'passed', time: 1234567890 };
const agent = { name: 'same-name', namespace: 'agents', uid: 'uid-one', agentType: 'native', model: 'observed-model', tools: ['web_fetch'], record };
const unrelated = { ...agent, uid: 'external', name: 'external-agent', record: null };
const place = (key: string, context: string, agents: unknown[], over: object = {}) => ({ key, context, service: 'control/orka', namespace: 'agents', stale: false, observedAt: 1234567890000, agents, ...over });
const one = place('one', 'team-one', [agent, unrelated]);
const two = place('two', 'team-two', [{ ...agent, uid: 'uid-three', record: { ...record, agentUid: 'uid-three' } }]);
const places = (list: unknown[] = [one, two], over: object = {}) => ({ places: list, historyState: 'complete', unobserved: [], ...over });
const saved = (list: unknown[] = [file]) => ({ state: 'complete', files: list, runActive: false, historyState: 'complete', runAgents: [] });
const local = { state: 'complete', installation: id, agents: [], observedAt: 123, observation: null, runActive: false, reason: null };
const props = { active: true, runBusy: false, localAiConnected: true, workspaceKey: 'ready', onOpenDocument: vi.fn(), onOpenExample: vi.fn(), onChooseWorkspace: vi.fn() };
function ipc(value: unknown = places(), files: unknown = saved(), live: unknown = local) {
  vi.stubGlobal('isTauri', true);
  mockIPC(command => command === 'overview_places' ? value : command === 'overview_files' ? files : live);
}
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); vi.clearAllMocks(); });
const row = (name = 'reader') => screen.findByRole('article', { name });
async function expanded(name = 'reader') {
  const agent = await row(name);
  const toggle = within(agent).getByRole('button', { name: `Show places for ${name}` });
  if (toggle.getAttribute('aria-expanded') === 'false') await userEvent.click(toggle);
  return agent;
}
const buttons = (el: HTMLElement) => within(el).getAllByRole('button').map(b => b.textContent);

test('one linked row retains distinct place evidence and keeps Open at the parent', async () => {
  ipc(); render(<Overview {...props} />);
  const parent = await expanded();
  expect(parent.getAttribute('aria-label')).toBe('reader');
  expect(within(parent).getAllByRole('article', { name: /team-one,|team-two,/ })).toHaveLength(2);
  expect(buttons(within(parent).getByRole('article', { name: 'This computer, Not tested' }))).toEqual(['Details']);
  const remote = within(parent).getByRole('article', { name: 'team-one, Passed' });
  expect(buttons(remote)).toEqual(['Details']);
  await userEvent.click(within(parent).getByRole('button', { name: 'Open reader' }));
  expect(props.onOpenDocument).toHaveBeenCalledWith(id);
  expect(within(remote).queryByRole('button', { name: /Open/ })).toBeNull();
});

test('an unlinked remote Agent has no Open or Run action and keeps Details evidence', async () => {
  ipc(); render(<Overview {...props} />);
  const parent = await expanded('external-agent');
  expect(within(parent).getByText('Running on team-one, not saved here')).toBeTruthy();
  const remote = within(parent).getByRole('article', { name: 'team-one, No recorded result' });
  expect(within(remote).queryByRole('button', { name: /^Open|^Run tests/ })).toBeNull();
  expect(buttons(remote)).toEqual(['Details']);
  await userEvent.click(within(remote).getByRole('button', { name: 'Details' }));
  expect(within(remote).getByText('external-agent')).toBeTruthy();
  expect(within(remote).getByText('Agent ID')).toBeTruthy();
});

test('local disconnection never blocks the fitting team Run with its own destination', async () => {
  ipc(); const run = vi.fn(); render(<Overview {...props} localAiConnected={false} onChatDocument={vi.fn()} onRunDocument={run} />);
  const parent = await expanded();
  const remote = within(parent).getByRole('button', { name: 'Run tests reader on team-one' });
  const local = within(parent).getByRole('button', { name: 'Run tests reader' });
  expect(remote).toHaveProperty('disabled', false); expect(local).toHaveProperty('disabled', true);
  await userEvent.click(remote); expect(run).toHaveBeenCalledWith(id, 'one');
});

test('document evidence survives edits while ambiguous hash copies never silently link', async () => {
  const enriched = { ...record, documentId: id, detailState: 'saved', report: { verdict: 'passed', passed: 2, total: 3 } };
  ipc(places([place('one', 'team-one', [{ ...agent, record: enriched }])]), saved([{ ...file, source: { ...source, tests: 'c'.repeat(64) } }, { ...file, documentId: id2, name: 'copy', summary: { ...file.summary, name: 'copy' } }]));
  const run = vi.fn(); render(<Overview {...props} onRunDocument={run} />);
  const parent = await expanded();
  const remote = within(parent).getByRole('article', { name: 'team-one, Not run since your edit' });
  expect(within(remote).getByText(/2 of 3 passed, before your edit/)).toBeTruthy();
  await userEvent.click(within(remote).getByRole('button', { name: /^Run tests/ }));
  expect(run).toHaveBeenCalledWith(id, 'one');
  expect(within(parent).queryByText(/button-primary/)).toBeNull();
  cleanup();
  ipc(places([two]), saved([file, { ...file, documentId: id2, name: 'copy', summary: { ...file.summary, name: 'copy' } }]));
  render(<Overview {...props} />);
  const unlinked = await expanded('same-name');
  expect(within(unlinked).getByText('Running on team-two, not saved here')).toBeTruthy();
  expect(within(unlinked).queryByRole('button', { name: /^Open|^Run tests/ })).toBeNull();
});

test('a failed Send on earlier files remains failed and counts once per agent row', async () => {
  const counts = vi.fn();
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, verdict: 'failed', documentId: id, detailState: 'saved', report: { verdict: 'failed', passed: 0, total: 1 } } }])]), saved([{ ...file, source: { ...source, agent: 'c'.repeat(64) } }]));
  render(<Overview {...props} onCounts={counts} onRunDocument={vi.fn()} />);
  const parent = await expanded();
  expect(within(parent).getByRole('article', { name: 'team-one, Failed' })).toBeTruthy();
  expect(within(parent).getByText(/0 of 1 passed, before your edit/)).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent, 1 failed'));
});

test.each([
  { documentId: id2, detailState: 'saved' },
  { documentId: null, detailState: 'removed' },
  { documentId: null, detailState: 'unavailable' },
])('new document evidence never silently rebinds by hash: %j', async evidence => {
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, ...evidence, report: null } }])]));
  render(<Overview {...props} onRunDocument={vi.fn()} />);
  const parent = await expanded('same-name');
  expect(within(parent).getByText('Running on team-one, not saved here')).toBeTruthy();
  expect(within(parent).queryByRole('button', { name: /^Open|^Run tests/ })).toBeNull();
});

test.each(['omitted', 'unavailable', 'legacy', 'removed'])('untrusted %s detail counts are not accepted as passing evidence', async detailState => {
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, documentId: detailState === 'legacy' ? null : id, detailState, report: { verdict: 'passed', passed: 1, total: 1 } } }])]));
  render(<Overview {...props} />);
  expect(await screen.findByText(/Couldn't read clusters just now/)).toBeTruthy();
  expect(screen.queryByText(/1 of 1 passed/)).toBeNull();
});

test('blank drafts are editable but expose no fitting action or broken state', async () => {
  ipc(places([]), saved([{ ...file, draft: true, summary: null, reason: 'Add instructions and a test to get started.', localResult: null }]));
  render(<Overview {...props} onRunDocument={vi.fn()} onChatDocument={vi.fn()} />);
  const parent = await expanded();
  expect(within(parent).getByText('Add instructions and a test to get started.')).toBeTruthy();
  const localRow = within(parent).getByRole('article', { name: 'This computer, Draft' });
  expect(buttons(localRow)).toEqual(['Details']);
  expect(screen.queryByText(/unsupported|invalid/i)).toBeNull();
});

test('cancelled remote and stale unknown evidence retain neutral or stale states', async () => {
  ipc(places([{ ...two, agents: [{ ...agent, record: { ...record, verdict: 'cancelled' } }] }]));
  const view = render(<Overview {...props} />); const parent = await expanded();
  const cancelled = within(parent).getByRole('article', { name: 'team-two, Not tested' });
  expect(cancelled.querySelector('.status-passed, .status-failed')).toBeNull();
  view.unmount();
  ipc(places([{ ...two, stale: true }])); render(<Overview {...props} />);
  const staleParent = await expanded();
  const stale = within(staleParent).getByRole('article', { name: 'team-two, Result unknown' });
  expect(within(stale).getByText('Last Send passed', { exact: false })).toBeTruthy();
  expect(stale.querySelector('.status-passed')).toBeNull();
  expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy();
});

test('same-context clusters stay distinguishable in filters and exact place rows', async () => {
  ipc(places([place('a', 'same', [agent], { service: 'control/a' }), place('b', 'same', [{ ...agent, uid: 'uid-two', record: { ...record, agentUid: 'uid-two' } }], { service: 'control/b' })]));
  render(<Overview {...props} />); const parent = await row();
  const control = screen.getByRole('group', { name: 'Place' });
  expect(within(control).getByRole('button', { name: 'same · agents · control/a' })).toBeTruthy();
  expect(within(control).getByRole('button', { name: 'same · agents · control/b' })).toBeTruthy();
  await userEvent.click(within(parent).getByRole('button', { name: 'Show places for reader' }));
  expect(within(parent).getByRole('article', { name: 'same · agents · control/a, Passed' })).toBeTruthy();
  expect(within(parent).getByRole('article', { name: 'same · agents · control/b, Passed' })).toBeTruthy();
});

test('a stale cluster keeps cached results unknown and recovers on a fresh read', async () => {
  ipc(places([{ ...two, stale: true }])); render(<Overview {...props} />);
  await expanded(); expect(screen.getByText(/Showing what Bridge saw/)).toBeTruthy();
  expect(screen.getByRole('article', { name: 'team-two, Result unknown' })).toBeTruthy();
  ipc(places([two])); await userEvent.click(screen.getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(screen.getByRole('article', { name: 'team-two, Passed' })).toBeTruthy());
});

test('a failed cluster read marks cached clusters stale and never becomes onboarding', async () => {
  ipc(); const view = render(<Overview {...props} />); await row();
  mockIPC(command => { if (command === 'overview_places') throw Error('nope'); return command === 'overview_files' ? saved() : local; });
  view.rerender(<Overview {...props} active={false} />); view.rerender(<Overview {...props} />);
  const error = screen.getByRole('article', { name: 'reader' });
  await userEvent.click(within(error).getByRole('button', { name: 'Show places for reader' }));
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
  // Re-read through the surviving failure control, not a removed top-level Refresh.
  const failed = await screen.findByText(/Couldn't read clusters just now/);
  expect(failed).toBeTruthy();
  await userEvent.click(within(failed.parentElement!).getByRole('button', { name: 'Check again' }));
  expect(await screen.findByRole('article', { name: 'team-two, Result unknown' })).toBeTruthy();
});

test('unavailable Send history produces an alert and no trusted pass evidence', async () => {
  ipc(places([{ ...two, agents: [{ ...agent, uid: 'uid-three', record: null }] }], { historyState: 'unavailable', unobserved: [{ ...record, agentUid: null, verdict: 'unknown' }] })); render(<Overview {...props} />);
  const parent = await expanded('same-name');
  expect(within(parent).getByRole('article', { name: "team-two, Couldn't check" })).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toContain('Saved Send results are unavailable');
  expect(screen.queryByText('Earlier Sends')).toBeNull();
});

test('cluster Details contain generated identity while the place face does not', async () => {
  ipc(places([two])); render(<Overview {...props} />);
  const remote = within(await expanded()).getByRole('article', { name: 'team-two, Passed' });
  expect(remote.textContent).not.toMatch(/uid-three|same-name/);
  await userEvent.click(within(remote).getByRole('button', { name: 'Details' }));
  expect(within(remote).getByText('same-name')).toBeTruthy();
  expect(within(remote).getByRole('button', { name: 'Copy Agent ID' })).toBeTruthy();
});

test('an unconnected or empty place is represented by status notes, not an invented green row', async () => {
  ipc(places([place('west', 'team-west', [], { observedAt: null, stale: true, checkedAt: 1234567890000 }), place('empty', 'team-empty', [])]), saved([]), { ...local, agents: [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }] });
  render(<Overview {...props} />);
  expect(await screen.findByText('Not connected since Bridge started. Connect it to see its agents.')).toBeTruthy();
  expect(screen.getByText('No agents in agents yet.')).toBeTruthy();
  const unlinked = await expanded('mine');
  expect(within(unlinked).getByRole('article', { name: 'This computer, No recorded result' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test('replies after hiding are ignored', async () => {
  let resolve!: (value: unknown) => void; ipc();
  mockIPC(command => command === 'overview_places' ? new Promise(r => { resolve = r; }) : command === 'overview_files' ? saved() : local);
  const view = render(<Overview {...props} />);
  await waitFor(() => expect(resolve).toBeTypeOf('function'));
  view.rerender(<Overview {...props} active={false} />);
  await act(async () => { resolve(places()); await Promise.resolve(); });
  expect(view.container.querySelector('.overview-places')).toBeNull();
});
