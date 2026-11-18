import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import Overview from './Overview';

const id = '9b2b93c4-375c-4815-9717-39852a96e411';
const id2 = '22345678-1234-1234-1234-123456789abc';
const id3 = '32345678-1234-1234-1234-123456789abc';
const installation = '12345678-1234-1234-1234-123456789abc';
const source = { agent: 'a'.repeat(64), tests: 'b'.repeat(64) };
const edited = { agent: 'a'.repeat(64), tests: 'c'.repeat(64) };
const runAgent = { installation, namespace: 'orka-system', agentUid: 'run-uid' };
const result = (verdict: string, passed: number, total: number, over: object = {}) => ({ installation, documentId: id, source, namespace: 'orka-system', agentName: 'bridge-run-1', agentUid: 'run-uid', observedAt: 1234567890000, state: 'complete', report: { verdict, passed, total }, ...over });
const file = { documentId: id, name: 'reader', state: 'readable', source, summary: { name: 'reader', description: 'Reads a page', model: 'gpt-test', tools: ['web_search'] }, reason: null, localResult: result('passed', 3, 3) };
const named = (documentId: string, name: string, over: object = {}) => ({ ...file, documentId, name, summary: { ...file.summary, name }, ...over });
const files = (list: unknown[] = [file], over: object = {}) => ({ state: 'complete', files: list, runActive: false, historyState: 'complete', runAgents: [runAgent], ...over });
const live = (agents: unknown[] = [], over: object = {}) => ({ state: 'complete', installation, agents, observedAt: 1234567890000, observation: null, runActive: false, reason: null, ...over });
const unavailable = { state: 'unavailable', installation: null, agents: [], observedAt: null, observation: null, runActive: false, reason: 'stopped' };
const none = { places: [], historyState: 'complete', unobserved: [] };
const noop = () => {};
const props = { active: true, runBusy: false, localAiConnected: true, workspaceKey: 'ready', onOpenDocument: noop, onChatDocument: noop, onRunDocument: noop, onOpenExample: noop, onChooseWorkspace: noop };
beforeEach(() => { vi.stubGlobal('isTauri', true); });
afterEach(() => { cleanup(); clearMocks(); vi.unstubAllGlobals(); });
function ipc(saved: unknown = files(), agents: unknown = live(), places: unknown = none) {
  mockIPC(command => command === 'overview_files' ? saved : command === 'overview_places' ? places : agents);
}
const row = (name: RegExp | string = /^reader$/) => screen.findByRole('article', { name });
async function expanded(name: RegExp | string = /^reader$/) {
  const agent = await row(name);
  const title = agent.getAttribute('aria-label');
  const toggle = within(agent).getByRole('button', { name: `Show places for ${title}` });
  if (toggle.getAttribute('aria-expanded') === 'false') await userEvent.click(toggle);
  return agent;
}
const buttons = (el: HTMLElement) => within(el).queryAllByRole('button');

test.each(['Starting the local cluster', 'Getting your workspace ready…', 'Stopping the local cluster', 'Setup failed'])('workspace status wins over a stopped inventory: %s', async workspaceNote => {
  ipc(files(), unavailable); render(<Overview {...props} workspaceNote={workspaceNote} />);
  expect(await screen.findByText(workspaceNote)).toBeTruthy();
  expect(screen.queryByText(/The workspace is stopped/)).toBeNull();
  const agent = await expanded();
  expect(within(agent).getByRole('button', { name: 'Open reader' })).toBeTruthy();
  expect(within(agent).queryByRole('button', { name: /^Chat|^Run tests/ })).toBeNull();
});

test('setup status blocks readiness actions even if the last live inventory is complete', async () => {
  ipc(); const view = render(<Overview {...props} />); await expanded();
  view.rerender(<Overview {...props} workspaceNote="Starting the local cluster" />);
  const agent = await row(); expect(within(agent).queryByRole('button', { name: /^Chat|^Run tests/ })).toBeNull();
  expect(within(agent).getByRole('button', { name: 'Open reader' })).toBeTruthy();
});

