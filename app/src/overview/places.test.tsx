import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, test, vi } from 'vitest';
import Overview from './Overview';
import { Board } from './places';

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
const group = (name: string) => screen.findByRole('region', { name });
const buttons = (el: HTMLElement) => within(el).getAllByRole('button').map(b => b.textContent);

test('the board explicitly marks a single stack and removes that layout when a second stack appears', () => {
  const groups = [{ key: 'local', title: 'This computer', notes: [], cards: [] }];
  const actions = { onOpen: vi.fn(), onCheck: vi.fn(), onClusters: vi.fn() };
  const view = render(<Board stacks={[groups, []]} byAgent={false} actions={actions} />);
  expect(view.container.querySelector('.overview-board-single')).toBeTruthy();
  view.rerender(<Board stacks={[groups, [{ ...groups[0], key: 'team', title: 'Team' }]]} byAgent={false} actions={actions} />);
  expect(view.container.querySelector('.overview-board-single')).toBeNull();
  expect(view.container.querySelectorAll('.overview-board > .overview-stack')).toHaveLength(2);
});

test('By cluster: This computer first, then each cluster by context name, with a count at the right', async () => {
  ipc(); render(<Overview {...props} />);
  const first = await group('This computer');
  expect(within(first).queryByText(/^(local cluster|workspace)$/)).toBeNull();
  const remote = await group('team-one');
  expect(within(remote).getByText('2 agents')).toBeTruthy();
  const order = screen.getAllByRole('heading', { level: 2 }).map(h => h.textContent);
  expect(order[0]).toBe('This computer'); expect(order.slice().sort()).toEqual(['This computer', 'team-one', 'team-two']);
});

test('remote cards keep a verdict only from a Bridge record, link a file only on exactly one hash match, and offer Open only', async () => {
  ipc(); render(<Overview {...props} />);
  const remote = await group('team-one');
  const linked = within(remote).getByRole('article', { name: /^reader/ });
  expect(within(linked).getByText('Passed')).toBeTruthy();
  expect(buttons(linked)).toEqual(['Open', 'Details']);
  await userEvent.click(within(linked).getByRole('button', { name: 'Open reader on team-one' }));
  expect(props.onOpenDocument).toHaveBeenCalledWith(id, 'one');
  const external = within(remote).getByRole('article', { name: /^external-agent/ });
  expect(within(external).queryByText(/Passed|Failed|Not tested|Result unknown/)).toBeNull();
  expect(buttons(external)).toEqual(['Details']);
  expect(within(external).getByText('Running on team-one, not saved here')).toBeTruthy();
});

test('local disconnection never blocks a team Run with its own destination', async () => {
  ipc();
  const run = vi.fn(); render(<Overview {...props} localAiConnected={false} onChatDocument={vi.fn()} onRunDocument={run} />);
  const remote = within(await group('team-one')).getByRole('button', { name: 'Run tests reader on team-one' });
  expect(remote.matches(':disabled')).toBe(false);
  await userEvent.click(remote);
  expect(run).toHaveBeenCalledWith(id, 'one');
  expect(screen.getByRole('button', { name: 'Run tests reader' }).matches(':disabled')).toBe(true);
});

test('document evidence survives edits and identical copies, with cluster counts and safe action targets', async () => {
  const enriched = { ...record, documentId: id, detailState: 'saved', report: { verdict: 'passed', passed: 2, total: 3 } };
  ipc(places([place('one', 'team-one', [{ ...agent, record: enriched }])]), saved([{ ...file, source: { ...source, tests: 'c'.repeat(64) } }, { ...file, documentId: id2, name: 'copy', summary: { ...file.summary, name: 'copy' } }]));
  const onRunDocument = vi.fn(); render(<Overview {...props} onRunDocument={onRunDocument} />);
  const remote = await group('team-one');
  const linked = await within(remote).findByRole('article', { name: 'reader, Not run since your edit' });
  expect(within(linked).getByText(/2 of 3 passed, before your edit/)).toBeTruthy();
  expect(buttons(linked)).toEqual(['Open', 'Run tests', 'Details']);
  expect(within(linked).getByRole('button', { name: 'Run tests reader on team-one' }).className).toContain('button-primary');
  await userEvent.click(within(linked).getByRole('button', { name: /^Run tests/ }));
  expect(onRunDocument).toHaveBeenCalledWith(id, 'one');
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  expect(within(screen.getByRole('region', { name: 'reader' })).getAllByRole('article')).toHaveLength(2);
  expect(within(screen.getByRole('region', { name: 'copy' })).getAllByRole('article')).toHaveLength(1);
});