test('generated Chat runtime Agents are hidden without hiding saved, remote or similarly named Agents', async () => {
  const generated = `bridge-chat-agent-${'a'.repeat(32)}`;
  const ownFile = named(id2, generated, { localResult: null });
  const runtime = (name: string, uid: string) => ({ name, namespace: 'orka-system', uid });
  const remote = { key: 'team', context: 'team', service: 'control/orka', namespace: 'agents', stale: false, observedAt: 1234567890000,
    agents: [{ ...runtime(generated, 'remote'), namespace: 'agents', agentType: 'runtime', model: null, tools: null, record: null }] };
  ipc(files([file, ownFile]), live([runtime(generated, 'chat'), runtime('bridge-chat-agent-mine', 'mine')]), { ...none, places: [remote] });
  const counts = vi.fn(); render(<Overview {...props} onCounts={counts} />);
  await row(/^bridge-chat-agent-mine$/); expect(screen.getAllByRole('heading', { name: generated })).toHaveLength(2);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('4 agents'));
  expect(screen.getAllByRole('article', { name: /^(reader|bridge-chat-agent)/ })).toHaveLength(4);
});

test.each([
  [files([]), 'No saved agents yet'], [files(), '1 saved agent'], [{ state: 'unavailable', files: [], runActive: false }, 'Saved files unavailable'],
])('an unvisited hidden Overview reports saved inventory only: %s', async (saved, label) => {
  const counts = vi.fn(); const commands: string[] = [];
  mockIPC(command => { commands.push(command); return command === 'overview_files' ? saved : none; });
  render(<Overview {...props} active={false} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith(label)); expect(commands).toEqual(['overview_files']);
});

test('activation discards an obsolete initial hidden read', async () => {
  let finish!: (value: unknown) => void; const counts = vi.fn();
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  const view = render(<Overview {...props} active={false} onCounts={counts} />);
  await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([file, named(id2, 'new-agent', { localResult: null })])); view.rerender(<Overview {...props} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents'));
  await act(async () => finish(files([]))); expect(counts).toHaveBeenLastCalledWith('2 agents');
});

test('registration refreshes visited Overview counts while hidden without reading live Agents', async () => {
  const counts = vi.fn(); ipc(); const view = render(<Overview {...props} filesRevision={0} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  view.rerender(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  let inventories = 0;
  mockIPC(command => command === 'overview_files' ? files([file, named(id2, 'new-agent', { localResult: null })]) : (inventories++, live()));
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents')); expect(inventories).toBe(0);
});

test('a workspace transition rearms a cancelled registration refresh while hidden', async () => {
  const counts = vi.fn(); ipc(); const view = render(<Overview {...props} filesRevision={0} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  view.rerender(<Overview {...props} active={false} filesRevision={0} onCounts={counts} />);
  let finish!: (value: unknown) => void;
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />); await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([file, named(id2, 'new-agent', { localResult: null })]));
  view.rerender(<Overview {...props} active={false} filesRevision={1} workspaceKey="starting" onCounts={counts} />);
  await act(async () => finish(files())); await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents shown'));
});

test('hidden test reads preserve the last final verdict and remember a new final for the next run', async () => {
  const counts = vi.fn(); ipc(files([{ ...file, localResult: result('failed', 0, 1) }]));
  const view = render(<Overview {...props} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent, 1 failed'));
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null }) };
  ipc(files([unknown], { runActive: true })); view.rerender(<Overview {...props} active={false} filesRevision={1} activeDocumentId={id} runBusy onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent shown, 1 failed'));
  ipc(files([{ ...file, localResult: result('passed', 1, 1) }])); view.rerender(<Overview {...props} active={false} filesRevision={1} onCounts={counts} />);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent'));
  ipc(files([unknown], { runActive: true })); view.rerender(<Overview {...props} filesRevision={1} activeDocumentId={id} runBusy onCounts={counts} />);
  expect(within(await expanded()).getByLabelText('This computer, Passed')).toBeTruthy();
});

test('a hidden registration read cannot overwrite files from a later workspace refresh', async () => {
  ipc(); const view = render(<Overview {...props} filesRevision={0} />); await row(); view.rerender(<Overview {...props} active={false} filesRevision={0} />);
  let finish!: (value: unknown) => void;
  mockIPC(command => command === 'overview_files' ? new Promise(resolve => { finish = resolve; }) : none);
  view.rerender(<Overview {...props} active={false} filesRevision={1} />); await waitFor(() => expect(finish).toBeTypeOf('function'));
  ipc(files([named(id3, 'latest', { localResult: null })])); view.rerender(<Overview {...props} filesRevision={1} workspaceKey="new" />);
  await row('latest'); await act(async () => finish(files([named(id2, 'outdated', { localResult: null })])));
  expect(screen.queryByRole('heading', { name: 'outdated' })).toBeNull();
});

test('initial loading is not empty and saved files do not wait for live reads', async () => {
  mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : new Promise(() => {}));
  render(<Overview {...props} />); expect(screen.getByText('Reading saved files…')).toBeTruthy(); expect(screen.queryByText('No agents yet')).toBeNull();
  await row(); expect(screen.getByText('Reading agents…')).toBeTruthy();
});

test('list rows are compact, accessible by exact title, and evidence waits behind expansion', async () => {
  ipc(); render(<Overview {...props} />); const agent = await row();
  expect(agent.getAttribute('aria-label')).toBe('reader'); expect(within(agent).getByRole('heading', { name: 'reader' })).toBeTruthy();
  expect(within(agent).getByText('Reads a page')).toBeTruthy(); expect(within(agent).queryByText('3 of 3 passed')).toBeNull();
  expect(within(agent).getByLabelText('This computer: Passed')).toBeTruthy(); expect(agent.querySelector('time')?.dateTime).toBe('2009-02-13T23:31:30.000Z');
  expect(buttons(agent).map(b => b.getAttribute('aria-label'))).toEqual(['Show places for reader', 'Open reader']);
  await userEvent.click(within(agent).getByRole('button', { name: 'Show places for reader' }));
  const local = within(agent).getByRole('article', { name: 'This computer, Passed' });
  expect(within(local).getByText(/3 of 3 passed/).closest('p')?.textContent ?? within(local).getByText(/3 of 3 passed/).textContent).toContain('3 of 3 passed');
  expect(local.textContent).not.toMatch(/bridge-run|aaaa|run-uid/);
  await userEvent.click(within(local).getByRole('button', { name: 'Details' }));
  expect(within(local).getByText('bridge-run-1')).toBeTruthy(); expect(within(local).getByRole('button', { name: 'Copy Agent file hash' })).toBeTruthy();
});

test('AI readiness disables only the fitting local action with one shared reason', async () => {
  ipc(); const open = vi.fn(), chat = vi.fn(), run = vi.fn(); const own = { ...props, onOpenDocument: open, onChatDocument: chat, onRunDocument: run };
  const view = render(<Overview {...own} localAiConnected={false} />); const agent = await expanded();
  const reason = screen.getByText('Connect the AI in Clusters to chat or run tests on this computer.'); expect(reason.tabIndex).toBe(0);
  const fitting = within(agent).getByRole('button', { name: 'Chat reader' }); expect(fitting).toHaveProperty('disabled', true); expect(fitting.getAttribute('aria-describedby')).toBe(reason.id);
  await userEvent.click(fitting); expect(chat).not.toHaveBeenCalled();
  expect(within(agent).queryByRole('button', { name: 'Run tests reader' })).toBeNull();
  await userEvent.click(within(agent).getByRole('button', { name: 'Open reader' })); expect(open).toHaveBeenCalledWith(id);
  view.rerender(<Overview {...own} localAiConnected />); await userEvent.click(within(agent).getByRole('button', { name: 'Chat reader' })); expect(chat).toHaveBeenCalledWith(id);
  expect(screen.queryByText(reason.textContent!)).toBeNull();
});