test('a failed Send on earlier files remains failed and counted in the tab summary', async () => {
  const counts = vi.fn();
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, verdict: 'failed', documentId: id, detailState: 'saved', report: { verdict: 'failed', passed: 0, total: 1 } } }])]), saved([{ ...file, source: { ...source, agent: 'c'.repeat(64) } }]));
  render(<Overview {...props} onCounts={counts} onRunDocument={vi.fn()} />);
  const remote = await group('team-one');
  expect(await within(remote).findByText('Failed')).toBeTruthy();
  expect(within(remote).getByText(/0 of 1 passed, before your edit/)).toBeTruthy();
  expect(within(remote).getByRole('button', { name: /^Open/ }).className).toContain('button-primary');
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents, 1 failed'));
});

test.each([
  { documentId: id2, detailState: 'saved' },
  { documentId: null, detailState: 'removed' },
  { documentId: null, detailState: 'unavailable' },
])('new document evidence never silently rebinds by hash: %j', async evidence => {
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, ...evidence, report: null } }])]));
  render(<Overview {...props} onRunDocument={vi.fn()} />);
  const remote = await group('team-one');
  expect(await within(remote).findByText('Running on team-one, not saved here')).toBeTruthy();
  expect(within(remote).queryByRole('button', { name: /^Open|^Run tests/ })).toBeNull();
});

test.each(['omitted', 'unavailable', 'legacy', 'removed'])('untrusted %s detail counts are not accepted as passing evidence', async detailState => {
  ipc(places([place('one', 'team-one', [{ ...agent, record: { ...record, documentId: detailState === 'legacy' ? null : id, detailState, report: { verdict: 'passed', passed: 1, total: 1 } } }])]));
  render(<Overview {...props} />);
  expect(await screen.findByText(/Couldn't read clusters just now/)).toBeTruthy();
  expect(screen.queryByText(/1 of 1 passed/)).toBeNull();
});

test('an untouched blank draft is editable but never advertised as runnable or broken in either grouping', async () => {
  ipc(places([]), saved([{ ...file, draft: true, summary: null, reason: 'Add instructions and a test to get started.', localResult: null }]));
  render(<Overview {...props} onRunDocument={vi.fn()} onChatDocument={vi.fn()} />);
  const here = await group('This computer');
  expect(await within(here).findByText('Draft')).toBeTruthy();
  expect(buttons(within(here).getByRole('article'))).toEqual(['Open', 'Details']);
  expect(within(here).getByText('Add instructions and a test to get started.')).toBeTruthy();
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  expect(within(screen.getByRole('region', { name: 'reader' })).getByText('Draft')).toBeTruthy();
  expect(screen.queryByText(/unsupported|invalid/i)).toBeNull();
});

test('a cancelled cluster result remains neutral rather than failed or passed', async () => {
  ipc(places([{ ...two, agents: [{ ...agent, record: { ...record, verdict: 'cancelled' } }] }]));
  render(<Overview {...props} />);
  const remote = await group('team-two');
  expect(within(remote).getByText('Not tested')).toBeTruthy();
  expect(remote.querySelector('.status-passed, .status-failed')).toBeNull();
  expect(remote.querySelector('.status-not-started')).toBeTruthy();
});

test('identical saved files are ambiguous: the cluster card stays unlinked, local cards stay one per document', async () => {
  ipc(places([two]), saved([file, { ...file, documentId: id2, name: 'copy', summary: { ...file.summary, name: 'copy' } }]));
  render(<Overview {...props} />);
  const remote = await group('team-two');
  expect(within(remote).queryByRole('button', { name: /^Open/ })).toBeNull();
  expect(within(remote).getByText('Running on team-two, not saved here')).toBeTruthy();
  const here = await group('This computer');
  expect(within(here).getAllByRole('article')).toHaveLength(2);
  expect(within(here).queryByText(/also on/)).toBeNull();
});

test('also on comes from unique document links: names for two, a count for more', async () => {
  const third = place('three', 'team-three', [{ ...agent, uid: 'u3', record: { ...record, agentUid: 'u3' } }]);
  ipc(places([one, two]), saved()); const view = render(<Overview {...props} />);
  const here = await group('This computer');
  expect(within(here).getByText(/also on/).closest('p')!.textContent).toBe('declared-model · web_fetch · also on team-one and team-two');
  const remote = within(await group('team-two')).getByRole('article', { name: /^reader/ });
  expect(within(remote).getByText(/also on/).closest('p')!.textContent).toBe('observed-model · web_fetch · also on this computer and team-one');
  view.unmount(); ipc(places([one, two, third])); render(<Overview {...props} />);
  expect((await within(await group('This computer')).findByText(/also on/)).textContent).toContain('also on 3 clusters');
});

test('same-context clusters stay distinguishable', async () => {
  ipc(places([place('a', 'same', [], { service: 'control/a' }), place('b', 'same', [], { service: 'control/b' })])); render(<Overview {...props} />);
  expect(await group('same · agents · control/a')).toBeTruthy(); expect(screen.getByRole('region', { name: 'same · agents · control/b' })).toBeTruthy();
});

test('By agent: one group per file with a card per cluster, agents without a file last with their cluster', async () => {
  ipc(); render(<Overview {...props} />);
  await group('team-one');
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  expect(screen.getByRole('button', { name: 'By agent' }).getAttribute('aria-pressed')).toBe('true');
  const doc = screen.getByRole('region', { name: 'reader' });
  expect(within(doc).getAllByText('Reads a page')).toHaveLength(1);
  expect(within(doc).getAllByRole('heading', { level: 3 }).map(h => h.textContent)).toEqual(['This computer', 'team-one', 'team-two']);
  const none = screen.getByRole('region', { name: 'Running agents, not saved here' });
  expect(within(none).getByRole('heading', { level: 3, name: 'external-agent' })).toBeTruthy();
  expect(within(none).getByText('team-one', { selector: 'code' }).closest('p')!.textContent).toBe('observed-model · web_fetch · on team-one');
  const titles = screen.getAllByRole('heading', { level: 2 }).map(h => h.textContent);
  expect(titles).toEqual(['reader', 'Running agents, not saved here']);
});

test('By agent keeps an unreadable saved document in its own group with a cluster title', async () => {
  ipc(places([]), saved([{ ...file, state: 'unreadable', source: null, summary: null, reason: 'The saved file is unreadable.' }]));
  render(<Overview {...props} />); await group('This computer');
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  const doc = screen.getByRole('region', { name: 'reader' });
  expect(within(doc).getByRole('heading', { level: 3, name: 'This computer' })).toBeTruthy();
  expect(within(doc).getByText('The saved file is unreadable.')).toBeTruthy();
  expect(within(doc).queryByRole('button', { name: /^Open/ })).toBeNull();
});

test('By agent keeps cluster trouble explanations and recovery actions visible', async () => {
  ipc(places([{ ...two, stale: true }]), saved(), { ...local, state: 'unavailable', reason: 'stopped' });
  render(<Overview {...props} />); await group('team-two');
  await userEvent.click(screen.getByRole('button', { name: 'By agent' }));
  expect(screen.getByText('The workspace is stopped. Start it to test or chat.')).toBeTruthy();
  expect(screen.getByText(/Couldn't check just now/)).toBeTruthy();
  expect(screen.getAllByRole('button', { name: 'Open Clusters' })).toHaveLength(2);
});

test('groups go to two stacks by card count and compact is one stack in the same logical order', async () => {
  const many = place('many', 'big', [agent, { ...agent, uid: 'b' }, { ...agent, uid: 'c' }].map((a, i) => ({ ...a, record: null, uid: `m${i}` })));
  ipc(places([many, place('s1', 'small-1', []), place('s2', 'small-2', [])])); const view = render(<Overview {...props} />);
  await group('big');
  const stacks = () => [...document.querySelectorAll('.overview-stack')].map(s => [...s.querySelectorAll('h2')].map(h => h.textContent));
  // Greedy by card count: the tall group sits alone in its stack while the short ones share the other.
  expect(stacks()).toEqual([['This computer', 'small-1', 'small-2'], ['big']]);
  view.unmount();
  vi.stubGlobal('matchMedia', (q: string) => ({ matches: true, media: q, addEventListener: () => {}, removeEventListener: () => {} }));
  render(<Overview {...props} />); await group('big');
  expect(stacks()[0]).toEqual(['This computer', 'big', 'small-1', 'small-2']);
});

test('a cluster not connected since startup shows no count and no invented empty or green state', async () => {
  ipc(places([place('west', 'team-west', [], { observedAt: null, stale: true, checkedAt: 1234567890000 })]));
  const choose = vi.fn(); render(<Overview {...props} onChooseWorkspace={choose} />);
  const west = await group('team-west');
  expect(within(west).getByText('Not connected since Bridge started. Connect it to see its agents.')).toBeTruthy();
  expect(within(west).queryByText(/^\d+ agents?$/)).toBeNull();
  expect(within(west).queryByText(/No agents in/)).toBeNull();
  await userEvent.click(within(west).getByRole('button', { name: 'Open Clusters' })); expect(choose).toHaveBeenCalled();
});

test('a stale cluster keeps cards as Result unknown with Open only, offers Check again, and recovers on a fresh read', async () => {
  ipc(places([{ ...two, stale: true }])); render(<Overview {...props} />);
  const remote = await group('team-two');
  expect(within(remote).getByText("Couldn't check just now.", { exact: false })).toBeTruthy();
  expect(within(remote).getByText(/Showing what Bridge saw/)).toBeTruthy();
  expect(within(remote).getByText('Result unknown')).toBeTruthy();
  expect(within(remote).getByText('Last Send passed', { exact: false })).toBeTruthy();
  expect(remote.querySelector('.status-passed')).toBeNull();
  expect(within(remote).getByRole('button', { name: 'Open Clusters' })).toBeTruthy();
  ipc(places([two])); await userEvent.click(within(remote).getByRole('button', { name: 'Check again' }));
  await waitFor(() => expect(within(screen.getByRole('region', { name: 'team-two' })).getByText('Passed')).toBeTruthy());
});

test('a failed cluster read keeps the last good clusters flagged stale, never as fresh', async () => {
  ipc(); render(<Overview {...props} />);
  await group('team-two');
  mockIPC(command => { if (command === 'overview_places') throw Error('nope'); return command === 'overview_files' ? saved() : local; });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByText(/Couldn't read clusters just now/)).toBeTruthy();
  const remote = screen.getByRole('region', { name: 'team-two' });
  expect(within(remote).getByText('Result unknown')).toBeTruthy(); expect(within(remote).queryByText('Passed')).toBeNull();
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test('without saved Send summaries cluster cards say they could not check, and Earlier Sends never appears', async () => {
  ipc(places([two], { historyState: 'unavailable', unobserved: [{ ...record, agentUid: null, verdict: 'unknown' }] })); render(<Overview {...props} />);
  const remote = await group('team-two');
  expect(within(remote).getByText("Couldn't check")).toBeTruthy();
  expect(remote.querySelector('.pill.status-needs-you')).toBeTruthy();
  expect(screen.getByRole('alert').textContent).toContain('Saved Send results are unavailable');
  expect(screen.queryByText('Earlier Sends')).toBeNull(); expect(screen.queryByRole('region', { name: 'Earlier Sends' })).toBeNull();
});

test('cluster Details hold the generated identity; nothing generated is on the card face', async () => {
  ipc(places([two])); render(<Overview {...props} />);
  const c = within(await group('team-two')).getByRole('article', { name: /^reader/ });
  expect(c.textContent).not.toMatch(/uid-three|same-name/);
  await userEvent.click(within(c).getByRole('button', { name: 'Details' }));
  expect(within(c).getByText('same-name')).toBeTruthy(); expect(within(c).getByRole('button', { name: 'Copy Agent ID' })).toBeTruthy();
});

test('unlinked local Agent and a cluster with no agents', async () => {
  ipc(places([place('empty', 'team-empty', [])]), saved([]), { ...local, agents: [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }] });
  render(<Overview {...props} />);
  expect(await screen.findByText('No agents in agents yet.')).toBeTruthy();
  expect(within(await group('This computer')).getByRole('heading', { level: 3, name: 'mine' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test('replies after hiding are ignored', async () => {
  let resolve!: (value: unknown) => void; ipc();
  mockIPC(command => command === 'overview_places' ? new Promise(r => { resolve = r; }) : command === 'overview_files' ? saved() : local);
  const view = render(<Overview {...props} />);
  await waitFor(() => expect(resolve).toBeTypeOf('function'));
  view.rerender(<Overview {...props} active={false} />);
  await act(async () => { resolve(places()); await Promise.resolve(); });
  expect(view.container.querySelector('section[aria-label="team-one"]')).toBeNull();
});