test('fitting actions and absence callbacks do not preserve obsolete per-card controls', async () => {
  const other = named(id2, 'failing', { localResult: result('failed', 0, 2, { documentId: id2 }) });
  const fresh = named(id3, 'fresh', { localResult: null }); ipc(files([file, other, fresh]), live());
  render(<Overview {...props} />);
  const passed = await expanded(/^reader$/), failed = await expanded(/^failing$/), untested = await expanded(/^fresh$/);
  expect(within(passed).getByRole('button', { name: 'Chat reader' })).toBeTruthy(); expect(within(passed).queryByRole('button', { name: 'Run tests reader' })).toBeNull();
  expect(within(failed).getByRole('button', { name: 'Run tests failing' })).toBeTruthy(); expect(within(failed).queryByRole('button', { name: 'Chat failing' })).toBeNull();
  expect(within(untested).getByRole('button', { name: 'Run tests fresh' })).toBeTruthy();
  for (const agent of [passed, failed, untested]) expect(agent.querySelectorAll('.button-primary')).toHaveLength(0);
});

test('Open, Chat and Run report the document while missing optional callbacks remove only those actions', async () => {
  ipc(); const open = vi.fn(), chat = vi.fn(), run = vi.fn(); const view = render(<Overview {...props} onOpenDocument={open} onChatDocument={chat} onRunDocument={run} />);
  const agent = await expanded(); await userEvent.click(within(agent).getByRole('button', { name: 'Open reader' })); await userEvent.click(within(agent).getByRole('button', { name: 'Chat reader' }));
  expect([open, chat].map(f => f.mock.calls)).toEqual([[[id]], [[id]]]); expect(run).not.toHaveBeenCalled();
  view.rerender(<Overview {...props} onChatDocument={undefined} onRunDocument={undefined} />);
  expect(within(agent).queryByRole('button', { name: /^Chat|^Run tests/ })).toBeNull();
});

test('each saved file owns its local result and earlier failures stay failed', async () => {
  const earlier = named(id2, 'moved', { source: edited, localResult: result('failed', 0, 1, { documentId: id2 }) });
  const oldPass = named(id3, 'old-pass', { source: edited, localResult: result('passed', 1, 1, { documentId: id3 }) });
  const counts = vi.fn(); ipc(files([file, earlier, oldPass])); render(<Overview {...props} onCounts={counts} />);
  const failed = await expanded(/^moved$/); expect(within(failed).getByRole('article', { name: 'This computer, Failed' })).toBeTruthy(); expect(within(failed).getByText(/0 of 1 passed, before your edit/)).toBeTruthy();
  const stale = await expanded(/^old-pass$/); expect(within(stale).getByRole('article', { name: 'This computer, Not run since your edit' })).toBeTruthy();
  expect(within(await expanded(/^reader$/)).getByRole('article', { name: 'This computer, Passed' })).toBeTruthy();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('3 agents, 1 failed'));
});

test('a pass recorded on earlier files is stale and offers Run tests', async () => {
  ipc(files([{ ...file, source: edited }])); render(<Overview {...props} />); const agent = await expanded();
  const local = within(agent).getByRole('article', { name: 'This computer, Not run since your edit' });
  expect(within(local).getByText(/3 of 3 passed, before your edit/)).toBeTruthy(); expect(local.querySelector('.status-passed')).toBeNull();
  expect(within(local).getByRole('button', { name: 'Run tests reader' })).toBeTruthy();
});

test('Bridge test-run Agents are hidden by exact identity, never by name alone', async () => {
  const agents = [{ name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }, { name: 'bridge-run-1', namespace: 'orka-system', uid: 'user-uid' }, { name: 'reader', namespace: 'elsewhere', uid: 'run-uid' }];
  ipc(files(), live(agents)); render(<Overview {...props} />); const stray = await expanded(/^bridge-run-1$/);
  expect(screen.getAllByRole('heading', { name: 'bridge-run-1' })).toHaveLength(1); expect(screen.getAllByRole('article', { name: /^reader$/ })).toHaveLength(2);
  expect(within(stray).getByText('Running on this computer, not saved here')).toBeTruthy(); expect(within(stray).queryByText(/Passed|Failed|Not tested|Result unknown/)).toBeNull();
  await userEvent.click(within(stray).getByRole('button', { name: 'Details' })); expect(within(stray).getByText('Name on the cluster')).toBeTruthy();
});

test('cancelled observations stay neutral and unreadable history hides unowned Agents', async () => {
  ipc(files([{ ...file, localResult: result('cancelled', 0, 1) }]), live()); const counts = vi.fn(); const view = render(<Overview {...props} onCounts={counts} />);
  const local = within(await expanded()).getByRole('article', { name: 'This computer, Not tested' }); expect(local.querySelector('.status-passed, .status-failed')).toBeNull();
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('1 agent')); view.unmount();
  ipc(files([{ ...file, localResult: null }], { historyState: 'unavailable', runAgents: [] }), live([{ name: 'bridge-run-9', namespace: 'orka-system', uid: 'unknown' }]));
  render(<Overview {...props} />); expect(within(await expanded()).getByRole('article', { name: 'This computer, Result unknown' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'bridge-run-9' })).toBeNull(); expect(screen.getByText(/Test history is unavailable/)).toBeTruthy();
});

test('running local results retain previous finals and terminal Unknown replaces them', async () => {
  ipc(files([{ ...file, localResult: result('failed', 1, 2) }])); const view = render(<Overview {...props} />); const agent = await expanded();
  expect(within(agent).getByRole('article', { name: 'This computer, Failed' })).toBeTruthy();
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null, observedAt: 1234567890001 }) };
  ipc(files([unknown], { runActive: true })); view.rerender(<Overview {...props} filesRevision={1} activeDocumentId={id} runBusy />);
  expect(within(agent).getByRole('article', { name: 'This computer, Failed' })).toBeTruthy(); expect(within(agent).getByText(/1 of 2 passed/)).toBeTruthy();
  ipc(files([unknown])); view.rerender(<Overview {...props} filesRevision={2} />);
  await waitFor(() => expect(within(agent).getByRole('article', { name: 'This computer, Result unknown' })).toBeTruthy()); expect(screen.queryByText(/1 of 2 passed/)).toBeNull();
});

test('an unseen terminal Unknown for one file is not replaced by a cached pass while another is active', async () => {
  ipc(); const view = render(<Overview {...props} />); await expanded(); view.rerender(<Overview {...props} active={false} />);
  const unknown = { ...file, localResult: result('unknown', 0, 0, { state: 'unknown', report: null, observedAt: 1234567890001 }) };
  const other = named(id2, 'other', { localResult: result('unknown', 0, 0, { documentId: id2, state: 'unknown', report: null }) });
  ipc(files([unknown, other], { runActive: true })); view.rerender(<Overview {...props} activeDocumentId={id2} runBusy filesRevision={1} />);
  expect(within(await expanded()).getByRole('article', { name: 'This computer, Result unknown' })).toBeTruthy();
  expect(screen.queryByText(/of 3 passed/)).toBeNull();
});

test('file metadata is escaped text', async () => {
  ipc(files([{ ...file, summary: { ...file.summary, description: '<img src=x onerror=alert(1)>', tools: ['x'.repeat(256)] } }])); render(<Overview {...props} />);
  expect(await screen.findByText('<img src=x onerror=alert(1)>')).toBeTruthy(); expect(document.querySelector('img')).toBeNull();
});

test('a local test in progress keeps last good Agents and says why while removing readiness actions', async () => {
  const agents = [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }]; ipc(files(), live(agents)); const view = render(<Overview {...props} />);
  await row(/^mine$/); mockIPC(command => command === 'overview_files' ? files([file], { runActive: true }) : command === 'overview_places' ? none : live([]));
  view.rerender(<Overview {...props} runBusy />); expect(await screen.findByText(/Updating when the test finishes/)).toBeTruthy();
  expect(screen.getByRole('article', { name: /^mine$/ })).toBeTruthy(); const agent = await expanded(); expect(within(agent).queryByRole('button', { name: /^Chat|^Run tests/ })).toBeNull();
});

test('stopped workspace keeps saved rows and only Open at the parent; true empty offers onboarding', async () => {
  ipc(files(), unavailable); const view = render(<Overview {...props} />); expect(await screen.findByText('The workspace is stopped. Start it to test or chat.')).toBeTruthy();
  const agent = await expanded(); expect(within(agent).queryByRole('button', { name: /^Chat|^Run tests/ })).toBeNull(); expect(within(agent).getByRole('button', { name: 'Open reader' })).toBeTruthy();
  const choose = vi.fn(); view.rerender(<Overview {...props} onChooseWorkspace={choose} />); await userEvent.click(screen.getByRole('button', { name: 'Open Clusters' })); expect(choose).toHaveBeenCalled();
  view.unmount(); ipc(files([]), unavailable); render(<Overview {...props} onNewAgent={noop} />);
  expect(await screen.findByRole('heading', { name: 'No agents yet' })).toBeTruthy(); expect(screen.getAllByRole('button', { name: 'New agent' })).toHaveLength(1);
});

test('image-limited listing keeps saved results and fitting local action', async () => {
  ipc(files(), { ...unavailable, reason: 'image' }); render(<Overview {...props} />); const agent = await expanded();
  expect(screen.queryByText(/can't list its agents/)).toBeNull(); expect(within(agent).getByRole('article', { name: 'This computer, Passed' })).toBeTruthy();
  expect(within(agent).getByRole('button', { name: 'Chat reader' })).toBeTruthy();
});

test('failed reads retain data and never produce empty onboarding', async () => {
  ipc(files([]), { ...unavailable, reason: 'read' }); const view = render(<Overview {...props} />);
  expect(await screen.findByText("Couldn't check just now.")).toBeTruthy(); expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
  ipc({ state: 'unavailable', files: [], runActive: false }, live()); view.rerender(<Overview {...props} filesRevision={1} />);
  expect((await screen.findAllByText(/Couldn't read your saved files/)).length).toBeGreaterThan(0); expect(screen.queryByRole('heading', { name: 'No agents yet' })).toBeNull();
});

test.each(['loading', 'unavailable', 'unconnected'])('incomplete %s inventory labels the count as shown', async state => {
  const counts = vi.fn(); mockIPC(command => {
    if (command === 'overview_files') return files([]);
    if (command === 'overview_places') return state === 'unconnected' ? { ...none, places: [{ key: 'team', context: 'team', service: 'control/orka', namespace: 'agents', stale: true, observedAt: null, agents: [] }] } : none;
    return state === 'loading' ? new Promise(() => {}) : state === 'unavailable' ? unavailable : live();
  }); render(<Overview {...props} onCounts={counts} />); await waitFor(() => expect(counts).toHaveBeenLastCalledWith('0 agents shown'));
});

test('visible Run defers live requests and rereading after release coalesces', async () => {
  let calls = 0;
  mockIPC(command => { if (command === 'overview_files') return files([file], { runActive: true }); if (command === 'overview_places') return none; if (command === 'overview_agents') calls++; return live(); });
  const view = render(<Overview {...props} runBusy />); await row(); view.rerender(<Overview {...props} filesRevision={1} runBusy />); expect(calls).toBe(0);
  mockIPC(command => { if (command === 'overview_files') return files(); if (command === 'overview_places') return none; if (command === 'overview_agents') calls++; return live(); });
  view.rerender(<Overview {...props} filesRevision={1} />); await waitFor(() => expect(calls).toBe(1));
});

test('hidden post-Run completion does not read live; returning does', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : (command === 'overview_agents' && calls++, live()));
  const view = render(<Overview {...props} active={false} runBusy />); view.rerender(<Overview {...props} active={false} />); await act(async () => {}); expect(calls).toBe(0);
  view.rerender(<Overview {...props} />); await waitFor(() => expect(calls).toBe(1));
});

test('brief non-Run Busy retries once, while a read failure never retries as Busy', async () => {
  let calls = 0; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : (++calls === 1 ? { ...unavailable, state: 'busy', reason: null } : live()));
  render(<Overview {...props} />); await waitFor(() => expect(calls).toBe(2));
});

test('known test identities remain hidden after native cleanup forgets removed Agents', async () => {
  ipc(files(), live([{ name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }])); const view = render(<Overview {...props} />); await expanded();
  ipc(files([file], { runActive: true, runAgents: [] })); view.rerender(<Overview {...props} filesRevision={1} />);
  await screen.findByText(/Updating when the test finishes/);
  expect(screen.queryByRole('heading', { name: 'bridge-run-1' })).toBeNull();
});

test('failed file reread retains ownership-filtered Agents and never classifies a new inventory', async () => {
  const counts = vi.fn(); ipc(files(), live([{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }, { name: 'bridge-run-1', namespace: 'orka-system', uid: 'run-uid' }]));
  const view = render(<Overview {...props} onCounts={counts} />); await row(/^mine$/); let inventories = 0;
  mockIPC(command => { if (command === 'overview_files') throw Error('file read failed'); if (command === 'overview_places') return none; inventories++; return live([{ name: 'new-test-Agent', namespace: 'orka-system', uid: 'new-uid' }]); });
  view.rerender(<Overview {...props} filesRevision={1} onCounts={counts} />); await screen.findByText(/Couldn't read your saved files just now/);
  expect(screen.getByRole('article', { name: /^mine$/ })).toBeTruthy(); expect(screen.queryByRole('heading', { name: 'new-test-Agent' })).toBeNull(); expect(inventories).toBe(0);
  await waitFor(() => expect(counts).toHaveBeenLastCalledWith('2 agents shown'));
});

test('same workspace failure keeps what Bridge saw; a new lifetime clears it and ignores late replies', async () => {
  const agents = [{ name: 'mine', namespace: 'orka-system', uid: 'mine-uid' }]; ipc(files(), live(agents)); const view = render(<Overview {...props} />); await row(/^mine$/);
  ipc(files(), { ...unavailable, reason: 'read' }); view.rerender(<Overview {...props} filesRevision={1} />);
  expect(await screen.findByText(/Showing what Bridge saw/)).toBeTruthy(); expect(screen.getByRole('article', { name: /^mine$/ })).toBeTruthy();
  let resolve!: (value: unknown) => void; mockIPC(command => command === 'overview_files' ? files() : command === 'overview_places' ? none : new Promise(r => { resolve = r; }));
  view.rerender(<Overview {...props} filesRevision={2} />); await waitFor(() => expect(resolve).toBeTypeOf('function'));
  ipc(files(), { ...unavailable, reason: 'read' }); view.rerender(<Overview {...props} filesRevision={2} workspaceKey="closed" />);
  await screen.findByText("Couldn't check just now."); await act(async () => { resolve(live(agents)); });
  expect(screen.queryByRole('heading', { name: 'mine' })).toBeNull(); expect(screen.queryByText(/Showing what Bridge saw/)).toBeNull();
});
